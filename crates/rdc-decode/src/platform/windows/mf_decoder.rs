//! rdc-decode / Windows Media Foundation H.264 解码器
//!
//! 层级：L3 编解码层 · 平台适配最内层
//! 用途：枚举硬件 H.264 解码 MFT（失败时显式回退到软件解码并报告），把 Annex-B 码流
//!       解成 NV12 帧，供上屏模块上传到 D3D11 纹理。
//! 调用方：rdc-decode::platform::windows 的 `create_decoder`、rdc-cli 的 S3b 实测工具
//!
//! 安全边界（unsafe）：
//! - 所有 COM 调用都在本文件内；[`MfH264Decoder`] 独占一个 MFT，只允许单一解码线程使用。
//! - `Lock` 指针只在「Lock → 拷贝 → Unlock」区间内有效，不跨调用保存。
//! - `MFTEnumEx` 返回的激活对象数组由 COM 分配器分配，复制后必须 `CoTaskMemFree`。

use std::mem::ManuallyDrop;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use windows::core::{GUID, Interface};
use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFMediaEventGenerator, IMFMediaType, IMFSample, IMFTransform, MFCreateMediaType,
    MFCreateMemoryBuffer, MFCreateSample, MFMediaType_Video, MFTEnumEx, MFVideoFormat_H264,
    MFVideoFormat_NV12, MFVideoInterlace_Progressive, MFSTARTUP_LITE, MFStartup,
    MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_INTERLACE_MODE, MF_MT_MAJOR_TYPE,
    MF_MT_PIXEL_ASPECT_RATIO, MF_MT_SUBTYPE, MF_TRANSFORM_ASYNC_UNLOCK,
    MFT_CATEGORY_VIDEO_DECODER, MFT_ENUM_FLAG, MFT_ENUM_FLAG_ALL, MFT_ENUM_FLAG_HARDWARE,
    MFT_ENUM_FLAG_SORTANDFILTER, MFT_MESSAGE_NOTIFY_BEGIN_STREAMING,
    MFT_MESSAGE_NOTIFY_END_OF_STREAM, MFT_MESSAGE_NOTIFY_START_OF_STREAM,
    MFT_MESSAGE_COMMAND_FLUSH, MFT_OUTPUT_DATA_BUFFER, MFT_OUTPUT_STREAM_PROVIDES_SAMPLES,
    MFT_REGISTER_TYPE_INFO, MF_E_NO_EVENTS_AVAILABLE, MF_E_TRANSFORM_NEED_MORE_INPUT,
    MF_E_TRANSFORM_STREAM_CHANGE, MF_EVENT_FLAG_NO_WAIT, MF_LOW_LATENCY,
    MF_SA_D3D11_AWARE, MF_VERSION, METransformHaveOutput, METransformNeedInput,
    MFT_FRIENDLY_NAME_Attribute,
};
use windows::Win32::System::Com::CoTaskMemFree;

use crate::error::DecodeError;

// =====** MF 初始化 =====**

static MF_INIT: OnceLock<Result<(), String>> = OnceLock::new();

/// 确保 Media Foundation 已初始化（进程内只执行一次）。
///
/// # Errors
/// `MFStartup` 失败时返回 [`DecodeError::DecoderUnavailable`]。
pub fn ensure_mf() -> Result<(), DecodeError> {
    match MF_INIT.get_or_init(|| unsafe {
        MFStartup(MF_VERSION, MFSTARTUP_LITE).map_err(|e| format!("MFStartup: {e}"))
    }) {
        Ok(()) => Ok(()),
        Err(msg) => Err(DecodeError::DecoderUnavailable(format!(
            "Media Foundation 初始化失败: {msg}"
        ))),
    }
}

// =====** 枚举 =====**

/// 枚举 H.264 解码 MFT（H.264 → NV12）。
///
/// # Arguments
/// * `hardware_only` - 只返回硬件 MFT；`false` 时返回全部（含软件同步 MFT）
///
/// # Returns
/// 激活对象列表；无匹配时为空。
///
/// # Errors
/// `MFTEnumEx` 失败时返回 [`DecodeError::DecoderUnavailable`]。
pub fn enumerate(hardware_only: bool) -> Result<Vec<IMFActivate>, DecodeError> {
    ensure_mf()?;
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_H264,
    };
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_NV12,
    };
    let flags: MFT_ENUM_FLAG = if hardware_only {
        MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER
    } else {
        MFT_ENUM_FLAG_ALL
    };
    let mut raw: *mut Option<IMFActivate> = std::ptr::null_mut();
    let mut count = 0u32;
    unsafe {
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_DECODER,
            flags,
            Some(&input),
            Some(&output),
            &mut raw,
            &mut count,
        )
        .map_err(|e| DecodeError::DecoderUnavailable(format!("MFTEnumEx(decoder): {e}")))?;
        if raw.is_null() || count == 0 {
            return Ok(Vec::new());
        }
        let slice = std::slice::from_raw_parts(raw, count as usize);
        let mut out = Vec::with_capacity(slice.len());
        for activate in slice.iter().flatten() {
            out.push(activate.clone());
        }
        CoTaskMemFree(Some(raw as *const core::ffi::c_void));
        Ok(out)
    }
}

/// 读取 MFT 友好名称。
pub fn activation_name(activate: &IMFActivate) -> String {
    let mut buf = [0u16; 128];
    let mut len = 0u32;
    unsafe {
        if activate
            .GetString(&MFT_FRIENDLY_NAME_Attribute, &mut buf, Some(&mut len))
            .is_ok()
        {
            return String::from_utf16_lossy(&buf[..len as usize]);
        }
    }
    "MF H.264 decoder".to_owned()
}

/// 列出可用解码器名称，供实测报告标注实际路径。
///
/// # Returns
/// `(硬件名称列表, 全部名称列表)`；枚举失败时两者均为空。
pub fn decoder_names() -> (Vec<String>, Vec<String>) {
    let hardware = enumerate(true).map(|list| list.iter().map(activation_name).collect());
    let all = enumerate(false).map(|list| list.iter().map(activation_name).collect());
    (hardware.unwrap_or_default(), all.unwrap_or_default())
}

// =====** 解码帧 =====**

/// 一帧解码结果（NV12，系统内存）。
#[derive(Debug, Clone)]
pub struct DecodedFrame {
    /// 输入时间码（微秒），取自样本时间。
    pub ts_us: u64,
    /// 宽（像素）。
    pub width: u32,
    /// 高（像素）。
    pub height: u32,
    /// Y 平面行距（字节）；本实现按 `width` 紧密排列。
    pub stride: u32,
    /// NV12 数据：`Y` 平面 + 交错的 `UV` 平面。
    pub data: Vec<u8>,
}

/// MFT 事件类别。
enum EventKind {
    /// 需要输入。
    NeedInput,
    /// 有输出可取。
    HaveOutput,
    /// 其它事件（属性变更等），忽略。
    Other,
}

// =====** MfH264Decoder =====**

/// 独占一个 H.264 解码 MFT 的会话。
pub struct MfH264Decoder {
    /// 解码 MFT。
    transform: IMFTransform,
    /// 事件生成器；同步 MFT 为 `None`。
    events: Option<IMFMediaEventGenerator>,
    /// 输出由 MFT 提供样本时为 `true`，否则由调用方预分配缓冲。
    provides_samples: bool,
    /// 输出缓冲大小下限（字节）。
    out_buffer_size: u32,
    /// 是否正在等待输入事件。
    expecting_input: bool,
    /// 输出尺寸。
    width: u32,
    /// 输出高度。
    height: u32,
    /// 已知的帧时长（100ns）。
    duration_100ns: u64,
    /// 最近一次送包的时间码（100ns），输出样本缺失时间时兜底。
    last_input_ts_100ns: u64,
    /// MFT 名称。
    name: String,
    /// 实际走的是否为硬件解码。
    hardware: bool,
    /// MFT 是否声明 DXVA 能力（诊断用；接线情况见决策待办 D28）。
    dxva_aware: bool,
}

impl MfH264Decoder {
    /// 打开解码器。
    ///
    /// # Arguments
    /// * `width` - 期望输出宽（像素）
    /// * `height` - 期望输出高（像素）
    /// * `fps` - 期望帧率，用于设置输出类型
    /// * `prefer_hardware` - 优先使用硬件解码；硬件不可用时退回软件并在 `hardware=false` 上体现
    ///
    /// # Returns
    /// 已配置好输入/输出类型、并进入流式状态的解码会话。
    ///
    /// # Errors
    /// 枚举为空、所有候选都配置失败、或消息发送失败时返回
    /// [`DecodeError::DecoderUnavailable`]。
    pub fn open(
        width: u32,
        height: u32,
        fps: u32,
        prefer_hardware: bool,
    ) -> Result<Self, DecodeError> {
        ensure_mf()?;
        if width == 0 || height == 0 {
            return Err(DecodeError::InvalidArgument("解码尺寸为 0".to_owned()));
        }
        let mut candidates: Vec<(IMFActivate, bool)> = Vec::new();
        if prefer_hardware {
            for activate in enumerate(true)? {
                candidates.push((activate, true));
            }
        }
        for activate in enumerate(false)? {
            candidates.push((activate, false));
        }
        if candidates.is_empty() {
            return Err(DecodeError::DecoderUnavailable("没有可用的 H.264 解码 MFT".to_owned()));
        }

        let mut last_error = String::new();
        for (activate, hardware) in candidates {
            let name = activation_name(&activate);
            match Self::configure(&activate, width, height, fps, hardware, name.clone()) {
                Ok(decoder) => return Ok(decoder),
                Err(error) => {
                    last_error = format!("{name}: {error}");
                }
            }
        }
        Err(DecodeError::DecoderUnavailable(format!(
            "没有可配置成功的 H.264 解码器（最后错误 {last_error}）"
        )))
    }

    /// 配置单个候选 MFT。
    fn configure(
        activate: &IMFActivate,
        width: u32,
        height: u32,
        fps: u32,
        hardware: bool,
        name: String,
    ) -> Result<Self, DecodeError> {
        let transform: IMFTransform = unsafe {
            activate
                .ActivateObject()
                .map_err(|e| DecodeError::DecoderUnavailable(format!("ActivateObject({name}): {e}")))?
        };
        let events: Option<IMFMediaEventGenerator> = transform.cast().ok();
        if events.is_some() {
            // 异步 MFT 必须先解锁，否则设置类型返回 MF_E_TRANSFORM_ASYNC_MFT_NOT_SUPPORTED。
            let attrs = unsafe { transform.GetAttributes() }
                .map_err(|e| DecodeError::DecoderUnavailable(format!("GetAttributes({name}): {e}")))?;
            unsafe {
                attrs
                    .SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)
                    .map_err(|e| DecodeError::DecoderUnavailable(format!("异步解锁({name}): {e}")))?;
                // 低时延对解码同样有意义；老驱动不认该属性时不阻断。
                let _ = attrs.SetUINT32(&MF_LOW_LATENCY, 1);
            }
        }

        let output = make_output_type(width, height, fps)?;
        let input = make_input_type()?;
        // 该 MFT 是否声明 DXVA 能力（MF_SA_D3D11_AWARE=1 表示可以接 D3D11 设备管理器做硬解）。
        // 声明能力不等于已经接线：本步骤只走系统内存输出，接线情况如实记录在缺口里。
        let dxva_aware = unsafe {
            transform
                .GetAttributes()
                .ok()
                .and_then(|attrs| attrs.GetUINT32(&MF_SA_D3D11_AWARE).ok())
                .map(|value| value != 0)
                .unwrap_or(false)
        };
        unsafe {
            // 解码器与编码器相反：必须先设置输入类型（H.264），再设置输出类型，
            // 否则返回 MF_E_TRANSFORM_TYPE_NOT_SET（0xC00D6D60）。
            transform
                .SetInputType(0, &input, 0)
                .map_err(|e| DecodeError::DecoderUnavailable(format!("SetInputType({name}): {e}")))?;
            transform
                .SetOutputType(0, &output, 0)
                .map_err(|e| DecodeError::DecoderUnavailable(format!("SetOutputType({name}): {e}")))?;
        }
        let info = unsafe {
            transform
                .GetOutputStreamInfo(0)
                .map_err(|e| {
                    DecodeError::DecoderUnavailable(format!("GetOutputStreamInfo({name}): {e}"))
                })?
        };
        let provides_samples = (info.dwFlags as i32) & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 != 0;
        let frame_len = width.saturating_mul(height).saturating_mul(3) / 2;

        let decoder = Self {
            transform,
            events,
            provides_samples,
            out_buffer_size: info.cbSize.max(frame_len),
            expecting_input: true,
            width,
            height,
            duration_100ns: 10_000_000 / u64::from(fps.max(1)),
            last_input_ts_100ns: 0,
            name,
            hardware,
            dxva_aware,
        };
        unsafe {
            decoder
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .map_err(|e| DecodeError::DecodeFailure(format!("BEGIN_STREAMING: {e}")))?;
            decoder
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .map_err(|e| DecodeError::DecodeFailure(format!("START_OF_STREAM: {e}")))?;
        }
        Ok(decoder)
    }

    /// 解码器名称。
    pub fn name(&self) -> &str {
        &self.name
    }

    /// 是否走硬件解码。
    pub fn is_hardware(&self) -> bool {
        self.hardware
    }

    /// MFT 是否声明 DXVA 能力（未接线时仅作诊断信息）。
    pub fn dxva_aware(&self) -> bool {
        self.dxva_aware
    }

    /// 送入一个 Annex-B 访问单元并取回已解出的帧。
    ///
    /// # Arguments
    /// * `annexb` - Annex-B 码流（关键帧需自带 SPS/PPS，否则解码器无法解析）
    /// * `ts_us` - 该帧的采集时间（微秒）
    ///
    /// # Returns
    /// 本次调用能取到的解码帧（可能为 0 或 1 帧；异步 MFT 常有 1 帧流水线延迟）。
    ///
    /// # Errors
    /// 等待事件超时、`ProcessInput`/`ProcessOutput` 失败时返回
    /// [`DecodeError::DecodeFailure`]。
    pub fn feed(&mut self, annexb: &[u8], ts_us: u64) -> Result<Vec<DecodedFrame>, DecodeError> {
        if annexb.is_empty() {
            return Err(DecodeError::InvalidArgument("码流为空".to_owned()));
        }
        let mut outs = Vec::new();
        self.drain_ready(&mut outs)?;
        self.wait_for_input(&mut outs, Duration::from_millis(300))?;

        let sample = self.build_input_sample(annexb, ts_us)?;
        unsafe {
            self.transform
                .ProcessInput(0, &sample, 0)
                .map_err(|e| DecodeError::DecodeFailure(format!("ProcessInput({}): {e}", self.name)))?;
        }
        self.expecting_input = false;
        self.last_input_ts_100ns = ts_us.saturating_mul(10);

        // 异步 MFT 的输出未必立刻可用；取不到时留到下一次 drain。
        if self.events.is_none() {
            self.collect_until_idle(&mut outs)?;
        }
        Ok(outs)
    }

    /// 冲刷解码器：发送 END_OF_STREAM 并尽力取回剩余帧。
    ///
    /// # Returns
    /// 冲刷得到的剩余帧。
    pub fn flush(&mut self) -> Result<Vec<DecodedFrame>, DecodeError> {
        let mut outs = Vec::new();
        unsafe {
            self.transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0)
                .map_err(|e| DecodeError::DecodeFailure(format!("END_OF_STREAM: {e}")))?;
        }
        for _ in 0..4 {
            match self.take_output_now()? {
                Some(frame) => outs.push(frame),
                None => break,
            }
        }
        Ok(outs)
    }

    /// 非阻塞排空事件队列与已就绪输出。
    fn drain_ready(&mut self, outs: &mut Vec<DecodedFrame>) -> Result<(), DecodeError> {
        let Some(events) = self.events.clone() else {
            return Ok(());
        };
        for _ in 0..8 {
            let event = match unsafe { events.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
                Ok(event) => event,
                Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => break,
                Err(e) => return Err(DecodeError::DecodeFailure(format!("GetEvent(NO_WAIT): {e}"))),
            };
            match unsafe { event.GetType() }.unwrap_or(0) as i32 {
                k if k == METransformHaveOutput.0 => {
                    if let Some(frame) = self.take_output_now()? {
                        outs.push(frame);
                    }
                }
                k if k == METransformNeedInput.0 => self.expecting_input = true,
                _ => {}
            }
        }
        Ok(())
    }

    /// 等到 NeedInput；期间若有输出则先取走。
    fn wait_for_input(
        &mut self,
        outs: &mut Vec<DecodedFrame>,
        timeout: Duration,
    ) -> Result<(), DecodeError> {
        if self.events.is_none() || self.expecting_input {
            return Ok(());
        }
        let deadline = Instant::now() + timeout;
        loop {
            match self.wait_event(deadline)? {
                Some(EventKind::NeedInput) => {
                    self.expecting_input = true;
                    return Ok(());
                }
                Some(EventKind::HaveOutput) => {
                    if let Some(frame) = self.take_output_now()? {
                        outs.push(frame);
                    }
                }
                Some(EventKind::Other) | None => {
                    // 超时也继续送包：MFT 可能不再发 NeedInput，硬等会让解码线程永久阻塞。
                    return Ok(());
                }
            }
        }
    }

    /// 同步 MFT：反复取输出直到无更多。
    fn collect_until_idle(&mut self, outs: &mut Vec<DecodedFrame>) -> Result<(), DecodeError> {
        for _ in 0..4 {
            match self.take_output_now()? {
                Some(frame) => outs.push(frame),
                None => break,
            }
        }
        Ok(())
    }

    /// 直接调用 ProcessOutput。
    fn take_output_now(&mut self) -> Result<Option<DecodedFrame>, DecodeError> {
        for _attempt in 0..2 {
            let mut provided: Option<IMFSample> = None;
            if !self.provides_samples {
                let buffer = unsafe { MFCreateMemoryBuffer(self.out_buffer_size.max(1)) }
                    .map_err(|e| DecodeError::DecodeFailure(format!("MFCreateMemoryBuffer(out): {e}")))?;
                let sample = unsafe { MFCreateSample() }
                    .map_err(|e| DecodeError::DecodeFailure(format!("MFCreateSample(out): {e}")))?;
                unsafe {
                    sample
                        .AddBuffer(&buffer)
                        .map_err(|e| DecodeError::DecodeFailure(format!("AddBuffer(out): {e}")))?;
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
            let result = unsafe {
                self.transform
                    .ProcessOutput(0, std::slice::from_mut(&mut out), &mut status)
            };
            unsafe {
                match result {
                    Ok(()) => {}
                    Err(e) if e.code() == MF_E_TRANSFORM_NEED_MORE_INPUT => {
                        let _ = ManuallyDrop::take(&mut out.pSample);
                        return Ok(None);
                    }
                    Err(e) if e.code() == MF_E_TRANSFORM_STREAM_CHANGE => {
                        let _ = ManuallyDrop::take(&mut out.pSample);
                        // 解码器在首帧后要求重设输出类型（分辨率/格式确认），重设后重试一次。
                        let available = self.transform.GetOutputAvailableType(0, 0).map_err(|e| {
                            DecodeError::DecodeFailure(format!("GetOutputAvailableType: {e}"))
                        })?;
                        self.transform.SetOutputType(0, &available, 0).map_err(|e| {
                            DecodeError::DecodeFailure(format!("SetOutputType(stream change): {e}"))
                        })?;
                        continue;
                    }
                    Err(e) => {
                        let _ = ManuallyDrop::take(&mut out.pSample);
                        return Err(DecodeError::DecodeFailure(format!("ProcessOutput: {e}")));
                    }
                }
                let sample = ManuallyDrop::take(&mut out.pSample).ok_or_else(|| {
                    DecodeError::DecodeFailure("ProcessOutput 成功但没有输出样本".to_owned())
                })?;
                let _ = ManuallyDrop::take(&mut out.pEvents);
                return Ok(Some(self.extract_frame(&sample)?));
            }
        }
        Ok(None)
    }

    /// 把一个输出样本转成 NV12 帧。
    fn extract_frame(&self, sample: &IMFSample) -> Result<DecodedFrame, DecodeError> {
        let buffer = unsafe { sample.ConvertToContiguousBuffer() }
            .map_err(|e| DecodeError::DecodeFailure(format!("ConvertToContiguousBuffer: {e}")))?;
        let mut ptr: *mut u8 = std::ptr::null_mut();
        let mut len = 0u32;
        unsafe {
            buffer
                .Lock(&mut ptr, None, Some(&mut len))
                .map_err(|e| DecodeError::DecodeFailure(format!("Lock(out): {e}")))?;
            let expected = (self.width as usize)
                .saturating_mul(self.height as usize)
                .saturating_mul(3)
                / 2;
            if (len as usize) < expected {
                let _ = buffer.Unlock();
                return Err(DecodeError::DecodeFailure(format!(
                    "解码输出长度 {len} 小于 NV12 期望 {expected}"
                )));
            }
            let data = std::slice::from_raw_parts(ptr, expected).to_vec();
            let _ = buffer.Unlock();
            Ok(DecodedFrame {
                ts_us: self.sample_ts_100ns(sample) / 10,
                width: self.width,
                height: self.height,
                stride: self.width,
                data,
            })
        }
    }

    /// 读取输出样本时间（100ns），缺失时回退到最近输入时间。
    fn sample_ts_100ns(&self, sample: &IMFSample) -> u64 {
        unsafe {
            if let Ok(ts) = sample.GetSampleTime() {
                if ts > 0 {
                    return ts as u64;
                }
            }
        }
        self.last_input_ts_100ns
    }

    /// 构造 Annex-B 输入样本。
    fn build_input_sample(
        &self,
        annexb: &[u8],
        ts_us: u64,
    ) -> Result<IMFSample, DecodeError> {
        let buffer = unsafe { MFCreateMemoryBuffer(annexb.len() as u32) }
            .map_err(|e| DecodeError::DecodeFailure(format!("MFCreateMemoryBuffer(in): {e}")))?;
        let mut ptr: *mut u8 = std::ptr::null_mut();
        unsafe {
            buffer
                .Lock(&mut ptr, None, None)
                .map_err(|e| DecodeError::DecodeFailure(format!("Lock(in): {e}")))?;
            std::ptr::copy_nonoverlapping(annexb.as_ptr(), ptr, annexb.len());
            buffer
                .Unlock()
                .map_err(|e| DecodeError::DecodeFailure(format!("Unlock(in): {e}")))?;
            buffer
                .SetCurrentLength(annexb.len() as u32)
                .map_err(|e| DecodeError::DecodeFailure(format!("SetCurrentLength: {e}")))?;
        }
        let sample = unsafe { MFCreateSample() }
            .map_err(|e| DecodeError::DecodeFailure(format!("MFCreateSample(in): {e}")))?;
        unsafe {
            sample
                .AddBuffer(&buffer)
                .map_err(|e| DecodeError::DecodeFailure(format!("AddBuffer(in): {e}")))?;
            sample
                .SetSampleTime((ts_us.saturating_mul(10)) as i64)
                .map_err(|e| DecodeError::DecodeFailure(format!("SetSampleTime: {e}")))?;
            sample
                .SetSampleDuration(self.duration_100ns as i64)
                .map_err(|e| DecodeError::DecodeFailure(format!("SetSampleDuration: {e}")))?;
        }
        Ok(sample)
    }

    /// 等一个事件，带截止时刻。
    fn wait_event(&self, deadline: Instant) -> Result<Option<EventKind>, DecodeError> {
        let Some(events) = self.events.as_ref() else {
            return Ok(None);
        };
        loop {
            match unsafe { events.GetEvent(MF_EVENT_FLAG_NO_WAIT) } {
                Ok(event) => {
                    let kind = unsafe { event.GetType() }.unwrap_or(0);
                    return Ok(Some(match kind as i32 {
                        k if k == METransformNeedInput.0 => EventKind::NeedInput,
                        k if k == METransformHaveOutput.0 => EventKind::HaveOutput,
                        _ => EventKind::Other,
                    }));
                }
                Err(e) if e.code() == MF_E_NO_EVENTS_AVAILABLE => {
                    if Instant::now() >= deadline {
                        return Ok(None);
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                Err(e) => {
                    return Err(DecodeError::DecodeFailure(format!("GetEvent: {e}")));
                }
            }
        }
    }
}

impl Drop for MfH264Decoder {
    fn drop(&mut self) {
        unsafe {
            let _ = self
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
        }
    }
}

// =====** 媒体类型 =====**

/// 构造 NV12 输出类型。
fn make_output_type(width: u32, height: u32, fps: u32) -> Result<IMFMediaType, DecodeError> {
    let media_type = unsafe { MFCreateMediaType() }
        .map_err(|e| DecodeError::DecodeFailure(format!("MFCreateMediaType(out): {e}")))?;
    unsafe {
        set_guid(&media_type, &MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        set_guid(&media_type, &MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
        media_type
            .SetUINT64(&MF_MT_FRAME_SIZE, pack_u64(width, height))
            .map_err(invalid)?;
        media_type
            .SetUINT64(&MF_MT_FRAME_RATE, pack_u64(fps.max(1), 1))
            .map_err(invalid)?;
        media_type
            .SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, pack_u64(1, 1))
            .map_err(invalid)?;
        media_type
            .SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)
            .map_err(invalid)?;
    }
    Ok(media_type)
}

/// 构造 H.264 输入类型。
fn make_input_type() -> Result<IMFMediaType, DecodeError> {
    let media_type = unsafe { MFCreateMediaType() }
        .map_err(|e| DecodeError::DecodeFailure(format!("MFCreateMediaType(in): {e}")))?;
    unsafe {
        set_guid(&media_type, &MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        set_guid(&media_type, &MF_MT_SUBTYPE, &MFVideoFormat_H264)?;
    }
    Ok(media_type)
}

/// 设置 GUID 属性。
unsafe fn set_guid(
    media_type: &IMFMediaType,
    key: &GUID,
    value: &GUID,
) -> Result<(), DecodeError> {
    media_type
        .SetGUID(key, value)
        .map_err(|e| DecodeError::DecodeFailure(format!("SetGUID: {e}")))?;
    Ok(())
}

/// 把两个 u32 打包成 u64（高 32 位在前）。
fn pack_u64(high: u32, low: u32) -> u64 {
    (u64::from(high) << 32) | u64::from(low)
}

/// 属性设置失败统一转换。
fn invalid(error: windows::core::Error) -> DecodeError {
    DecodeError::DecodeFailure(format!("SetAttribute: {error}"))
}

// =====** 自检 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 帧尺寸打包必须高 32 位为宽、低 32 位为高（与 MF 约定一致）。
    #[test]
    fn pack_frame_size() {
        assert_eq!(pack_u64(1920, 1080), 0x0000_0780_0000_0438);
    }

    /// 解码器名称枚举不应 panic；本机无硬件时至少返回软件解码器。
    #[test]
    fn enumerate_returns_names() {
        let (hardware, all) = decoder_names();
        assert!(all.iter().any(|name| !name.is_empty()));
        assert!(hardware.len() <= all.len());
    }

    /// 非法尺寸必须被拒绝。
    #[test]
    fn zero_size_is_rejected() {
        assert!(MfH264Decoder::open(0, 1080, 60, false).is_err());
    }
}
