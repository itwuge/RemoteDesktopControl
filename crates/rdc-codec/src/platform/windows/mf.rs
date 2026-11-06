//! rdc-codec / Windows Media Foundation H.264 编码封装
//!
//! 层级：L3 编解码层 · 平台适配最内层
//! 用途：枚举硬件 H.264 编码器 MFT，配置输入（NV12）与输出（H.264），
//!       同步提交原始帧并取回码流；提供码率/GOP/强制关键帧的 ICodecAPI 热更新
//! 调用方：rdc-codec::platform::windows::PlatformEncoder
//!
//! 安全边界（unsafe）：
//! - 所有 COM 调用都在此文件内；[`MfH264Session`] 独占一个 MFT，必须由单一编码线程使用。
//! - `Lock` 指针只在「Lock → 拷贝 → Unlock」区间内有效，绝不跨调用保存。
//! - `MFTEnumEx` 返回的激活对象数组由 COM 分配器分配，复制完成后必须 `CoTaskMemFree`。

use std::mem::ManuallyDrop;
use std::sync::OnceLock;

use rdc_protocol::EncoderConfig;
use windows::core::{GUID, Interface, VARIANT};
use windows::Win32::Media::MediaFoundation::{
    eAVEncH264VProfile_High, eAVEncH264VProfile_Main, ICodecAPI, IMFActivate,
    IMFMediaEventGenerator, IMFMediaType, IMFSample, IMFTransform, MFCreateMediaType,
    MFCreateMemoryBuffer, MFCreateSample, MFMediaType_Video, MFTEnumEx, MFVideoFormat_H264,
    MFVideoFormat_NV12, MFVideoInterlace_Progressive,
    MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE,
    MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE, MF_MT_MPEG2_PROFILE, MF_MT_MPEG_SEQUENCE_HEADER,
    MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MF_TRANSFORM_ASYNC_UNLOCK,
    MFSTARTUP_LITE, MFStartup, MFSampleExtension_CleanPoint, MFT_CATEGORY_VIDEO_ENCODER,
    MFT_ENUM_FLAG, MFT_ENUM_FLAG_ALL, MFT_ENUM_FLAG_HARDWARE, MFT_ENUM_FLAG_SORTANDFILTER,
    MFT_MESSAGE_COMMAND_DRAIN,
    MFT_MESSAGE_COMMAND_FLUSH, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING,
    MFT_MESSAGE_NOTIFY_END_OF_STREAM, MFT_MESSAGE_NOTIFY_END_STREAMING,
    MFT_MESSAGE_NOTIFY_START_OF_STREAM, MFT_OUTPUT_DATA_BUFFER, MFT_OUTPUT_STREAM_PROVIDES_SAMPLES,
    MFT_REGISTER_TYPE_INFO, MF_E_NO_EVENTS_AVAILABLE, MF_E_TRANSFORM_NEED_MORE_INPUT,
    MF_E_TRANSFORM_STREAM_CHANGE, MF_VERSION, MF_LOW_LATENCY, CODECAPI_AVEncCommonLowLatency,
    CODECAPI_AVEncCommonMeanBitRate, CODECAPI_AVEncCommonRateControlMode,
    CODECAPI_AVEncMPVDefaultBPictureCount, CODECAPI_AVEncMPVGOPSize,
    CODECAPI_AVEncVideoForceKeyFrame, CODECAPI_AVEncVideoMaxNumRefFrame,
    CODECAPI_AVEncCommonMaxBitRate, CODECAPI_AVEncCommonQualityVsSpeed,
    MFT_FRIENDLY_NAME_Attribute,
    MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS, METransformHaveOutput, METransformNeedInput,
    MF_EVENT_FLAG_NO_WAIT,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::Variant::VT_BOOL;

use crate::CodecError;

// =====** VT_BOOL VARIANT =====**

/// 手工构造的 `VT_BOOL` VARIANT（windows-core 未提供 bool 构造器，布局与原生 VARIANT 一致）。
#[repr(C)]
struct BoolVariant {
    vt: u16,
    reserved: [u16; 3],
    value: i16,
    padding: [u8; 14],
}

impl BoolVariant {
    fn new(flag: bool) -> Self {
        Self {
            vt: VT_BOOL.0,
            reserved: [0; 3],
            value: if flag { -1 } else { 0 },
            padding: [0; 14],
        }
    }

    fn as_variant(&self) -> &VARIANT {
        unsafe { &*(self as *const Self as *const VARIANT) }
    }
}

// =====** MF 初始化 =====**

/// 进程级 Media Foundation 初始化结果（只执行一次）。
static MF_INIT: OnceLock<Result<(), String>> = OnceLock::new();

/// 确保 Media Foundation 已初始化。
///
/// # Returns
/// 已初始化返回 `Ok(())`。
///
/// # Errors
/// `MFStartup` 失败时返回 [`CodecError::EncoderFailure`]，并保留原始错误串。
pub fn ensure_mf() -> Result<(), CodecError> {
    match MF_INIT.get_or_init(|| unsafe {
        // MFSTARTUP_LITE 不初始化网络组件；本阶段只需要本地编码器。
        MFStartup(MF_VERSION, MFSTARTUP_LITE).map_err(|e| format!("MFStartup: {e}"))
    }) {
        Ok(()) => Ok(()),
        Err(msg) => Err(CodecError::EncoderFailure(format!(
            "Media Foundation 初始化失败: {msg}"
        ))),
    }
}

// =====** 硬件枚举 =====**

/// 枚举硬件 H.264 编码器（NV12 → H.264），可选包含同步软件 MFT。
///
/// # Arguments
/// * `include_software` - 是否纳入同步软件 MFT；只有显式允许软编时才为 `true`，
///   默认路径绝不返回软件编码器（避免「静默软编」）。
///
/// # Returns
/// 按系统排序的激活对象列表；无可用编码器时返回空列表。
///
/// # Errors
/// `MFTEnumEx` 失败返回 [`CodecError::EncoderFailure`]。
pub fn enumerate_h264(include_software: bool) -> Result<Vec<IMFActivate>, CodecError> {
    ensure_mf()?;
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_NV12,
    };
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };
    let flags: MFT_ENUM_FLAG = if include_software {
        // 需要软编时放开全部类别（含同步软件 MFT），由上层显式标注为软件路径。
        MFT_ENUM_FLAG_ALL
    } else {
        MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER
    };
    let mut raw: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    unsafe {
        if let Err(e) = MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            flags,
            Some(&input),
            Some(&output),
            &mut raw,
            &mut count,
        ) {
            return Err(CodecError::EncoderFailure(format!("MFTEnumEx: {e}")));
        }
        if raw.is_null() || count == 0 {
            return Ok(Vec::new());
        }
        // 激活对象是 COM 引用计数对象，先复制到 Vec，再释放原始数组。
        let slice = std::slice::from_raw_parts(raw, count as usize);
        let mut out = Vec::with_capacity(slice.len());
        for act in slice.iter().flatten() {
            out.push(act.clone());
        }
        CoTaskMemFree(Some(raw as *const core::ffi::c_void));
        Ok(out)
    }
}

/// 读取编码器友好名称。
///
/// # Arguments
/// * `activate` - MFT 激活对象。
///
/// # Returns
/// 名称字符串；读取失败返回 `"MF H.264 encoder"`。
///
/// # Errors
/// 不会失败。
pub fn activation_name(activate: &IMFActivate) -> String {
    let mut buf = [0u16; 128];
    let mut len = 0u32;
    unsafe {
        if activate
            .GetString(&MFT_FRIENDLY_NAME_Attribute, &mut buf, Some(&mut len))
            .is_ok()
            && len > 0
        {
            let n = (len as usize).min(buf.len());
            let end = buf[..n].iter().position(|c| *c == 0).unwrap_or(n);
            if end > 0 {
                return String::from_utf16_lossy(&buf[..end]);
            }
        }
    }
    "MF H.264 encoder".to_owned()
}

// =====** 编码输出样本 =====**

/// `CODECAPI_AVEncCommonRateControlMode` 的「交给驱动决定」取值（Uninitialized）。
const RATE_CONTROL_DRIVER_DEFAULT: u32 = 0;

/// 一帧编码输出（时间戳使用 Media Foundation 的 100ns 单位）。
#[derive(Debug, Clone)]
pub struct EncodedSample {
    /// 码流字节（长度前缀或起始码由 MFT 决定，调用方按需转换）。
    pub data: Vec<u8>,
    /// 是否关键帧。
    pub keyframe: bool,
    /// 采样时间（100ns）。
    pub pts_100ns: i64,
}

// =====** MfH264Session =====**

/// 单个 H.264 编码会话。
pub struct MfH264Session {
    /// MFT 变换。
    transform: IMFTransform,
    /// 编码器参数接口（部分 MFT 不支持）。
    codec_api: Option<ICodecAPI>,
    /// 异步事件源（硬件 MFT 通常异步；软件 MFT 为 `None`）。
    events: Option<IMFMediaEventGenerator>,
    /// 输出样本由 MFT 分配。
    provides_samples: bool,
    /// 非提供样本模式下的输出缓冲大小。
    out_buffer_size: u32,
    /// 编码器序列头（SPS/PPS，Annex-B）。
    seq_header: Vec<u8>,
    /// 当前宽高与帧率。
    width: u32,
    height: u32,
    fps: u32,
    /// 输入行距。
    stride: u32,
    /// 强制关键帧标志。
    force_key: bool,
    /// 是否已收到 NeedInput 事件（异步 MFT 的流控状态）。
    expecting_input: bool,
    /// 编码器名称。
    name: String,
    /// 编码分段耗时累计（诊断打点）。
    enc_frames: u64,
    sum_build_us: u64,
    sum_wait_us: u64,
    sum_drain_us: u64,
    sum_process_us: u64,
    /// `build` 内部细分：样本/缓冲分配 vs 颜色转换 vs 其它。
    sum_alloc_us: u64,
    sum_convert_us: u64,
}

impl MfH264Session {
    /// 创建会话并完成类型协商。
    ///
    /// # Arguments
    /// * `activate` - 已枚举的 MFT 激活对象。
    /// * `cfg` - 编码配置（分辨率/帧率/码率/GOP/低延迟）。
    ///
    /// # Returns
    /// 已进入 streaming 状态的会话。
    ///
    /// # Errors
    /// 激活失败、类型协商失败、状态切换失败时返回 [`CodecError`]。
    pub fn new(activate: &IMFActivate, cfg: &EncoderConfig) -> Result<Self, CodecError> {
        ensure_mf()?;
        let name = activation_name(activate);
        let transform: IMFTransform = unsafe {
            activate
                .ActivateObject()
                .map_err(|e| CodecError::EncoderFailure(format!("ActivateObject({name}): {e}")))?
        };

        // 硬件 MFT 多为异步：必须先解锁，再设置类型。
        // 注意：AMD 等驱动不实现 IMFTransform 上的 IMFAttributes 查询（cast 会返回
        // E_NOINTERFACE），必须走 `GetAttributes()` 拿到属性存储，否则解锁静默失效，
        // 随后的 SetInputType 会返回 0xC00D6D77（MF_E_TRANSFORM_ASYNC_MFT_NOT_SUPPORTED）。
        let events: Option<IMFMediaEventGenerator> = transform.cast().ok();
        if events.is_some() {
            let attrs = unsafe { transform.GetAttributes() }.map_err(|e| {
                CodecError::EncoderFailure(format!("GetAttributes({name}): {e}"))
            })?;
            unsafe {
                attrs
                    .SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)
                    .map_err(|e| {
                        CodecError::EncoderFailure(format!("异步解锁失败({name}): {e}"))
                    })?;
                // 关闭异步 MFT 的内部帧缓存：不设该属性时驱动会多攒一帧再吐，
                // 1080p 下表现为「每次 push 额外等 ~8ms」，吞吐也被串行化到 ~46fps。
                // 失败不阻断：老驱动可能不认该属性，由上层实测值体现。
                if let Err(e) = attrs.SetUINT32(&MF_LOW_LATENCY, 1) {
                    eprintln!("[mf] MF_LOW_LATENCY 设置失败({name}): {e}");
                }
            }
        }

        let input = make_input_type(cfg)?;
        let mut output = make_output_type(cfg)?;
        unsafe {
            // 顺序很关键：AMD 硬件 MFT 要求先设置输出类型再设置输入类型，
            // 反序会在 SetInputType 返回 0xC00D6D60（当前流未设置有效类型）。
            if let Err(err) = transform.SetOutputType(0, &output, 0) {
                // High profile 不被支持时退回 Main。
                let main = make_output_type_main(cfg)?;
                transform.SetOutputType(0, &main, 0).map_err(|e| {
                    CodecError::EncoderFailure(format!(
                        "SetOutputType({name}) High 失败({err})，Main 也失败: {e}"
                    ))
                })?;
                output = main;
            }
            transform
                .SetInputType(0, &input, 0)
                .map_err(|e| CodecError::EncoderFailure(format!("SetInputType({name}): {e}")))?;
        }

        let info = unsafe {
            transform
                .GetOutputStreamInfo(0)
                .map_err(|e| CodecError::EncoderFailure(format!("GetOutputStreamInfo: {e}")))?
        };
        let provides_samples =
            (info.dwFlags as i32) & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 != 0;
        let out_buffer_size = info.cbSize;

        // 序列头可能要到首帧后才出现；先取一次，为空时由上层在编码后补齐。
        let seq_header = unsafe { read_sequence_header(&output) }.unwrap_or_default();
        let codec_api: Option<ICodecAPI> = transform.cast().ok();

        let mut session = Self {
            transform,
            codec_api,
            events,
            provides_samples,
            out_buffer_size,
            seq_header,
            width: cfg.width,
            height: cfg.height,
            fps: cfg.fps,
            stride: aligned_stride(cfg.width),
            force_key: false,
            expecting_input: false,
            name,
            enc_frames: 0,
            sum_build_us: 0,
            sum_wait_us: 0,
            sum_drain_us: 0,
            sum_process_us: 0,
            sum_alloc_us: 0,
            sum_convert_us: 0,
        };
        // 码控参数属于可选能力：不支持时不阻断会话，由上层报告「热更新不可用」。
        let _ = session.apply_runtime_params(cfg);
        unsafe {
            session
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .map_err(|e| CodecError::EncoderFailure(format!("BEGIN_STREAMING: {e}")))?;
            session
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .map_err(|e| CodecError::EncoderFailure(format!("START_OF_STREAM: {e}")))?;
        }
        Ok(session)
    }

    /// 编码器名称。
    ///
    /// # Returns
    /// 友好名称。
    ///
    /// # Errors
    /// 不会失败。
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 序列头（SPS/PPS）。
    ///
    /// # Returns
    /// Annex-B 序列头；编码器尚未提供时为空。
    ///
    /// # Errors
    /// 不会失败。
    pub fn sequence_header(&self) -> &[u8] {
        &self.seq_header
    }

    /// 读取并刷新序列头。
    ///
    /// 为什么需要刷新：硬件 MFT 往往在首帧编码后才把 SPS/PPS 写入输出类型的
    /// `MF_MT_MPEG_SEQUENCE_HEADER`，构造时读到的只是空值。
    ///
    /// # Returns
    /// 最新的 Annex-B 序列头；仍为空表示驱动不提供（调用方应从首个 IDR 中提取）。
    ///
    /// # Errors
    /// 不会失败；读取失败保留旧值。
    pub fn refresh_sequence_header(&mut self) -> &[u8] {
        if self.seq_header.is_empty() {
            let current = unsafe { self.transform.GetOutputCurrentType(0) };
            if let Ok(current) = current {
                if let Ok(header) = unsafe { read_sequence_header(&current) } {
                    if !header.is_empty() {
                        self.seq_header = header;
                    }
                }
            }
        }
        &self.seq_header
    }

    /// 当前生效帧率。
    ///
    /// # Returns
    /// 帧率（Hz）。
    ///
    /// # Errors
    /// 不会失败。
    pub fn fps(&self) -> u32 {
        self.fps
    }

    /// 请求下一帧为关键帧。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败；编码器不支持该参数时静默忽略。
    pub fn request_keyframe(&mut self) {
        self.force_key = true;
    }

    /// 把 RGBA8 帧提交给编码器并同步取回码流。
    ///
    /// # Arguments
    /// * `rgba` - RGBA8 像素（长度 `w*h*4`）。
    /// * `pts_100ns` - 采样时间（100ns）。
    /// * `duration_100ns` - 采样时长（100ns）。
    ///
    /// # Returns
    /// 本次调用产生的编码输出（通常 1 个；MFT 有缓存时可能 0~2 个）。
    ///
    /// # Errors
    /// 颜色转换、输入提交或输出取回失败时返回 [`CodecError::EncoderFailure`]。
    pub fn encode(
        &mut self,
        rgba: &[u8],
        pts_100ns: i64,
        duration_100ns: i64,
    ) -> Result<Vec<EncodedSample>, CodecError> {
        let expected = self.width as usize * self.height as usize * 4;
        if rgba.len() < expected {
            return Err(CodecError::InvalidConfig(format!(
                "RGBA 长度不足: {} < {expected}",
                rgba.len()
            )));
        }

        // 流水线关键：先把上一帧已就绪的输出取走（不阻塞等 GPU 编完），再提交本帧。
        // 这样每帧的成本只有「颜色转换 + 提交」，GPU 编码在两次 push 的间隔内并行完成。
        // 若在这里阻塞等 HaveOutput，1080p 下每帧会被 GPU 往返时延串行化到 20ms 量级。
        let mut outs = Vec::new();
        let drain_start = std::time::Instant::now();
        self.drain_ready(&mut outs)?;
        let drain_us = drain_start.elapsed().as_micros() as u64;
        // 异步 MFT 只有在 NeedInput 状态下才接收输入；正常情况 drain_ready 已置位。
        let wait_start = std::time::Instant::now();
        self.wait_for_input(&mut outs)?;
        let wait_us = wait_start.elapsed().as_micros() as u64;

        let build_start = std::time::Instant::now();
        let (sample, alloc_us, convert_us) =
            self.build_input_sample(rgba, pts_100ns, duration_100ns)?;
        let build_us = build_start.elapsed().as_micros() as u64;
        let process_start = std::time::Instant::now();
        unsafe {
            self.transform
                .ProcessInput(0, &sample, 0)
                .map_err(|e| CodecError::EncoderFailure(format!("ProcessInput: {e}")))?;
        }
        let process_us = process_start.elapsed().as_micros() as u64;
        self.expecting_input = false;
        self.enc_frames += 1;
        self.sum_build_us += build_us;
        self.sum_wait_us += wait_us;
        self.sum_drain_us += drain_us;
        self.sum_process_us += process_us;
        self.sum_alloc_us += alloc_us;
        self.sum_convert_us += convert_us;
        if self.enc_frames % 60 == 0 {
            let n = self.enc_frames;
            eprintln!(
                "[enc] frames={n} build_avg={}us(alloc={}us convert={}us) wait_avg={}us \
                 drain_avg={}us process_avg={}us",
                self.sum_build_us / n,
                self.sum_alloc_us / n,
                self.sum_convert_us / n,
                self.sum_wait_us / n,
                self.sum_drain_us / n,
                self.sum_process_us / n
            );
        }
        // 同步 MFT 没有事件队列，输出只能当场取；异步 MFT 的输出去下一次 encode 取。
        if self.events.is_none() {
            self.collect_outputs(&mut outs)?;
        }
        Ok(outs)
    }

    /// 冲刷编码器缓存。
    ///
    /// # Returns
    /// 缓存中剩余帧的编码输出。
    ///
    /// # Errors
    /// 取回失败返回 [`CodecError::EncoderFailure`]。
    pub fn drain(&mut self) -> Result<Vec<EncodedSample>, CodecError> {
        unsafe {
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_COMMAND_DRAIN, 0);
        }
        let mut outs = Vec::new();
        // drain 最多残留 1~2 帧；上限 8 防止异常 MFT 死循环。
        for _ in 0..8 {
            if self.events.is_some() {
                match self.wait_event()? {
                    EventKind::HaveOutput => {}
                    EventKind::NeedInput | EventKind::Other => break,
                }
            }
            match self.take_output_now()? {
                Some(s) => outs.push(s),
                None => break,
            }
        }
        unsafe {
            let _ = self
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
            let _ = self
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
        }
        Ok(outs)
    }

    /// 热更新码率与 GOP（不改分辨率/帧率）。
    ///
    /// # Arguments
    /// * `cfg` - 新配置。
    ///
    /// # Returns
    /// 参数已下发返回 `Ok(())`。
    ///
    /// # Errors
    /// 编码器不支持 ICodecAPI 或下发失败时返回 [`CodecError::InvalidConfig`] /
    /// [`CodecError::EncoderFailure`]。
    pub fn update_rate_control(&mut self, cfg: &EncoderConfig) -> Result<(), CodecError> {
        self.apply_runtime_params(cfg)
    }

    /// 重新协商输出类型上的帧率（分辨率不变时使用）。
    ///
    /// # Arguments
    /// * `fps` - 新帧率。
    ///
    /// # Returns
    /// 成功返回 `Ok(())`。
    ///
    /// # Errors
    /// MFT 拒绝新类型时返回 [`CodecError::InvalidConfig`]。
    pub fn update_fps(&mut self, fps: u32) -> Result<(), CodecError> {
        let mut cfg = EncoderConfig {
            codec: rdc_protocol::CodecKind::H264,
            width: self.width,
            height: self.height,
            fps: fps.max(1),
            target_bitrate_bps: 1,
            gop: 1,
            intra_refresh: false,
            low_latency: true,
            chroma_444: false,
            bit_depth: 8,
            temporal_layers: 1,
        };
        // 复用 make_output_type 的帧率字段；码率字段会被 SetOutputType 忽略（已在流中）。
        cfg.target_bitrate_bps = 1;
        let output = make_output_type(&cfg)?;
        unsafe {
            self.transform
                .SetOutputType(0, &output, 0)
                .map_err(|e| CodecError::InvalidConfig(format!("SetOutputType(fps): {e}")))?;
        }
        self.fps = cfg.fps;
        Ok(())
    }

    /// 下发码控参数（码率/码控模式/低延迟/B 帧/GOP）。
    fn apply_runtime_params(&mut self, cfg: &EncoderConfig) -> Result<(), CodecError> {
        if self.codec_api.is_none() {
            return Err(CodecError::InvalidConfig(
                "编码器不支持 ICodecAPI 运行期重配".to_owned(),
            ));
        }
        unsafe {
            // 码控模式实测：驱动默认 14.46 Mbps（目标 8，+80.8%）；CBR=1 与
            // PeakConstrainedVBR=2 都塌到 0.07 Mbps。AMDh264Encoder 对显式码控支持不完整，
            // 回落到驱动默认并如实记录 D19；峰值属性仍下发，若驱动认可可起约束作用。
            self.set_codec_value(
                &CODECAPI_AVEncCommonRateControlMode,
                VARIANT::from(RATE_CONTROL_DRIVER_DEFAULT),
            )?;
            let _ = self.set_codec_value(
                &CODECAPI_AVEncCommonMaxBitRate,
                VARIANT::from(cfg.target_bitrate_bps.saturating_mul(5) / 4),
            );
            self.set_codec_value(
                &CODECAPI_AVEncCommonMeanBitRate,
                VARIANT::from(cfg.target_bitrate_bps),
            )?;
            // CODECAPI 的低延迟是 VT_BOOL；此前用 VT_UI4 等于没生效（调研结论）。
            let low_latency = BoolVariant::new(cfg.low_latency);
            let low_ok = self
                .set_codec_value_ref(&CODECAPI_AVEncCommonLowLatency, low_latency.as_variant())
                .is_ok();
            let gop_ok = self
                .set_codec_value(&CODECAPI_AVEncMPVGOPSize, VARIANT::from(cfg.gop.max(1)))
                .is_ok();
            // 低延迟必须禁用 B 帧，否则硬件会缓存多帧，破坏时延目标。
            let b_frames = if cfg.low_latency { 0u32 } else { 2u32 };
            let b_ok = self
                .set_codec_value(
                    &CODECAPI_AVEncMPVDefaultBPictureCount,
                    VARIANT::from(b_frames),
                )
                .is_ok();
            // 参考帧压到 1：DPB 越小，解码端可复用的表面越少，稳态积压越低。
            let refs_ok = !cfg.low_latency
                || self
                    .set_codec_value(
                        &CODECAPI_AVEncVideoMaxNumRefFrame,
                        VARIANT::from(1u32),
                    )
                    .is_ok();
            // 质量/速度权衡：低延迟场景取「速度优先」，避免驱动默认的慢速搜索
            // 把 1080p 单帧编码拉到 16ms 以上（实测 push p50 恰好被卡在一个帧周期）。
            let speed_ok = self
                .set_codec_value(
                    &CODECAPI_AVEncCommonQualityVsSpeed,
                    VARIANT::from(if cfg.low_latency { 0u32 } else { 50u32 }),
                )
                .is_ok();
            eprintln!(
                "[enc] codecapi low_latency={low_ok} gop={gop_ok} bframes={b_frames}:{b_ok} max_ref1={refs_ok} speed_first={speed_ok}"
            );
        }
        Ok(())
    }

    /// 通过 ICodecAPI 设置一个值。
    unsafe fn set_codec_value(&self, key: &GUID, value: VARIANT) -> Result<(), CodecError> {
        self.set_codec_value_ref(key, &value)
    }

    /// 通过 ICodecAPI 设置一个引用形式的 VARIANT（VT_BOOL 场景）。
    unsafe fn set_codec_value_ref(&self, key: &GUID, value: &VARIANT) -> Result<(), CodecError> {
        let Some(api) = self.codec_api.as_ref() else {
            return Err(CodecError::InvalidConfig(
                "编码器不支持 ICodecAPI".to_owned(),
            ));
        };
        api.SetValue(key, value)
            .map_err(|e| CodecError::EncoderFailure(format!("ICodecAPI::SetValue: {e}")))
    }

    /// 构造 NV12 输入样本（直接写入 MFT 缓冲）。
    ///
    /// # Returns
    /// 样本，以及内部两段耗时（分配 / 颜色转换），用于分段打点。
    fn build_input_sample(
        &mut self,
        rgba: &[u8],
        pts_100ns: i64,
        duration_100ns: i64,
    ) -> Result<(IMFSample, u64, u64), CodecError> {
        let stride = self.stride as usize;
        let buffer_size = stride * self.height as usize * 3 / 2;
        let alloc_start = std::time::Instant::now();
        let sample = unsafe { MFCreateSample() }
            .map_err(|e| CodecError::EncoderFailure(format!("MFCreateSample: {e}")))?;
        let buffer = unsafe { MFCreateMemoryBuffer(buffer_size as u32) }
            .map_err(|e| CodecError::EncoderFailure(format!("MFCreateMemoryBuffer: {e}")))?;
        let alloc_us = alloc_start.elapsed().as_micros() as u64;

        // 直接写进 MF 缓冲，避免「先转 NV12 Vec 再拷进 sample」的二次拷贝。
        let convert_start = std::time::Instant::now();
        let convert = unsafe {
            let mut data: *mut u8 = std::ptr::null_mut();
            let mut max_len = 0u32;
            let mut cur_len = 0u32;
            buffer
                .Lock(&mut data, Some(&mut max_len), Some(&mut cur_len))
                .map_err(|e| CodecError::EncoderFailure(format!("IMFMediaBuffer::Lock: {e}")))?;
            let dst = std::slice::from_raw_parts_mut(data, max_len as usize);
            let convert = crate::nv12::rgba_to_nv12(rgba, self.width, self.height, dst);
            let _ = buffer.Unlock();
            convert
        };
        let convert_us = convert_start.elapsed().as_micros() as u64;
        convert?;
        unsafe {
            buffer
                .SetCurrentLength(buffer_size as u32)
                .map_err(|e| CodecError::EncoderFailure(format!("SetCurrentLength: {e}")))?;
            sample
                .AddBuffer(&buffer)
                .map_err(|e| CodecError::EncoderFailure(format!("AddBuffer: {e}")))?;
            sample
                .SetSampleTime(pts_100ns)
                .map_err(|e| CodecError::EncoderFailure(format!("SetSampleTime: {e}")))?;
            sample
                .SetSampleDuration(duration_100ns)
                .map_err(|e| CodecError::EncoderFailure(format!("SetSampleDuration: {e}")))?;
            if self.force_key {
                // 不支持 ForceKeyFrame 的编码器忽略错误，由 GOP 兜底。
                let _ = self.set_codec_value(&CODECAPI_AVEncVideoForceKeyFrame, VARIANT::from(1u32));
                // 只对本帧生效：不清标志会让后续每一帧都请求 IDR，
                // 实测会把单帧编码成本抬到 45ms 量级并让码率偏离目标（D18/D19 根因）。
                self.force_key = false;
            }
        }
        Ok((sample, alloc_us, convert_us))
    }

    /// 非阻塞排空已就绪的 MFT 事件与输出（流水线关键：不在这里等 GPU）。
    ///
    /// # Arguments
    /// * `outs` - 收集到的输出追加到这里。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 取输出失败返回 [`CodecError::EncoderFailure`]；无事件可取的正常情况不算错误。
    fn drain_ready(&mut self, outs: &mut Vec<EncodedSample>) -> Result<(), CodecError> {
        // 先克隆事件生成器句柄（COM +1 引用），避免 `&self.events` 与 `&mut self` 借用冲突。
        let Some(events) = self.events.clone() else {
            return Ok(());
        };
        // 上限 8 轮：一次排空最多遇到「HaveOutput → NeedInput」各若干次，防异常 MFT 死循环。
        for _ in 0..8 {
            let event = match unsafe { events.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
                Ok(event) => event,
                // 队列空是正常状态，不是错误。
                Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => break,
                Err(e) => {
                    return Err(CodecError::EncoderFailure(format!("GetEvent(NO_WAIT): {e}")));
                }
            };
            let kind = unsafe { event.GetType() }.unwrap_or(0);
            match kind as i32 {
                // 取走输出；取不到（NEED_MORE_INPUT）说明还没编完，留给下一次。
                k if k == METransformHaveOutput.0 => {
                    if let Some(sample) = self.take_output_now()? {
                        outs.push(sample);
                    }
                }
                k if k == METransformNeedInput.0 => self.expecting_input = true,
                _ => {}
            }
        }
        Ok(())
    }

    /// 异步 MFT：等到 NeedInput 事件；期间若有输出则先取走。
    fn wait_for_input(&mut self, outs: &mut Vec<EncodedSample>) -> Result<(), CodecError> {
        if self.events.is_none() || self.expecting_input {
            return Ok(());
        }
        // 上限 8 轮：编码器在重配/首帧时可能连续发多个事件。
        for _ in 0..8 {
            match self.wait_event()? {
                EventKind::NeedInput => {
                    self.expecting_input = true;
                    return Ok(());
                }
                EventKind::HaveOutput => {
                    if let Some(s) = self.take_output_now()? {
                        outs.push(s);
                    }
                }
                EventKind::Other => {
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    /// 输入后收集输出：异步等待 HaveOutput，同步直接轮询。
    fn collect_outputs(&mut self, outs: &mut Vec<EncodedSample>) -> Result<(), CodecError> {
        if self.events.is_none() {
            // 同步 MFT：ProcessOutput 直到 NEED_MORE_INPUT。
            for _ in 0..4 {
                match self.take_output_now()? {
                    Some(s) => outs.push(s),
                    None => break,
                }
            }
            return Ok(());
        }
        for _ in 0..4 {
            match self.wait_event()? {
                EventKind::HaveOutput => {
                    match self.take_output_now()? {
                        Some(s) => outs.push(s),
                        None => break,
                    }
                }
                EventKind::NeedInput => {
                    self.expecting_input = true;
                    break;
                }
                EventKind::Other => {
                    break;
                }
            }
        }
        Ok(())
    }

    /// 直接调用 ProcessOutput（不等待事件）。
    ///
    /// # Returns
    /// 有输出返回样本；`NEED_MORE_INPUT` 返回 `None`。
    ///
    /// # Errors
    /// 其它 HRESULT 返回 [`CodecError::EncoderFailure`]。
    fn take_output_now(&mut self) -> Result<Option<EncodedSample>, CodecError> {
        let mut provided: Option<IMFSample> = None;
        if !self.provides_samples {
            let size = self.out_buffer_size.max(1);
            let buffer = unsafe { MFCreateMemoryBuffer(size) }.map_err(|e| {
                CodecError::EncoderFailure(format!("MFCreateMemoryBuffer(out): {e}"))
            })?;
            let sample = unsafe { MFCreateSample() }
                .map_err(|e| CodecError::EncoderFailure(format!("MFCreateSample(out): {e}")))?;
            unsafe {
                sample
                    .AddBuffer(&buffer)
                    .map_err(|e| CodecError::EncoderFailure(format!("AddBuffer(out): {e}")))?;
            }
            provided = Some(sample);
        }
        let mut out = MFT_OUTPUT_DATA_BUFFER {
            dwStreamID: 0,
            pSample: ManuallyDrop::new(provided),
            dwStatus: 0,
            pEvents: ManuallyDrop::new(None),
        };
        let mut status = 0u32;
        // ProcessOutput 的签名是 `(flags, &mut [buffer], *mut u32)`：单流编码只传一个输出缓冲。
        let result = unsafe {
            self.transform
                .ProcessOutput(0, std::slice::from_mut(&mut out), &mut status)
        };
        // 以下操作全部围绕 COM 输出的原始所有权，必须在 unsafe 中完成。
        unsafe {
            match result {
                Ok(()) => {}
                Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => {
                    let _ = ManuallyDrop::take(&mut out.pSample);
                    return Ok(None);
                }
                Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                    let _ = ManuallyDrop::take(&mut out.pSample);
                    return Err(CodecError::EncoderFailure(
                        "编码器输出格式变化，需重建会话".to_owned(),
                    ));
                }
                Err(e) => {
                    let _ = ManuallyDrop::take(&mut out.pSample);
                    return Err(CodecError::EncoderFailure(format!("ProcessOutput: {e}")));
                }
            }
            let sample = ManuallyDrop::take(&mut out.pSample);
            let _ = ManuallyDrop::take(&mut out.pEvents);
            let Some(sample) = sample else {
                return Ok(None);
            };
            Ok(Some(extract_sample(&sample)?))
        }
    }

    /// 阻塞等待一个 MFT 事件并归类。
    fn wait_event(&mut self) -> Result<EventKind, CodecError> {
        let Some(events) = self.events.as_ref() else {
            return Ok(EventKind::Other);
        };
        // 同步阻塞：编码线程独占，MFT 保证输入后必产生 NeedInput/HaveOutput 之一。
        let event = unsafe {
            match events.GetEvent(MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS(0)) {
                Ok(e) => e,
                Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => return Ok(EventKind::Other),
                Err(e) => return Err(CodecError::EncoderFailure(format!("GetEvent: {e}"))),
            }
        };
        let kind = unsafe { event.GetType() }.unwrap_or(0);
        // METransformNeedInput = 601，METransformHaveOutput = 602（mfapi.h 中的 MF_EVENT_TYPE）。
        Ok(match kind as i32 {
            k if k == METransformNeedInput.0 => EventKind::NeedInput,
            k if k == METransformHaveOutput.0 => EventKind::HaveOutput,
            _ => EventKind::Other,
        })
    }
}

impl Drop for MfH264Session {
    /// 结束流并释放 MFT（COM 引用计数减一）。
    fn drop(&mut self) {
        unsafe {
            let _ = self
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
        }
    }
}

// =====** 内部工具 =====**

/// MFT 事件归类。
enum EventKind {
    /// 需要新的输入。
    NeedInput,
    /// 有输出可取。
    HaveOutput,
    /// 其它事件（格式变化、drain 完成等）。
    Other,
}

/// 构造 NV12 输入媒体类型。
pub(crate) fn make_input_type(cfg: &EncoderConfig) -> Result<IMFMediaType, CodecError> {
    let media_type = unsafe { MFCreateMediaType() }
        .map_err(|e| CodecError::EncoderFailure(format!("MFCreateMediaType(in): {e}")))?;
    unsafe {
        set_guid(&media_type, &MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        set_guid(&media_type, &MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
        media_type
            .SetUINT64(&MF_MT_FRAME_SIZE, pack_u64(cfg.width, cfg.height))
            .map_err(invalid)?;
        media_type
            .SetUINT64(&MF_MT_FRAME_RATE, pack_u64(cfg.fps.max(1), 1))
            .map_err(invalid)?;
        media_type
            .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack_u64(1, 1))
            .map_err(invalid)?;
        media_type
            .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
            .map_err(invalid)?;
        // 不设置 MF_MT_YUV_MATRIX / MF_MT_DEFAULT_STRIDE：AMD 等硬件编码器会直接拒绝
        // 带这两个属性的输入类型（0xC00D6D60）。颜色矩阵改由 ICodecAPI 下发，
        // 行距由样本缓冲的连续布局决定（本实现按 width 紧密排列）。
    }
    Ok(media_type)
}

/// 构造 H.264 输出媒体类型（High profile）。
pub(crate) fn make_output_type(cfg: &EncoderConfig) -> Result<IMFMediaType, CodecError> {
    let media_type = unsafe { MFCreateMediaType() }
        .map_err(|e| CodecError::EncoderFailure(format!("MFCreateMediaType(out): {e}")))?;
    unsafe {
        set_guid(&media_type, &MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        set_guid(&media_type, &MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
        media_type
            .SetUINT32(&MF_MT_AVG_BITRATE, cfg.target_bitrate_bps.max(1))
            .map_err(invalid)?;
        media_type
            .SetUINT64(&MF_MT_FRAME_SIZE, pack_u64(cfg.width, cfg.height))
            .map_err(invalid)?;
        media_type
            .SetUINT64(&MF_MT_FRAME_RATE, pack_u64(cfg.fps.max(1), 1))
            .map_err(invalid)?;
        media_type
            .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
            .map_err(invalid)?;
        media_type
            .SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_High.0 as u32)
            .map_err(invalid)?;
    }
    Ok(media_type)
}

/// 构造 Main profile 输出类型（High 不支持时兜底）。
fn make_output_type_main(cfg: &EncoderConfig) -> Result<IMFMediaType, CodecError> {
    let media_type = make_output_type(cfg)?;
    unsafe {
        media_type
            .SetUINT32(&MF_MT_MPEG2_PROFILE, eAVEncH264VProfile_Main.0 as u32)
            .map_err(invalid)?;
    }
    Ok(media_type)
}

/// 读取输出类型上的序列头。
///
/// # Returns
/// 序列头字节；未设置时为空。
///
/// # Errors
/// 属性读取失败返回 [`CodecError::EncoderFailure`]。
unsafe fn read_sequence_header(output: &IMFMediaType) -> Result<Vec<u8>, CodecError> {
    let size = output
        .GetBlobSize(&MF_MT_MPEG_SEQUENCE_HEADER)
        .map_err(|e| CodecError::EncoderFailure(format!("GetBlobSize(seq): {e}")))?;
    if size == 0 {
        return Ok(Vec::new());
    }
    let mut buf = vec![0u8; size as usize];
    output
        .GetBlob(&MF_MT_MPEG_SEQUENCE_HEADER, &mut buf, None)
        .map_err(|e| CodecError::EncoderFailure(format!("GetBlob(seq): {e}")))?;
    Ok(buf)
}

/// 从输出样本提取码流。
unsafe fn extract_sample(sample: &IMFSample) -> Result<EncodedSample, CodecError> {
    let pts = sample.GetSampleTime().unwrap_or(0);
    let keyframe = sample
        .GetUINT32(&MFSampleExtension_CleanPoint)
        .map(|v| v != 0)
        .unwrap_or(false);
    let buffer = sample
        .ConvertToContiguousBuffer()
        .map_err(|e| CodecError::EncoderFailure(format!("ConvertToContiguousBuffer: {e}")))?;
    let mut ptr: *mut u8 = std::ptr::null_mut();
    let mut max_len = 0u32;
    let mut cur_len = 0u32;
    buffer
        .Lock(&mut ptr, Some(&mut max_len), Some(&mut cur_len))
        .map_err(|e| CodecError::EncoderFailure(format!("Lock: {e}")))?;
    let data = std::slice::from_raw_parts(ptr, cur_len as usize).to_vec();
    let _ = buffer.Unlock();
    Ok(EncodedSample {
        data,
        keyframe,
        pts_100ns: pts,
    })
}

/// NV12 行距按 32 字节对齐（主流硬件编码器的隐式要求）。
fn aligned_stride(width: u32) -> u32 {
    width.div_ceil(32) * 32
}

/// 把宽高打包成 `(hi, lo)` 的 u64（Media Foundation 的 size 属性格式）。
fn pack_u64(hi: u32, lo: u32) -> u64 {
    (u64::from(hi) << 32) | u64::from(lo)
}

/// 设置 GUID 属性。
unsafe fn set_guid(media_type: &IMFMediaType, key: &GUID, value: &GUID) -> Result<(), CodecError> {
    media_type
        .SetGUID(key, value)
        .map_err(|e| CodecError::EncoderFailure(format!("SetGUID: {e}")))
}

/// 统一的媒体类型配置错误。
fn invalid(e: windows::core::Error) -> CodecError {
    CodecError::InvalidConfig(format!("媒体类型属性设置失败: {e}"))
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 宽高打包必须符合 MF 的 (high<<32|low) 约定。
    #[test]
    fn pack_size_layout() {
        assert_eq!(pack_u64(1920, 1080), 0x0000_0780_0000_0438);
    }

    /// NV12 stride 必须按 32 字节对齐。
    #[test]
    fn stride_alignment() {
        assert_eq!(aligned_stride(1920), 1920);
        assert_eq!(aligned_stride(1366), 1376);
        assert_eq!(aligned_stride(100), 128);
    }
}
