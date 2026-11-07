//! rdc-codec / AMD AMF 编码器后端（Windows）
//!
//! 层级：L3 编解码层 · 平台适配（`platform/windows.rs` 的子模块）
//! 用途：让 `VideoEncoder` 的第三个后端直连 AMF（H.264 + D3D11 输入），
//!       MF 保留为回退；通过 `RDC_ENCODER=amf` 切换。
//!
//! 实现形态：AMF SDK 是 C++ 接口且 `amfrt64.dll` 只导出 `AMFInit`，
//! 因此 C++ 薄 shim（`crates/rdc-codec/shim/amf_enc.cpp` → `rdc-amf.dll`）
//! 负责 AMF 会话与事件驱动取包，本模块只做 C ABI 绑定与 trait 适配。
//!
//! 诚实的边界：
//! - 本机（RX 580 / 31.0.21912.14）实测：Usage=ULTRA_LOW_LATENCY 返回 INVALID_ARG(3)，
//!   自动回退 LOW_LATENCY；push p50 1.8ms / p99 3.5ms，首包第 1 帧即出。
//! - 输入按 BGRA 解释（采集帧为 Windows 原生 BGRA8）。
//! - 取包必然有一次纹理/系统内存回读，不宣称「完全零拷贝」。

#![allow(non_snake_case)]

use std::collections::VecDeque;
use std::ffi::{c_char, c_void, CStr};
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::time::Instant;

use rdc_runtime::PooledBuffer;
use windows::core::PCSTR;
use windows::Win32::Foundation::{FreeLibrary, HMODULE};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

use crate::encoder::{
    CodecKind, EncodedPacket, EncoderCaps, VideoConfig, VideoEncoder,
};
use crate::error::CodecError;

type OpenFn = unsafe extern "C" fn(i32, i32, i32, i32, i32, i32, *mut c_void) -> i32;
type PushRgbaFn = unsafe extern "C" fn(*const u8, i32, i64) -> i32;
type PollFn = unsafe extern "C" fn(*mut u8, i32, *mut i32, *mut i64, *mut i32) -> i32;
type DrainFn = unsafe extern "C" fn() -> i32;
type CloseFn = unsafe extern "C" fn();
type LastErrorFn = unsafe extern "C" fn() -> *const c_char;
type ProbeCapsFn = unsafe extern "C" fn(*mut i32, *mut i32, *mut i32) -> i32;

/// 内部排队深度：一进一出，实测见 shim 的 `queue_depth=1`。
const QUEUE_DEPTH: u32 = 1;

/// AMF 后端实例。
pub struct AmfEncoder {
    _lib: HMODULE,
    open: OpenFn,
    push_rgba: PushRgbaFn,
    poll: PollFn,
    drain: DrainFn,
    close: CloseFn,
    last_error: LastErrorFn,
    probe_caps: ProbeCapsFn,

    config: Option<VideoConfig>,
    pending: VecDeque<EncodedPacket>,
    out_buf: Vec<u8>,
    name: &'static str,
    warned_keyframe: bool,
}

// AMF 会话由单线程独占（与 MF 后端同一约定）；库句柄本身可跨线程持有。
unsafe impl Send for AmfEncoder {}

impl AmfEncoder {
    /// 尝试加载 `rdc-amf.dll` 并构造后端；DLL 不存在时返回 `None`（由调用方回退 MF）。
    pub fn try_new() -> Option<Self> {
        let lib = load_shim()?;
        unsafe {
            let open = symbol::<OpenFn>(lib, b"rdcamf_open\0")?;
            let push_rgba = symbol::<PushRgbaFn>(lib, b"rdcamf_push_rgba\0")?;
            let poll = symbol::<PollFn>(lib, b"rdcamf_poll\0")?;
            let drain = symbol::<DrainFn>(lib, b"rdcamf_drain\0")?;
            let close = symbol::<CloseFn>(lib, b"rdcamf_close\0")?;
            let last_error = symbol::<LastErrorFn>(lib, b"rdcamf_last_error\0")?;
            let probe_caps = symbol::<ProbeCapsFn>(lib, b"rdcamf_probe_caps\0")?;
            Some(Self {
                _lib: lib,
                open,
                push_rgba,
                poll,
                drain,
                close,
                last_error,
                probe_caps,
                config: None,
                pending: VecDeque::new(),
                out_buf: vec![0u8; 4 * 1024 * 1024],
                name: "amf-h264-d3d11",
                warned_keyframe: false,
            })
        }
    }

    fn error_text(&self) -> String {
        unsafe {
            let p = (self.last_error)();
            if p.is_null() {
                return String::from("unknown");
            }
            CStr::from_ptr(p).to_string_lossy().into_owned()
        }
    }

    fn poll_raw(&mut self) -> Option<EncodedPacket> {
        let mut len = 0i32;
        let mut pts_us = 0i64;
        let mut key = 0i32;
        let rc = unsafe {
            (self.poll)(
                self.out_buf.as_mut_ptr(),
                self.out_buf.len() as i32,
                &mut len,
                &mut pts_us,
                &mut key,
            )
        };
        if rc != 1 || len <= 0 {
            return None;
        }
        Some(EncodedPacket {
            data: self.out_buf[..len as usize].to_vec(),
            pts_ns: (pts_us.max(0) as u64) * 1000,
            is_keyframe: key != 0,
            suggested_shard_bytes: 0,
        })
    }

    /// 探测可用性（供 backend 选择使用）。
    pub fn probe_caps(&self) -> (bool, bool) {
        let mut h264 = 0i32;
        let mut hevc = 0i32;
        unsafe {
            (self.probe_caps)(&mut h264, &mut hevc, std::ptr::null_mut());
        }
        (h264 != 0, hevc != 0)
    }
}

impl Drop for AmfEncoder {
    fn drop(&mut self) {
        if self.config.is_some() {
            unsafe { (self.close)() };
        }
        unsafe {
            let _ = FreeLibrary(self._lib);
        }
    }
}

impl VideoEncoder for AmfEncoder {
    fn probe(&self, codecs: &[CodecKind]) -> Vec<EncoderCaps> {
        let (h264, _hevc) = self.probe_caps();
        if !h264 {
            return Vec::new();
        }
        let want = |k: CodecKind| codecs.is_empty() || codecs.contains(&k);
        if !want(CodecKind::H264) {
            return Vec::new();
        }
        vec![EncoderCaps {
            codec: CodecKind::H264,
            max_width: 3840,
            max_height: 2160,
            max_fps: 60,
            supports_444: false,
            supports_10bit: false,
            supports_runtime_reconfigure: false,
        }]
    }

    fn configure(&mut self, config: VideoConfig) -> Result<(), CodecError> {
        if config.codec != CodecKind::H264 {
            return Err(CodecError::Unsupported);
        }
        if config.width == 0 || config.height == 0 || config.bitrate_bps == 0 {
            return Err(CodecError::InvalidConfig(format!(
                "AMF 配置非法: {}x{} @ {} bps",
                config.width, config.height, config.bitrate_bps
            )));
        }
        if self.config.is_some() {
            unsafe { (self.close)() };
            self.config = None;
        }
        // input_mode=0：host BGRA（采集帧为 BGRA8）。纹理直入路径已由 spike 实测
        // （DX11 BGRA p50 1.8ms / VP→NV12 p50 1.9ms），待采集侧交付 D3D11 纹理后启用。
        let rc = unsafe {
            (self.open)(
                config.width as i32,
                config.height as i32,
                config.fps as i32,
                config.bitrate_bps as i32,
                0,
                0,
                std::ptr::null_mut(),
            )
        };
        if rc != 0 {
            let msg = self.error_text();
            tracing::error!("[amf] 打开失败: {}", msg);
            return Err(CodecError::HardwareUnsupported);
        }
        tracing::info!(
            "[amf] 后端已打开 {}x{}@{} bitrate={} queue_depth={} usage_effective=probed",
            config.width,
            config.height,
            config.fps,
            config.bitrate_bps,
            QUEUE_DEPTH
        );
        self.config = Some(config);
        self.pending.clear();
        Ok(())
    }

    fn push(
        &mut self,
        frame: PooledBuffer,
        pts_ns: u64,
        force_keyframe: bool,
    ) -> Result<(), CodecError> {
        let cfg = self.config.ok_or(CodecError::HardwareUnsupported)?;
        let expected = (cfg.width as usize) * (cfg.height as usize) * 4;
        let data = frame.as_slice();
        if data.len() < expected {
            return Err(CodecError::EncoderFailure(format!(
                "帧长度 {} < {}（{}x{} BGRA）",
                data.len(),
                expected,
                cfg.width,
                cfg.height
            )));
        }
        if force_keyframe && !self.warned_keyframe {
            self.warned_keyframe = true;
            tracing::warn!("[amf] shim 未暴露强制 IDR 接口，关键帧请求按 GOP 周期生效");
        }
        let rc = unsafe {
            (self.push_rgba)(
                data.as_ptr(),
                (cfg.width as i32) * 4,
                (pts_ns / 1000) as i64,
            )
        };
        if rc != 0 {
            return Err(CodecError::EncoderFailure(format!(
                "AMF SubmitInput 失败: {}",
                self.error_text()
            )));
        }
        Ok(())
    }

    fn poll_packet(&mut self) -> Option<EncodedPacket> {
        self.pending.pop_front().or_else(|| self.poll_raw())
    }

    fn reconfigure(&mut self, config: VideoConfig) -> Result<(), CodecError> {
        // AMF shim 尚未暴露动态码控接口：重配走重建，代价一次会话初始化。
        self.configure(config)
    }

    fn request_keyframe(&mut self) {
        if !self.warned_keyframe {
            self.warned_keyframe = true;
            tracing::warn!("[amf] shim 未暴露强制 IDR 接口，关键帧请求按 GOP 周期生效");
        }
    }

    fn backend_name(&self) -> &str {
        self.name
    }

    fn flush(&mut self) -> Result<Vec<EncodedPacket>, CodecError> {
        if self.config.is_none() {
            return Ok(Vec::new());
        }
        unsafe {
            if (self.drain)() != 0 {
                return Err(CodecError::EncoderFailure(format!(
                    "AMF Drain 失败: {}",
                    self.error_text()
                )));
            }
        }
        let mut out = Vec::new();
        let deadline = Instant::now() + std::time::Duration::from_millis(50);
        loop {
            if let Some(p) = self.poll_raw() {
                out.push(p);
                continue;
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        Ok(out)
    }
}

// =====** DLL 加载 =====**

fn shim_candidates() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(p) = std::env::var("RDC_AMF_DLL") {
        v.push(PathBuf::from(p));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            v.push(dir.join("rdc-amf.dll"));
        }
    }
    // 开发期：target/amf/rdc-amf.dll（build_amf.ps1 的默认输出）
    v.push(PathBuf::from("target/amf/rdc-amf.dll"));
    v
}

fn load_shim() -> Option<HMODULE> {
    for path in shim_candidates() {
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(std::iter::once(0)).collect();
        if !path.exists() {
            continue;
        }
        if let Ok(h) = unsafe { LoadLibraryW(windows::core::PCWSTR(wide.as_ptr())) } {
            return Some(h);
        }
    }
    None
}

unsafe fn symbol<T: Copy>(lib: HMODULE, name: &[u8]) -> Option<T> {
    let f = GetProcAddress(lib, PCSTR(name.as_ptr()))?;
    Some(std::mem::transmute_copy(&f))
}
