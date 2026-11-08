//! rdc-decode / FFmpeg H.264 解码器（LGPL 动态链接）
//!
//! 层级：L3 编解码层 · 平台适配最内层
//! 用途：用 libavcodec 解码 Annex-B H.264，逐帧交付 NV12。作为 MF 路径的可切换替代：
//!       `RDC_DECODER=ffmpeg` 时启用，`RDC_FFMPEG_MODE=d3d11` 额外走 d3d11va 硬解
//!       （输出直接是 GPU 纹理，不做回读）。
//! 调用方：rdc-decode::video_decoder 的选择入口、rdc-cli 的 S3b-4 实测工具
//!
//! 低延迟：`AV_CODEC_FLAG_LOW_DELAY` + 单线程（或 slice 线程），不启用 frame-level
//! 多线程，避免 (N−1) 帧重排延迟。`bf=0` 的码流下单帧送→收无额外排队。
//!
//! 许可：LGPL v3 共享构建，动态链接；禁止静态链接，禁止链接 GPL 组件。

use std::ffi::{c_char, c_void, CStr};
use std::sync::Once;

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D11::ID3D11Device;
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;

use crate::error::DecodeError;

use super::mf_decoder::{DecodedFrame, OutputKind, TextureFrame};

/// 错误缓冲长度。
const ERR_LEN: usize = 256;

/// C 侧一帧输出（与 `shim/ffdec.c` 的 `RdcFfFrame` 严格同布局）。
#[repr(C)]
struct RdcFfFrame {
    /// 系统内存路径：Y 平面；D3D11 路径：`ID3D11Texture2D*`。
    plane0: *const u8,
    /// 系统内存路径：UV 平面。
    plane1: *const u8,
    /// Y 平面行距（字节）。
    stride0: i32,
    /// UV 平面行距（字节）。
    stride1: i32,
    width: i32,
    height: i32,
    /// 非 0 表示 `plane0` 是 GPU 纹理。
    is_texture: i32,
    /// 纹理数组下标。
    array_slice: i32,
    /// 该帧 pts（微秒）。
    pts_us: i64,
}

/// C 侧累计统计。
#[repr(C)]
#[derive(Default, Clone, Copy)]
struct RdcFfStats {
    feeds: i64,
    frames: i64,
    decode_ms_total: f64,
    pack_ms_total: f64,
    threads: i32,
    delay_frames: i32,
}

/// 帧回调签名（返回非 0 表示中止）。
type FrameCb = extern "C" fn(*mut c_void, *const RdcFfFrame) -> i32;

/// shim 日志回调签名：`level` 用 `AV_LOG_*` 数值，`msg` 是已经格式化好的整行。
type LogCb = extern "C" fn(i32, *const c_char);

/// `AV_LOG_ERROR`（libavutil/log.h）。
const AV_LOG_ERROR: i32 = 16;
/// `AV_LOG_WARNING`。
const AV_LOG_WARNING: i32 = 24;
/// `AV_LOG_INFO`。
const AV_LOG_INFO: i32 = 32;
/// `AV_LOG_VERBOSE`。
const AV_LOG_VERBOSE: i32 = 40;

/// 把 shim 日志转发到 `tracing`，保持原文（不添删任何字符）。
extern "C" fn log_trampoline(level: i32, msg: *const c_char) {
    if msg.is_null() {
        return;
    }
    let text = unsafe { CStr::from_ptr(msg) }.to_string_lossy();
    match level {
        l if l <= AV_LOG_ERROR => tracing::error!(target: "rdc_decode::ffmpeg", "{text}"),
        l if l <= AV_LOG_WARNING => tracing::warn!(target: "rdc_decode::ffmpeg", "{text}"),
        l if l <= AV_LOG_INFO => tracing::info!(target: "rdc_decode::ffmpeg", "{text}"),
        l if l <= AV_LOG_VERBOSE => tracing::debug!(target: "rdc_decode::ffmpeg", "{text}"),
        _ => tracing::trace!(target: "rdc_decode::ffmpeg", "{text}"),
    }
}

/// shim 的回调是进程级状态，用 `Once` 只注册一次。
static LOGGER_ONCE: Once = Once::new();

/// 注册 shim 日志出口（幂等）。注册失败不可能发生，`Once` 保证并发安全。
fn ensure_logger_registered() {
    LOGGER_ONCE.call_once(|| unsafe { rdff_set_logger(Some(log_trampoline)) });
}

extern "C" {
    fn rdff_set_logger(cb: Option<LogCb>);
    fn rdff_create(
        threads: i32,
        low_delay: i32,
        out: *mut *mut c_void,
        err: *mut c_char,
        errlen: i32,
    ) -> i32;
    fn rdff_bind_d3d11(dec: *mut c_void, device: *mut c_void, err: *mut c_char, errlen: i32)
        -> i32;
    fn rdff_open(dec: *mut c_void, err: *mut c_char, errlen: i32) -> i32;
    fn rdff_feed(
        dec: *mut c_void,
        data: *const u8,
        len: i32,
        pts_us: i64,
        cb: FrameCb,
        ctx: *mut c_void,
        err: *mut c_char,
        errlen: i32,
    ) -> i32;
    fn rdff_drain(
        dec: *mut c_void,
        cb: FrameCb,
        ctx: *mut c_void,
        err: *mut c_char,
        errlen: i32,
    ) -> i32;
    fn rdff_flush(
        dec: *mut c_void,
        cb: FrameCb,
        ctx: *mut c_void,
        err: *mut c_char,
        errlen: i32,
    ) -> i32;
    fn rdff_stats(dec: *mut c_void, out: *mut RdcFfStats);
    fn rdff_destroy(dec: *mut c_void);
}

/// 一次收帧的回调状态。
struct Sink<'a> {
    /// 上层逐帧处理。
    on_frame: Box<dyn FnMut(DecodedFrame) -> Result<(), DecodeError> + 'a>,
    /// 上层返回的错误（中止时保留）。
    error: Option<DecodeError>,
    /// 本次收帧产生的帧数。
    produced: u64,
    /// 本次收帧里 GPU 纹理帧的数量（用于判定实际输出类型）。
    texture_frames: u64,
}

/// 把 C 帧转成 Rust 帧；系统内存路径按紧密 NV12 拷贝，GPU 路径只 AddRef 纹理。
fn convert_frame(frame: &RdcFfFrame) -> Result<DecodedFrame, DecodeError> {
    let width = u32::try_from(frame.width).unwrap_or(0);
    let height = u32::try_from(frame.height).unwrap_or(0);
    if frame.is_texture != 0 {
        if frame.plane0.is_null() {
            return Err(DecodeError::DecodeFailure(
                "FFmpeg 返回空 GPU 纹理".to_owned(),
            ));
        }
        // FFmpeg 只在 AVFrame 生命周期内持有这个引用；先 AddRef 再放手，
        // 保证上屏期间纹理不会被解码器回收（绝不 Release 别人的引用）。
        let borrowed = unsafe { ID3D11Texture2D::from_raw(frame.plane0 as *mut c_void) };
        let texture = borrowed.clone();
        std::mem::forget(borrowed);
        return Ok(DecodedFrame {
            ts_us: frame.pts_us.max(0) as u64,
            width,
            height,
            stride: width,
            data: Vec::new(),
            texture: Some(TextureFrame {
                texture,
                array_slice: frame.array_slice.max(0) as u32,
                sample: None,
            }),
        });
    }
    let pixels = (width as usize)
        .saturating_mul(height as usize)
        .saturating_mul(3)
        / 2;
    if frame.plane0.is_null() || pixels == 0 {
        return Err(DecodeError::DecodeFailure("FFmpeg 返回空 NV12".to_owned()));
    }
    let mut data = vec![0u8; pixels];
    // shim 已把两平面整理成「行距 = width」的紧密排列，这里一次整块拷贝。
    unsafe {
        std::ptr::copy_nonoverlapping(frame.plane0, data.as_mut_ptr(), pixels);
    }
    Ok(DecodedFrame {
        ts_us: frame.pts_us.max(0) as u64,
        width,
        height,
        stride: width,
        data,
        texture: None,
    })
}

/// C 回调入口。
extern "C" fn frame_trampoline(ctx: *mut c_void, frame: *const RdcFfFrame) -> i32 {
    if ctx.is_null() || frame.is_null() {
        return 1;
    }
    // SAFETY: 指针来自下方 `feed_with` 的栈上 `Sink`，只在调用期间有效。
    let sink = unsafe { &mut *(ctx as *mut Sink<'_>) };
    let frame = unsafe { &*frame };
    if frame.is_texture != 0 {
        sink.texture_frames += 1;
    }
    match convert_frame(frame) {
        Ok(decoded) => match (sink.on_frame)(decoded) {
            Ok(()) => {
                sink.produced += 1;
                0
            }
            Err(error) => {
                sink.error = Some(error);
                1
            }
        },
        Err(error) => {
            sink.error = Some(error);
            1
        }
    }
}

/// FFmpeg H.264 解码会话。
pub struct FfmpegH264Decoder {
    /// C 侧实例。
    handle: *mut c_void,
    /// 输出尺寸。
    width: u32,
    /// 输出高度。
    height: u32,
    /// 期望的 GPU 路径（实际以输出缓冲类型复核）。
    hw_requested: bool,
    /// 硬解时保持设备生命周期长于解码器。
    device: Option<ID3D11Device>,
    /// 已送入帧数。
    fed: u64,
    /// 已产出帧数。
    produced: u64,
    /// 实际输出缓冲类型。
    output_kind: OutputKind,
    /// 解码器名（报告用）。
    name: String,
}

/// 一次调用 shim 的方式。
#[derive(Clone, Copy, PartialEq, Eq)]
enum FeedMode {
    /// 送一个访问单元并收帧。
    Feed,
    /// 只收帧（不送包、不发送 EOS）。
    Drain,
    /// 发送 EOS 并取尽剩余帧。
    Flush,
}

impl FfmpegH264Decoder {
    /// 打开解码器。
    ///
    /// # Arguments
    /// * `device` - 呈现窗口的 D3D11 设备；`Some` 且 `prefer_hardware` 时走 d3d11va
    /// * `width` / `height` - 期望尺寸（仅用于报告与校验，实际尺寸以 SPS 为准）
    /// * `fps` - 期望帧率（FFmpeg 解码不依赖它，仅保留接口一致）
    /// * `prefer_hardware` - 是否优先走 d3d11va
    ///
    /// # Errors
    /// shim 编译缺失、libavcodec 不可用、打开失败时返回 [`DecodeError::DecoderUnavailable`]。
    pub fn open_with_device(
        device: Option<&ID3D11Device>,
        width: u32,
        height: u32,
        _fps: u32,
        prefer_hardware: bool,
    ) -> Result<Self, DecodeError> {
        let use_hw = prefer_hardware && device.is_some();
        // 必须先接好日志出口：shim 的 create/open/feed 都会打诊断。
        ensure_logger_registered();
        let mut handle: *mut c_void = std::ptr::null_mut();
        let mut err = [0 as c_char; ERR_LEN];
        // 单线程：绝不启用 frame-level 多线程（那会引入 (N−1) 帧延迟）。
        let rc = unsafe { rdff_create(1, 1, &mut handle, err.as_mut_ptr(), ERR_LEN as i32) };
        if rc < 0 || handle.is_null() {
            return Err(DecodeError::DecoderUnavailable(format!(
                "rdff_create 失败: {}",
                c_err(&err)
            )));
        }
        let decoder = Self {
            handle,
            width,
            height,
            hw_requested: use_hw,
            device: if use_hw { device.cloned() } else { None },
            fed: 0,
            produced: 0,
            output_kind: OutputKind::Unknown,
            name: if use_hw {
                "FFmpeg H.264 (d3d11va)".to_owned()
            } else {
                "FFmpeg H.264 (software, 1 thread)".to_owned()
            },
        };
        if use_hw {
            let raw = decoder.device.as_ref().unwrap().as_raw();
            let rc =
                unsafe { rdff_bind_d3d11(decoder.handle, raw, err.as_mut_ptr(), ERR_LEN as i32) };
            if rc < 0 {
                // 硬解绑定失败按显式错误上报，由上层决定回退 MF 还是改走软解。
                return Err(DecodeError::DecoderUnavailable(format!(
                    "d3d11va 绑定失败: {}",
                    c_err(&err)
                )));
            }
        }
        let rc = unsafe { rdff_open(decoder.handle, err.as_mut_ptr(), ERR_LEN as i32) };
        if rc < 0 {
            return Err(DecodeError::DecoderUnavailable(format!(
                "avcodec_open2 失败: {}",
                c_err(&err)
            )));
        }
        Ok(decoder)
    }

    /// 解码器内部延迟帧数（`delay + has_b_frames`）。
    pub fn delay_frames(&self) -> i64 {
        let mut stats = RdcFfStats::default();
        unsafe { rdff_stats(self.handle, &mut stats) };
        i64::from(stats.delay_frames)
    }

    /// 累计耗时与帧数（报告用）。
    pub fn perf(&self) -> (i64, i64, f64, f64) {
        let mut stats = RdcFfStats::default();
        unsafe { rdff_stats(self.handle, &mut stats) };
        (
            stats.feeds,
            stats.frames,
            stats.decode_ms_total,
            stats.pack_ms_total,
        )
    }

    /// 送入一个访问单元（或只收帧/冲刷），逐帧回调。
    fn feed_with(
        &mut self,
        annexb: &[u8],
        ts_us: u64,
        mode: FeedMode,
        sink: &mut Sink<'_>,
    ) -> Result<(), DecodeError> {
        if annexb.is_empty() && mode == FeedMode::Feed {
            return Err(DecodeError::InvalidArgument("码流为空".to_owned()));
        }
        let mut err = [0 as c_char; ERR_LEN];
        let ctx = sink as *mut Sink<'_> as *mut c_void;
        let rc = unsafe {
            match mode {
                FeedMode::Feed => rdff_feed(
                    self.handle,
                    annexb.as_ptr(),
                    annexb.len() as i32,
                    ts_us as i64,
                    frame_trampoline,
                    ctx,
                    err.as_mut_ptr(),
                    ERR_LEN as i32,
                ),
                FeedMode::Drain => rdff_drain(
                    self.handle,
                    frame_trampoline,
                    ctx,
                    err.as_mut_ptr(),
                    ERR_LEN as i32,
                ),
                FeedMode::Flush => rdff_flush(
                    self.handle,
                    frame_trampoline,
                    ctx,
                    err.as_mut_ptr(),
                    ERR_LEN as i32,
                ),
            }
        };
        if let Some(error) = sink.error.take() {
            return Err(error);
        }
        if rc < 0 {
            return Err(DecodeError::DecodeFailure(format!(
                "FFmpeg 解码失败: {}",
                c_err(&err)
            )));
        }
        if mode == FeedMode::Feed {
            self.fed = self.fed.saturating_add(1);
        }
        self.produced = self.produced.saturating_add(sink.produced);
        if sink.produced > 0 {
            self.output_kind = if sink.texture_frames > 0 {
                OutputKind::DxgiTexture
            } else {
                OutputKind::SystemMemory
            };
        }
        Ok(())
    }

    /// 送入一个访问单元并逐帧回调（与 MF 的 `feed_each` 同语义）。
    pub fn feed_each<F>(
        &mut self,
        annexb: &[u8],
        ts_us: u64,
        on_frame: F,
    ) -> Result<(), DecodeError>
    where
        F: FnMut(DecodedFrame) -> Result<(), DecodeError>,
    {
        let mut sink = Sink {
            on_frame: Box::new(on_frame),
            error: None,
            produced: 0,
            texture_frames: 0,
        };
        self.feed_with(annexb, ts_us, FeedMode::Feed, &mut sink)
    }

    /// 只收帧、不送包（不发送 EOS，用于补收已就绪输出）。
    pub fn pump(&mut self) -> Result<Vec<DecodedFrame>, DecodeError> {
        let mut outs = Vec::new();
        let mut sink = Sink {
            on_frame: Box::new(|frame| {
                outs.push(frame);
                Ok(())
            }),
            error: None,
            produced: 0,
            texture_frames: 0,
        };
        self.feed_with(&[], 0, FeedMode::Drain, &mut sink)?;
        drop(sink);
        Ok(outs)
    }

    /// 冲刷并取尽剩余帧。
    pub fn flush(&mut self) -> Result<Vec<DecodedFrame>, DecodeError> {
        let mut outs = Vec::new();
        let mut sink = Sink {
            on_frame: Box::new(|frame| {
                outs.push(frame);
                Ok(())
            }),
            error: None,
            produced: 0,
            texture_frames: 0,
        };
        // 发送 NULL 包（EOS）后取尽剩余输出。
        self.feed_with(&[], 0, FeedMode::Flush, &mut sink)?;
        drop(sink);
        Ok(outs)
    }

    /// 期望输出尺寸（实际以 SPS 为准，报告用）。
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// 解码器名。
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 是否真正走 GPU 硬解（请求 + 实际输出纹理）。
    pub fn hardware_active(&self) -> bool {
        self.hw_requested && self.output_kind == OutputKind::DxgiTexture
    }

    /// 实际输出缓冲类型。
    pub fn output_kind(&self) -> OutputKind {
        self.output_kind
    }

    /// 已送 − 已出（内部排队帧数）。
    pub fn in_flight(&self) -> u64 {
        self.fed.saturating_sub(self.produced)
    }
}

impl Drop for FfmpegH264Decoder {
    fn drop(&mut self) {
        // 先释放解码器，再让持有的设备引用随结构体一起释放。
        if !self.handle.is_null() {
            unsafe { rdff_destroy(self.handle) };
            self.handle = std::ptr::null_mut();
        }
    }
}

/// 取 C 错误缓冲文本。
fn c_err(buf: &[c_char; ERR_LEN]) -> String {
    let text: String = buf
        .iter()
        .take_while(|c| **c != 0)
        .map(|c| (*c as u8) as char)
        .collect();
    if text.is_empty() {
        "未知错误".to_owned()
    } else {
        text
    }
}
