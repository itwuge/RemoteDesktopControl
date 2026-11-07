//! rdc-codec / Windows 平台编码器后端
//!
//! 层级：L3 编解码层 · 平台适配
//! 用途：基于 Media Foundation 的 H.264 硬件编码实现（无 GPL 组件），
//!       提供编码器能力探测（分辨率/帧率/色度/位深/码率范围）、
//!       运行期参数热更新（码率/GOP/帧率）、以及编码耗时与实测码率统计
//! 调用方：rdc-codec::platform::create_encoder（rdc-cli 的 S3a 端到端验证）
//!
//! 设计取舍：
//! - 只枚举 `MFT_ENUM_FLAG_HARDWARE`；无硬件时直接返回 [`CodecError::HardwareUnsupported`]，
//!   绝不静默切软编（软编只在显式 `allow_software` 时可被探测报告列出）。
//! - 编码会话独占单线程；`push` 内部同步完成 ProcessInput/ProcessOutput，
//!   因此 `poll_packet` 只是从已完成的输出队列取包（详见 docs/决策待办.md D18）。

use std::collections::VecDeque;
use std::time::Instant;

use rdc_protocol::codec::{CodecKind as ProtocolCodecKind, EncodedFrame, EncoderConfig, RawFrame};
use rdc_protocol::ProtocolError;
use rdc_runtime::PooledBuffer;
use windows::core::Interface;
use windows::Win32::Media::MediaFoundation::{
    IMFActivate, IMFTransform, MF_TRANSFORM_ASYNC_UNLOCK,
};

use crate::encoder::{
    CodecKind, EncodedPacket, EncoderCapability, EncoderCaps, ProbeReport, VideoChroma, VideoConfig,
    VideoEncoder,
};
use crate::error::CodecError;
use crate::metrics::{EncodeSnapshot, EncodeStats};

// =====** 子模块 =====**

// Media Foundation H.264 会话封装（Rust 2018 允许 `windows.rs` + `windows/` 目录并存）。
pub mod mf;

// AMD AMF 原生编码后端（经 rdc-amf.dll 的 C ABI；MF 保留为回退）。
pub mod amf;

// =====** 常量 =====**

/// 探测时尝试的分辨率候选（从大到小，取第一个协商成功的）。
const RESOLUTION_CANDIDATES: [(u32, u32); 4] =
    [(3840, 2160), (2560, 1440), (1920, 1080), (1280, 720)];

/// 探测时尝试的帧率候选（从高到低）。
const FPS_CANDIDATES: [u32; 3] = [60, 30, 24];

/// 探测时尝试的码率候选（用于记录「可设置范围」，单位 bps）。
const BITRATE_CANDIDATES: [u32; 5] =
    [1_000_000, 4_000_000, 8_000_000, 16_000_000, 32_000_000];

/// 硬件编码器选择名称前缀，用于日志与报告。
const BACKEND_HW: &str = "mf-h264-hw";

// =====** PlatformEncoder =====**

/// Windows Media Foundation H.264 编码器。
pub struct PlatformEncoder {
    /// 最近一次生效的配置。
    config: Option<VideoConfig>,
    /// 已激活的 MFT 句柄，用于分辨率变化时重建会话。
    activation: Option<IMFActivate>,
    /// 当前编码会话。
    session: Option<mf::MfH264Session>,
    /// 编码统计（耗时/码率/帧数）。
    stats: EncodeStats,
    /// 统计起点；首次 `push` 时置位。
    started: Option<Instant>,
    /// 已产出但尚未被 `poll_packet` 取走的包。
    queue: VecDeque<EncodedPacket>,
    /// 实际选用的编码器名称。
    selected_name: String,
    /// 是否允许软件 MFT（仅 `RDC_ENCODER=software` 诊断路径为 true）。
    software: bool,
}

impl PlatformEncoder {
    /// 构造未配置的编码器。
    ///
    /// # Returns
    /// 空实例；硬件会话在 [`VideoEncoder::configure`] 中创建。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new() -> Self {
        Self {
            config: None,
            activation: None,
            session: None,
            stats: EncodeStats::new(),
            started: None,
            queue: VecDeque::new(),
            selected_name: String::new(),
            software: false,
        }
    }

    /// 构造「只允许软件 MFT」的编码器（S7-7 块 3 对照基线，默认路径不会走到这里）。
    ///
    /// # Returns
    /// 空实例；`configure` 时按 `MFT_ENUM_FLAG_ALL` 枚举并优先选择同步软件 MFT。
    ///
    /// # Errors
    /// 不会失败；选不到软件 MFT 时由 `configure` 返回错误，不回退硬编。
    pub fn new_software() -> Self {
        Self {
            software: true,
            ..Self::new()
        }
    }

    /// 实际选用的编码器名称（未配置时返回后端标识）。
    ///
    /// # Returns
    /// 名称字符串。
    ///
    /// # Errors
    /// 不会失败。
    fn current_backend_name(&self) -> &str {
        if self.selected_name.is_empty() {
            BACKEND_HW
        } else {
            &self.selected_name
        }
    }

    /// 把一段 RGBA8 像素编码为码流包。
    ///
    /// 为什么单独抽出来：池化路径（`push`）与协议路径（`Encoder::encode`）共用同一段
    /// 「计时 + 提交 + 取输出 + 统计」逻辑，避免两处口径不一致。
    fn encode_slice(
        &mut self,
        rgba: &[u8],
        pts_ns: u64,
        force_keyframe: bool,
    ) -> Result<Vec<EncodedPacket>, CodecError> {
        let Some(session) = self.session.as_mut() else {
            return Err(CodecError::InvalidConfig("编码器尚未配置".to_owned()));
        };
        if force_keyframe {
            session.request_keyframe();
        }
        let fps = session.fps().max(1);
        let duration_100ns = (10_000_000i64 / i64::from(fps)).max(1);
        let started = Instant::now();
        let samples = session.encode(rgba, (pts_ns as i64) / 100, duration_100ns)?;
        let cost_us = started.elapsed().as_micros() as u64;

        let mut packets = Vec::with_capacity(samples.len());
        for (idx, sample) in samples.iter().enumerate() {
            // 一次 encode 调用可能吐出 0~2 个包：耗时只记在第一个包上，其余按 0 记，
            // 保证「p99 编码耗时」不被同一调用的重复计数放大。
            let cost = if idx == 0 { cost_us } else { 0 };
            self.stats
                .record(cost, cost_us, sample.data.len(), sample.keyframe);
            packets.push(EncodedPacket {
                data: sample.data.clone(),
                pts_ns: if sample.pts_100ns > 0 {
                    (sample.pts_100ns as u64) * 100
                } else {
                    pts_ns
                },
                is_keyframe: sample.keyframe,
                // 0 表示交由传输层按 MTU 决定分片大小。
                suggested_shard_bytes: 0,
            });
        }
        if self.started.is_none() {
            self.started = Some(Instant::now());
        }
        // 首帧编码后驱动通常才写入 SPS/PPS，这里顺手刷新一次，供封装模块使用。
        let _ = session.refresh_sequence_header();
        Ok(packets)
    }

    /// 用已激活的 MFT 创建会话（分辨率变化时复用同一激活对象重建）。
    fn build_session(&mut self, cfg: &VideoConfig) -> Result<(), CodecError> {
        let pcfg = to_protocol_config(cfg);
        pcfg.validate()
            .map_err(|e| CodecError::InvalidConfig(format!("配置非法: {e}")))?;
        let Some(activation) = self.activation.as_ref() else {
            return Err(CodecError::HardwareUnsupported);
        };
        let session = mf::MfH264Session::new(activation, &pcfg)?;
        self.selected_name = session.name().to_owned();
        self.config = Some(*cfg);
        self.session = Some(session);
        Ok(())
    }
}

impl Default for PlatformEncoder {
    /// 等价于 [`PlatformEncoder::new`]。
    fn default() -> Self {
        Self::new()
    }
}

// 安全性说明：`windows` crate 的 COM 接口类型默认 `!Send`（其内部是非空指针）。
// 本编码器由单一编码线程独占使用，且所有 COM 对象都在该线程创建与释放；
// 这里手动声明 `Send` 只是为了把实例移交给该线程（创建线程与使用线程之间没有并发访问，
// 也没有跨线程 release）。若将来要在多线程共享，必须先改为 MTA 封送。
unsafe impl Send for PlatformEncoder {}

/// 把动态错误串提升为协议层要求的 `&'static str`。
///
/// 为什么不返回 `String`：`ProtocolError` 是冻结的线格式类型，其字段为 `&'static str`。
/// 编码失败属于终态错误，发生频率极低，泄漏一小段字符串换取「不改协议」是可接受的。
fn leak(msg: String) -> &'static str {
    Box::leak(msg.into_boxed_str())
}

// =====** VideoEncoder 实现 =====**

impl VideoEncoder for PlatformEncoder {
    /// 探测硬件 H.264 能力并转换为摘要。
    ///
    /// # Arguments
    /// * `codecs` - 待探测格式；空切片表示全部已知格式。
    ///
    /// # Returns
    /// 受支持格式的摘要列表；本次只实现 H.264，因此非 H.264 返回空。
    ///
    /// # Errors
    /// 不会失败；无硬件时返回空列表。
    fn probe(&self, codecs: &[CodecKind]) -> Vec<EncoderCaps> {
        if !codecs.is_empty() && !codecs.contains(&CodecKind::H264) {
            return Vec::new();
        }
        let report = probe_report();
        if !report.hardware_available {
            return Vec::new();
        }
        let Some(best) = report.hardware.first() else {
            return Vec::new();
        };
        vec![EncoderCaps {
            codec: CodecKind::H264,
            max_width: best.max_width,
            max_height: best.max_height,
            max_fps: best.max_fps,
            // H.264 硬件 MFT 只接受 NV12（4:2:0 8-bit），4:4:4 / 10-bit 不可用。
            supports_444: false,
            supports_10bit: false,
            supports_runtime_reconfigure: best.runtime_reconfigure,
        }]
    }

    /// 配置编码器：枚举硬件 MFT 并建立会话。
    ///
    /// # Arguments
    /// * `config` - 编码格式、分辨率、帧率、码率、色度与位深。
    ///
    /// # Returns
    /// 会话就绪返回 `Ok(())`。
    ///
    /// # Errors
    /// 非 H.264、宽高非偶数或超出能力时返回 [`CodecError::InvalidConfig`]；
    /// 无硬件编码器时返回 [`CodecError::HardwareUnsupported`]（不静默软编）。
    fn configure(&mut self, config: VideoConfig) -> Result<(), CodecError> {
        if config.codec != CodecKind::H264 {
            return Err(CodecError::InvalidConfig(format!(
                "Windows 后端 S3a 只实现 H.264，收到 {:?}",
                config.codec
            )));
        }
        // NV12 为 4:2:0，宽高必须为偶数；奇数尺寸在这里明确拒绝而不是让驱动静默裁剪。
        if config.width % 2 != 0 || config.height % 2 != 0 {
            return Err(CodecError::InvalidConfig(format!(
                "NV12 要求偶数宽高，收到 {}x{}",
                config.width, config.height
            )));
        }
        if config.bit_depth != 8 {
            return Err(CodecError::InvalidConfig(format!(
                "硬件 MFT 仅接受 8-bit 输入，收到 {}",
                config.bit_depth
            )));
        }
        if config.chroma != VideoChroma::Yuv420 {
            return Err(CodecError::InvalidConfig(
                "硬件 MFT 仅接受 4:2:0（NV12）输入".to_owned(),
            ));
        }

        let activations = mf::enumerate_h264(self.software)?;
        if activations.is_empty() {
            return Err(CodecError::HardwareUnsupported);
        }
        // 按系统排序依次尝试：某个 MFT 激活/协商失败时换下一个，避免单点失败。
        let mut last_err = CodecError::HardwareUnsupported;
        for activation in activations {
            // 软编诊断路径必须真选到软件 MFT；选不到就让 configure 失败，绝不静默退回硬编。
            if self.software && !mf::is_software_activation(&activation) {
                continue;
            }
            let pcfg = to_protocol_config(&config);
            match mf::MfH264Session::new(&activation, &pcfg) {
                Ok(session) => {
                    self.selected_name = session.name().to_owned();
                    if self.software {
                        // M1 要求：软编诊断路径必须打印实际选中的 MFT 名称与 software 标记。
                        eprintln!(
                            "[enc] backend=mf-sw selected=\"{}\" software=true",
                            self.selected_name
                        );
                    }
                    self.session = Some(session);
                    self.activation = Some(activation);
                    self.config = Some(config);
                    return Ok(());
                }
                Err(err) => last_err = err,
            }
        }
        Err(last_err)
    }

    /// 提交一帧 RGBA8 并同步取回码流。
    ///
    /// # Arguments
    /// * `frame` - 池化帧（RGBA8，长度 `w*h*4`）；调用返回后归还内存池。
    /// * `pts_ns` - 采集时间戳（纳秒）。
    /// * `force_keyframe` - 是否强制本帧为 IDR。
    ///
    /// # Returns
    /// 入队成功返回 `Ok(())`；输出通过 [`VideoEncoder::poll_packet`] 取走。
    ///
    /// # Errors
    /// 未配置、像素长度不足或 MFT 失败时返回 [`CodecError`]。
    fn push(
        &mut self,
        frame: PooledBuffer,
        pts_ns: u64,
        force_keyframe: bool,
    ) -> Result<(), CodecError> {
        let packets = self.encode_slice(frame.as_slice(), pts_ns, force_keyframe)?;
        // frame 在此 drop，缓冲自动归还内存池。
        drop(frame);
        self.queue.extend(packets);
        Ok(())
    }

    /// 取出一包编码结果。
    ///
    /// # Returns
    /// 队列非空时返回一包；空队列返回 `None`。
    ///
    /// # Errors
    /// 不会失败。
    fn poll_packet(&mut self) -> Option<EncodedPacket> {
        self.queue.pop_front()
    }

    /// 运行期重配。
    ///
    /// # Arguments
    /// * `config` - 新配置。
    ///
    /// # Returns
    /// 参数已下发返回 `Ok(())`。
    ///
    /// # Errors
    /// 格式/色度/位深变化无法热更新时返回 [`CodecError::InvalidConfig`]；
    /// 分辨率变化会重建会话，重建失败时返回会话错误。
    fn reconfigure(&mut self, config: VideoConfig) -> Result<(), CodecError> {
        let Some(current) = self.config else {
            return Err(CodecError::InvalidConfig("编码器尚未配置".to_owned()));
        };
        if config.codec != current.codec
            || config.chroma != current.chroma
            || config.bit_depth != current.bit_depth
        {
            return Err(CodecError::InvalidConfig(
                "S3a 热更新仅支持分辨率/帧率/码率/GOP；格式/色度/位深变化需重建会话".to_owned(),
            ));
        }
        if config.width == current.width && config.height == current.height {
            // 分辨率不变：码率/GOP 走 ICodecAPI，帧率走输出类型重协商。
            let pcfg = to_protocol_config(&config);
            if let Some(session) = self.session.as_mut() {
                session.update_rate_control(&pcfg)?;
                if config.fps != current.fps {
                    session.update_fps(config.fps)?;
                }
            }
            self.config = Some(config);
            return Ok(());
        }
        // 分辨率变化：NV12 尺寸参与类型协商，只能重建会话。
        self.session = None;
        self.build_session(&config)
    }

    /// 请求下一帧为关键帧。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    fn request_keyframe(&mut self) {
        if let Some(session) = self.session.as_mut() {
            session.request_keyframe();
        }
    }

    /// 实际后端名称（含所选 MFT 友好名称）。
    ///
    /// # Returns
    /// 未配置时返回 `mf-h264-hw`，配置后返回具体驱动名称。
    ///
    /// # Errors
    /// 不会失败。
    fn backend_name(&self) -> &str {
        self.current_backend_name()
    }

    /// 编码统计快照。
    ///
    /// # Returns
    /// 尚未提交首帧时返回 `None`；否则按「首帧到现在的毫秒数」计算。
    ///
    /// # Errors
    /// 不会失败。
    fn stats_snapshot(&self) -> Option<EncodeSnapshot> {
        let started = self.started?;
        Some(self.stats.snapshot(started.elapsed().as_millis() as u64))
    }

    /// 序列头（SPS/PPS）。
    ///
    /// # Returns
    /// Annex-B 序列头；驱动未提供时为空。
    ///
    /// # Errors
    /// 不会失败。
    fn sequence_header(&self) -> &[u8] {
        // 该方法需要刷新输出类型，而 trait 只给 `&self`；序列头在 `new` 与首次
        // `refresh_sequence_header` 后已缓存，这里直接读缓存值。
        match self.session.as_ref() {
            Some(session) => session.sequence_header(),
            None => &[],
        }
    }

    /// 冲刷 MFT 流水线，把滞留帧编码结果取出。
    ///
    /// # Returns
    /// 剩余码流包（已按提交顺序）。
    ///
    /// # Errors
    /// MFT 冲刷失败时返回 [`CodecError::EncoderFailure`]。
    fn flush(&mut self) -> Result<Vec<EncodedPacket>, CodecError> {
        let Some(session) = self.session.as_mut() else {
            return Ok(Vec::new());
        };
        let samples = session.drain()?;
        let mut packets = Vec::with_capacity(samples.len());
        for sample in samples {
            // 冲刷出的帧同样计入码率统计，但耗时为 0（不代表一次真实编码调用）。
            self.stats
                .record(0, 0, sample.data.len(), sample.keyframe);
            packets.push(EncodedPacket {
                data: sample.data,
                pts_ns: (sample.pts_100ns.max(0) as u64) * 100,
                is_keyframe: sample.keyframe,
                suggested_shard_bytes: 0,
            });
        }
        Ok(packets)
    }
}

// =====** 协议 Encoder 实现（冻结接口） =====**

impl rdc_protocol::codec::Encoder for PlatformEncoder {
    /// 实现名称。
    ///
    /// # Returns
    /// 硬件路径返回 `mf-h264-hw`，未配置时返回 `mf-h264`。
    fn name(&self) -> &'static str {
        if self.session.is_some() {
            BACKEND_HW
        } else {
            "mf-h264"
        }
    }

    /// 能力位。
    ///
    /// # Returns
    /// S3a 只保证 H.264 基线能力（HEVC/4:4:4/10-bit 均未实现），因此为 `0`。
    fn capabilities(&self) -> u64 {
        0
    }

    /// 应用协议配置。
    ///
    /// # Arguments
    /// * `config` - 冻结的协议配置。
    ///
    /// # Returns
    /// 已建立会话返回 `Ok(())`。
    ///
    /// # Errors
    /// 配置非法时返回 [`ProtocolError::InvalidField`]；硬件不可用时返回
    /// [`ProtocolError::CodecUnsupported`]。
    fn configure(&mut self, config: &EncoderConfig) -> Result<(), ProtocolError> {
        config.validate()?;
        let vc = VideoConfig {
            codec: CodecKind::H264,
            width: config.width,
            height: config.height,
            fps: config.fps,
            bitrate_bps: config.target_bitrate_bps,
            chroma: if config.chroma_444 {
                VideoChroma::Yuv444
            } else {
                VideoChroma::Yuv420
            },
            bit_depth: config.bit_depth,
            low_latency: config.low_latency,
        };
        VideoEncoder::configure(self, vc).map_err(|e| match e {
            CodecError::InvalidConfig(msg) => ProtocolError::InvalidField(leak(msg)),
            _ => ProtocolError::CodecUnsupported("Windows 后端无可用 H.264 硬件编码器"),
        })
    }

    /// 编码一帧 RGBA8。
    ///
    /// # Arguments
    /// * `frame` - 协议帧（RGBA8）。
    ///
    /// # Returns
    /// 第一个输出包映射为 [`EncodedFrame`]；无输出时返回空数据的帧。
    ///
    /// # Errors
    /// 编码失败返回 [`ProtocolError::State`]。
    fn encode(&mut self, frame: &RawFrame) -> Result<EncodedFrame, ProtocolError> {
        frame.validate()?;
        let started = Instant::now();
        let mut packets = self
            .encode_slice(&frame.rgba, frame.capture_ts_us * 1000, false)
            .map_err(|e| ProtocolError::State(leak(format!("编码失败: {e}"))))?;
        if packets.is_empty() {
            packets.push(EncodedPacket {
                data: Vec::new(),
                pts_ns: frame.capture_ts_us * 1000,
                is_keyframe: false,
                suggested_shard_bytes: 0,
            });
        }
        let pkt = packets.remove(0);
        Ok(EncodedFrame {
            frame_id: frame.frame_id,
            capture_ts_us: frame.capture_ts_us,
            keyframe: pkt.is_keyframe,
            temporal_id: 0,
            data: pkt.data,
            encode_ms: started.elapsed().as_secs_f32() * 1000.0,
        })
    }

    /// 请求关键帧。
    fn request_keyframe(&mut self) {
        VideoEncoder::request_keyframe(self);
    }

    /// 冲刷编码器。
    ///
    /// # Returns
    /// 缓存中剩余帧的 [`EncodedFrame`] 列表（帧号统一填 `u16::MAX`，表示非输入帧）。
    ///
    /// # Errors
    /// 失败返回 [`ProtocolError::State`]。
    fn flush(&mut self) -> Result<Vec<EncodedFrame>, ProtocolError> {
        let Some(session) = self.session.as_mut() else {
            return Ok(Vec::new());
        };
        let samples = session
            .drain()
            .map_err(|e| ProtocolError::State(leak(format!("flush 失败: {e}"))))?;
        Ok(samples
            .into_iter()
            .map(|s| EncodedFrame {
                frame_id: u16::MAX,
                capture_ts_us: (s.pts_100ns.max(0) as u64) * 100,
                keyframe: s.keyframe,
                temporal_id: 0,
                data: s.data,
                encode_ms: 0.0,
            })
            .collect())
    }
}

// =====** 能力探测 =====**

/// 探测本机可用编码器并给出完整能力报告。
///
/// 报告分两类：硬件（默认选用）与软件（仅列出，供显式启用时参考）。
///
/// # Returns
/// 探测报告；`error` 非空表示探测本身失败。
///
/// # Errors
/// 不会失败——错误写进报告的 `error` 字段，便于 CLI 直接打印。
pub fn probe_report() -> ProbeReport {
    let mut report = ProbeReport::default();
    let hardware = match mf::enumerate_h264(false) {
        Ok(list) => list,
        Err(err) => {
            report.error = Some(err.to_string());
            return report;
        }
    };
    for activation in &hardware {
        if let Some(cap) = probe_one(activation, true) {
            report.hardware.push(cap);
        }
    }
    report.hardware_available = !report.hardware.is_empty();

    // 软件 MFT 只用于报告：拿全量枚举结果减去硬件名称集合。
    if let Ok(all) = mf::enumerate_h264(true) {
        let hw_names: Vec<&str> = report.hardware.iter().map(|c| c.name.as_str()).collect();
        for activation in &all {
            if let Some(cap) = probe_one(activation, false) {
                if !hw_names.contains(&cap.name.as_str()) {
                    report.software.push(cap);
                }
            }
        }
    }
    report
}

/// 探测单个 MFT：激活后按候选分辨率/帧率/码率做类型协商。
///
/// # Arguments
/// * `activate` - 已枚举的激活对象。
/// * `hardware` - 是否硬件（仅用于标注）。
///
/// # Returns
/// 协商成功时返回能力记录；激活失败返回 `None`。
///
/// # Errors
/// 不会失败。
fn probe_one(activate: &IMFActivate, hardware: bool) -> Option<EncoderCapability> {
    let name = mf::activation_name(activate);

    // 每个候选分辨率都激活一个全新的 MFT：AMD 等驱动的失败协商会污染实例状态，
    // 复用实例会让后续正确尺寸也被拒绝（0xC00D6D60）。
    let mut best: Option<(IMFTransform, u32, u32, u32)> = None;
    let mut probe_error = String::new();
    'outer: for (width, height) in RESOLUTION_CANDIDATES {
        for fps in FPS_CANDIDATES {
            let cfg = probe_config(width, height, fps, 8_000_000);
            match probe_negotiate(activate, &cfg) {
                Ok(transform) => {
                    // 候选表按「分辨率优先、帧率次之」排列，首个成功即最优。
                    best = Some((transform, width, height, fps));
                    break 'outer;
                }
                Err(err) => probe_error = format!("{width}x{height}@{fps}: {err}"),
            }
        }
    }
    let (transform, width, height, fps) = match best {
        Some(v) => v,
        None => {
            // 诊断：协商全部失败时把最后一次错误打印出来，便于定位驱动限制。
            eprintln!("[probe] {name} 未通过类型协商: {probe_error}");
            return None;
        }
    };

    // 码率范围：在最优分辨率上逐个尝试候选码率，记录可接受的区间。
    // 每次都用新实例，避免上一次协商的残留状态影响判断。
    let mut min_bitrate = 0u32;
    let mut max_bitrate = 0u32;
    for rate in BITRATE_CANDIDATES {
        let cfg = probe_config(width, height, fps, rate);
        if probe_negotiate(activate, &cfg).is_ok() {
            if min_bitrate == 0 {
                min_bitrate = rate;
            }
            max_bitrate = rate;
        }
    }

    let runtime_reconfigure = transform
        .cast::<windows::Win32::Media::MediaFoundation::ICodecAPI>()
        .is_ok();
    Some(EncoderCapability {
        name,
        hardware,
        codec: CodecKind::H264,
        max_width: width,
        max_height: height,
        max_fps: fps,
        chroma: VideoChroma::Yuv420,
        bit_depth: 8,
        min_bitrate_bps: min_bitrate,
        max_bitrate_bps: max_bitrate,
        runtime_reconfigure,
        note: if probe_error.is_empty() {
            "分辨率/帧率/码率由 SetInputType+SetOutputType 协商得出；色度为 NV12(4:2:0)，位深 8".to_owned()
        } else {
            format!(
                "分辨率/帧率/码率由 SetInputType+SetOutputType 协商得出；色度为 NV12(4:2:0)，位深 8；{probe_error}"
            )
        },
    })
}

/// 组装探测用的协议配置。
fn probe_config(width: u32, height: u32, fps: u32, bitrate: u32) -> EncoderConfig {
    EncoderConfig {
        codec: ProtocolCodecKind::H264,
        width,
        height,
        fps,
        target_bitrate_bps: bitrate,
        gop: fps,
        intra_refresh: false,
        low_latency: true,
        chroma_444: false,
        bit_depth: 8,
        temporal_layers: 1,
    }
}

/// 尝试一次输入/输出类型协商。
///
/// # Arguments
/// * `activate` - 激活对象（每次调用都会新建实例，避免失败状态污染）。
/// * `cfg` - 待协商配置。
///
/// # Returns
/// 协商成功的 MFT 实例（供后续码率尝试复用）。
///
/// # Errors
/// 激活失败、异步解锁失败或任一 SetInputType/SetOutputType 失败返回 [`CodecError`]。
fn probe_negotiate(activate: &IMFActivate, cfg: &EncoderConfig) -> Result<IMFTransform, CodecError> {
    let transform: IMFTransform = unsafe {
        activate
            .ActivateObject()
            .map_err(|e| CodecError::EncoderFailure(format!("ActivateObject: {e}")))
    }?;
    // 异步 MFT 必须先解锁；AMD 等驱动不实现 transform 上的 IMFAttributes 查询，
    // 必须用 GetAttributes() 取属性存储，否则解锁静默失效（0xC00D6D77）。
    let attrs = unsafe { transform.GetAttributes() }.map_err(|e| {
        CodecError::EncoderFailure(format!("GetAttributes: {e}"))
    })?;
    unsafe {
        attrs
            .SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)
            .map_err(|e| CodecError::EncoderFailure(format!("异步解锁失败: {e}")))?;
    }

    let input = mf::make_input_type(cfg)?;
    let output = mf::make_output_type(cfg)?;
    unsafe {
        // 与真实会话保持一致：AMD MFT 必须先输出后输入。
        transform
            .SetOutputType(0, &output, 0)
            .map_err(|e| CodecError::InvalidConfig(format!("SetOutputType: {e}")))?;
        transform
            .SetInputType(0, &input, 0)
            .map_err(|e| CodecError::InvalidConfig(format!("SetInputType: {e}")))?;
    }
    Ok(transform)
}

/// 把 crate 侧配置映射为冻结协议配置。
///
/// # Arguments
/// * `cfg` - crate 配置。
///
/// # Returns
/// 协议配置；GOP 按 1 秒一个关键帧（低延迟场景足够，且便于按 GOP 切分分片组）。
///
/// # Errors
/// 不会失败。
fn to_protocol_config(cfg: &VideoConfig) -> EncoderConfig {
    EncoderConfig {
        codec: ProtocolCodecKind::H264,
        width: cfg.width,
        height: cfg.height,
        fps: cfg.fps.max(1),
        target_bitrate_bps: cfg.bitrate_bps.max(1),
        gop: cfg.fps.max(1),
        intra_refresh: false,
        low_latency: cfg.low_latency,
        chroma_444: cfg.chroma == VideoChroma::Yuv444,
        bit_depth: cfg.bit_depth,
        temporal_layers: 1,
    }
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 协议配置映射必须保留分辨率/帧率/码率，GOP 固定为 1 秒。
    #[test]
    fn protocol_config_mapping() {
        let vc = VideoConfig {
            codec: CodecKind::H264,
            width: 1920,
            height: 1080,
            fps: 60,
            bitrate_bps: 8_000_000,
            chroma: VideoChroma::Yuv420,
            bit_depth: 8,
            low_latency: true,
        };
        let pcfg = to_protocol_config(&vc);
        assert_eq!(pcfg.width, 1920);
        assert_eq!(pcfg.height, 1080);
        assert_eq!(pcfg.fps, 60);
        assert_eq!(pcfg.gop, 60);
        assert!(!pcfg.chroma_444);
        assert!(pcfg.validate().is_ok());
    }

    /// 奇数宽高与 10-bit 必须在创建会话前被拒绝。
    #[test]
    fn configure_rejects_invalid_input() {
        let mut enc = PlatformEncoder::new();
        let mut cfg = VideoConfig {
            codec: CodecKind::H264,
            width: 1921,
            height: 1080,
            fps: 60,
            bitrate_bps: 8_000_000,
            chroma: VideoChroma::Yuv420,
            bit_depth: 8,
            low_latency: true,
        };
        assert!(matches!(
            enc.configure(cfg),
            Err(CodecError::InvalidConfig(_))
        ));
        cfg.width = 1920;
        cfg.bit_depth = 10;
        assert!(matches!(
            enc.configure(cfg),
            Err(CodecError::InvalidConfig(_))
        ));
        cfg.bit_depth = 8;
        cfg.chroma = VideoChroma::Yuv444;
        assert!(matches!(
            enc.configure(cfg),
            Err(CodecError::InvalidConfig(_))
        ));
    }

    /// 未配置时 `poll_packet` 与 `request_keyframe` 必须是安全空操作。
    #[test]
    fn unconfigured_encoder_is_safe() {
        let mut enc = PlatformEncoder::new();
        assert!(enc.poll_packet().is_none());
        enc.request_keyframe();
        assert!(enc.sequence_header().is_empty());
        assert_eq!(rdc_protocol::codec::Encoder::capabilities(&enc), 0);
    }
}
