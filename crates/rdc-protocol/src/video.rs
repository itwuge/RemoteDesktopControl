//! rdc-protocol / 远端视频流消息
//!
//! 层级：L2 能力层
//! 用途：定义 Android 被控端/主控端之间的 H.264 视频流元数据与访问单元分片格式，
//!       并提供接收侧重排、丢包检测与重新同步（等下一个 IDR）的唯一实现。
//! 调用方：rdc-bridge 的视频订阅服务；宿主集成测试用离线码流直接喂消息。
//!
//! 与 S4 `WallMessage` 的关系：分片思路一致（先帧头、再 ≤1352 字节分片、时间戳用
//! 单调时钟微秒），但**消息族独立**（tag 0x61~0x64），避免与墙上缩略帧混用。
//!
//! 时间戳单位：**微秒**（`pts_us`，u64，发送端单调时钟）。接收端只用它做端到端
//! 延迟观测，不用来做重排顺序；重排完全依赖 `frame_id`。
//!
//! 丢包/乱序策略：
//! * 分片可乱序到达，按 `frag_index` 重排；重复分片保留先到的一份。
//! * 帧头按 `frame_id` 连续性判断丢帧；发现空洞立即 `needs_keyframe = true`，
//!   后续非关键帧整帧丢弃，直到收到 IDR 才恢复。
//! * 重组窗口 4 帧，超窗淘汰最旧未完成帧，并把状态切回“等 IDR”。
//! * 元数据（`config_id`）变化会清空待重组帧并触发一次重建通知。

use crate::error::ProtocolError;

// =====** 协议常量 =====**

/// 单个视频分片载荷上限（与媒体包明文上限一致，避免 IP 分片）。
pub const VIDEO_FRAGMENT_BYTES: usize = 1352;

/// 单帧原始 Annex-B 上限（32 MiB），防止恶意 `total_len` 撑爆内存。
pub const VIDEO_MAX_FRAME_BYTES: usize = 32 * 1024 * 1024;

/// 同时在等的最大帧数（与 docs/04 重组窗口一致）。
pub const VIDEO_REASSEMBLY_WINDOW: usize = 4;

/// 单个参数集（SPS/PPS）字节上限。
pub const VIDEO_MAX_PARAMETER_SET_BYTES: usize = 512;

/// 主码流编号（与 `StreamKind::VideoMain` 对齐）。
pub const VIDEO_STREAM_MAIN: u32 = 0;

/// 单会话可协商的视频码流上限（含主码流）。
///
/// 多码流协商语义（阶段 5）：
/// * `stream_id = 0` 主码流；`1..VIDEO_MAX_STREAMS` 子码流（另需能力位 `SUB_STREAM`）。
/// * 接收端用 `video_select_stream` 选定一路；未选定表示不过滤（兼容旧调用）。
/// * **分辨率/码率切换不是新消息类型**，而是发送端递增 `config_id` 的新 `Meta`。
///   接收端 `requires_rebuild` 命中后清空在途分片、触发 `ConfigChanged`，并等下一个
///   IDR 才恢复出帧——旧端只要支持 H.264 就能正确重建，无需感知 bit14。
pub const VIDEO_MAX_STREAMS: u32 = 2;

/// 媒体通道单分片的净字节上限（`= FRAGMENT_PAYLOAD_MAX - 13` 字节消息固定头）。
///
/// `VIDEO_FRAGMENT_BYTES` 校验的是分片数据长度，而线上单包限制的是**整条消息**：
/// 13 字节固定头 + 1339 字节数据 = 1352，再加上 32 字节 S4 头与 16 字节 AEAD tag
/// 正好 1400 字节（IPv4/IPv6 路径 MTU 内，不触发 IP 分片）。
/// 发送端切分帧时必须使用本常量；接收端仍接受 `VIDEO_FRAGMENT_BYTES` 以内的消息。
pub const VIDEO_MEDIA_FRAGMENT_BYTES: usize = crate::FRAGMENT_PAYLOAD_MAX - 13;

/// 视频元数据消息 tag。
pub const TAG_VIDEO_META: u8 = 0x61;
/// 视频帧头消息 tag。
pub const TAG_VIDEO_FRAME: u8 = 0x62;
/// 视频分片消息 tag。
pub const TAG_VIDEO_FRAGMENT: u8 = 0x63;
/// 视频流结束/重置信令 tag。
pub const TAG_VIDEO_DONE: u8 = 0x64;

/// 元数据固定部分长度：tag+stream+config+w+h+fps+codec+color+range+flags+sps_len+pps_len。
const META_FIXED_LEN: usize = 1 + 4 + 4 + 2 + 2 + 2 + 1 + 1 + 1 + 1 + 2 + 2;
/// 帧头固定长度：tag+stream+config+frame+pts+flags+total+frag_count。
const FRAME_FIXED_LEN: usize = 1 + 4 + 4 + 4 + 8 + 1 + 4 + 2;
/// 分片固定头长度：tag+stream+frame+index+count。
const FRAGMENT_FIXED_LEN: usize = 1 + 4 + 4 + 2 + 2;
/// Done 长度：tag+stream。
const DONE_LEN: usize = 1 + 4;

const META_FLAG_IN_BAND: u8 = 0x01;
const FRAME_FLAG_KEYFRAME: u8 = 0x01;

// =====** 类型 =====**

/// 视频编码格式（当前只支持 H.264）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    /// H.264 / AVC，Annex-B 字节流。
    H264 = 1,
}

impl VideoCodec {
    fn to_u8(self) -> u8 {
        self as u8
    }

    fn from_u8(value: u8) -> Result<Self, ProtocolError> {
        match value {
            1 => Ok(Self::H264),
            _ => Err(ProtocolError::CodecUnsupported("未知视频编码")),
        }
    }
}

/// 色彩空间（决定 Android `KEY_COLOR_SPACE` 的映射）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoColorSpace {
    /// BT.709。
    Bt709 = 1,
    /// BT.601。
    Bt601 = 2,
    /// sRGB。
    Srgb = 3,
}

impl VideoColorSpace {
    fn to_u8(self) -> u8 {
        self as u8
    }

    fn from_u8(value: u8) -> Result<Self, ProtocolError> {
        match value {
            1 => Ok(Self::Bt709),
            2 => Ok(Self::Bt601),
            3 => Ok(Self::Srgb),
            _ => Err(ProtocolError::InvalidField("未知色彩空间")),
        }
    }
}

/// 色彩范围（有限/全）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoColorRange {
    /// 有限范围（16-235），视频默认。
    Limited = 0,
    /// 全范围（0-255）。
    Full = 1,
}

impl VideoColorRange {
    fn to_u8(self) -> u8 {
        self as u8
    }

    fn from_u8(value: u8) -> Result<Self, ProtocolError> {
        match value {
            0 => Ok(Self::Limited),
            1 => Ok(Self::Full),
            _ => Err(ProtocolError::InvalidField("未知色彩范围")),
        }
    }
}

/// 视频流元数据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoStreamConfig {
    /// 子码流编号（与 `StreamKind::VideoMain` 对齐时为 0）。
    pub stream_id: u32,
    /// 配置版本号：任一元数据变化必须递增，接收端据此触发重建。
    pub config_id: u32,
    /// 编码宽。
    pub width: u16,
    /// 编码高。
    pub height: u16,
    /// 目标帧率。
    pub fps: u16,
    /// 编码格式。
    pub codec: VideoCodec,
    /// 色彩空间。
    pub color_space: VideoColorSpace,
    /// 色彩范围。
    pub color_range: VideoColorRange,
    /// SPS/PPS 是否随每个关键帧的 Annex-B 携带。
    pub parameter_sets_in_band: bool,
    /// 带外 SPS（`parameter_sets_in_band=false` 时使用）。
    pub sps: Vec<u8>,
    /// 带外 PPS。
    pub pps: Vec<u8>,
}

impl VideoStreamConfig {
    /// 元数据是否发生会影响解码器初始化的变化。
    pub fn requires_rebuild(&self, other: &Self) -> bool {
        self.stream_id != other.stream_id
            || self.config_id != other.config_id
            || self.width != other.width
            || self.height != other.height
            || self.fps != other.fps
            || self.codec != other.codec
            || self.color_space != other.color_space
            || self.color_range != other.color_range
            || self.parameter_sets_in_band != other.parameter_sets_in_band
            || self.sps != other.sps
            || self.pps != other.pps
    }
}

/// 视频帧头。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoFrameHeader {
    /// 子码流编号。
    pub stream_id: u32,
    /// 必须与当前元数据的 `config_id` 一致。
    pub config_id: u32,
    /// 帧号（u32 单调递增，允许回绕）。
    pub frame_id: u32,
    /// 采集时间戳（微秒）。
    pub pts_us: u64,
    /// 是否 IDR。
    pub keyframe: bool,
    /// 原始 Annex-B 字节数。
    pub total_len: u32,
    /// 分片总数。
    pub frag_count: u16,
}

/// 视频消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoMessage {
    /// 元数据（宽度/帧率/编码/色彩/SPS/PPS 策略）。
    Meta(VideoStreamConfig),
    /// 帧头。
    Frame(VideoFrameHeader),
    /// 分片。
    Fragment {
        /// 子码流。
        stream_id: u32,
        /// 帧号。
        frame_id: u32,
        /// 分片下标。
        frag_index: u16,
        /// 分片总数。
        frag_count: u16,
        /// 分片字节。
        data: Vec<u8>,
    },
    /// 流结束/重置信令：接收端清空并等下一个 IDR。
    Done {
        /// 子码流。
        stream_id: u32,
    },
}

impl VideoMessage {
    /// 返回该消息所属码流编号（用于多码流过滤）。
    pub fn stream_id(&self) -> u32 {
        match self {
            Self::Meta(config) => config.stream_id,
            Self::Frame(header) => header.stream_id,
            Self::Fragment { stream_id, .. } | Self::Done { stream_id } => *stream_id,
        }
    }
}

// =====** 编码 =====**

/// 编码一条视频消息。
///
/// # Errors
/// 分片超过 [`VIDEO_FRAGMENT_BYTES`]、参数集超过上限或计数非法时返回错误。
pub fn encode_video_message(message: &VideoMessage) -> Result<Vec<u8>, ProtocolError> {
    match message {
        VideoMessage::Meta(config) => {
            if config.sps.len() > VIDEO_MAX_PARAMETER_SET_BYTES
                || config.pps.len() > VIDEO_MAX_PARAMETER_SET_BYTES
            {
                return Err(ProtocolError::PayloadTooLarge {
                    actual: config.sps.len().max(config.pps.len()),
                    max: VIDEO_MAX_PARAMETER_SET_BYTES,
                });
            }
            let mut out = Vec::with_capacity(
                META_FIXED_LEN + config.sps.len() + config.pps.len(),
            );
            out.push(TAG_VIDEO_META);
            out.extend_from_slice(&config.stream_id.to_le_bytes());
            out.extend_from_slice(&config.config_id.to_le_bytes());
            out.extend_from_slice(&config.width.to_le_bytes());
            out.extend_from_slice(&config.height.to_le_bytes());
            out.extend_from_slice(&config.fps.to_le_bytes());
            out.push(config.codec.to_u8());
            out.push(config.color_space.to_u8());
            out.push(config.color_range.to_u8());
            out.push(if config.parameter_sets_in_band {
                META_FLAG_IN_BAND
            } else {
                0
            });
            out.extend_from_slice(&(config.sps.len() as u16).to_le_bytes());
            out.extend_from_slice(&(config.pps.len() as u16).to_le_bytes());
            out.extend_from_slice(&config.sps);
            out.extend_from_slice(&config.pps);
            Ok(out)
        }
        VideoMessage::Frame(header) => {
            if header.total_len as usize > VIDEO_MAX_FRAME_BYTES {
                return Err(ProtocolError::PayloadTooLarge {
                    actual: header.total_len as usize,
                    max: VIDEO_MAX_FRAME_BYTES,
                });
            }
            let mut out = Vec::with_capacity(FRAME_FIXED_LEN);
            out.push(TAG_VIDEO_FRAME);
            out.extend_from_slice(&header.stream_id.to_le_bytes());
            out.extend_from_slice(&header.config_id.to_le_bytes());
            out.extend_from_slice(&header.frame_id.to_le_bytes());
            out.extend_from_slice(&header.pts_us.to_le_bytes());
            out.push(if header.keyframe {
                FRAME_FLAG_KEYFRAME
            } else {
                0
            });
            out.extend_from_slice(&header.total_len.to_le_bytes());
            out.extend_from_slice(&header.frag_count.to_le_bytes());
            Ok(out)
        }
        VideoMessage::Fragment {
            stream_id,
            frame_id,
            frag_index,
            frag_count,
            data,
        } => {
            if data.len() > VIDEO_FRAGMENT_BYTES {
                return Err(ProtocolError::PayloadTooLarge {
                    actual: data.len(),
                    max: VIDEO_FRAGMENT_BYTES,
                });
            }
            if *frag_count == 0 || *frag_index >= *frag_count {
                return Err(ProtocolError::InvalidField("分片下标越界"));
            }
            let mut out = Vec::with_capacity(FRAGMENT_FIXED_LEN + data.len());
            out.push(TAG_VIDEO_FRAGMENT);
            out.extend_from_slice(&stream_id.to_le_bytes());
            out.extend_from_slice(&frame_id.to_le_bytes());
            out.extend_from_slice(&frag_index.to_le_bytes());
            out.extend_from_slice(&frag_count.to_le_bytes());
            out.extend_from_slice(data);
            Ok(out)
        }
        VideoMessage::Done { stream_id } => {
            let mut out = Vec::with_capacity(DONE_LEN);
            out.push(TAG_VIDEO_DONE);
            out.extend_from_slice(&stream_id.to_le_bytes());
            Ok(out)
        }
    }
}

// =====** 解码 =====**

/// 解码一条视频消息。
///
/// # Errors
/// 空载荷、未知 tag、固定字段截断、分片越界或参数集超限返回错误。
pub fn decode_video_message(bytes: &[u8]) -> Result<VideoMessage, ProtocolError> {
    let (&tag, rest) = bytes
        .split_first()
        .ok_or(ProtocolError::InvalidField("空视频消息"))?;
    match tag {
        TAG_VIDEO_META => {
            if rest.len() < META_FIXED_LEN - 1 {
                return Err(ProtocolError::PacketTooShort {
                    actual: rest.len(),
                    min: META_FIXED_LEN - 1,
                });
            }
            let stream_id = read_u32(rest, 0)?;
            let config_id = read_u32(rest, 4)?;
            let width = read_u16(rest, 8)?;
            let height = read_u16(rest, 10)?;
            let fps = read_u16(rest, 12)?;
            let codec = VideoCodec::from_u8(rest[14])?;
            let color_space = VideoColorSpace::from_u8(rest[15])?;
            let color_range = VideoColorRange::from_u8(rest[16])?;
            let flags = rest[17];
            let sps_len = read_u16(rest, 18)? as usize;
            let pps_len = read_u16(rest, 20)? as usize;
            if sps_len > VIDEO_MAX_PARAMETER_SET_BYTES || pps_len > VIDEO_MAX_PARAMETER_SET_BYTES {
                return Err(ProtocolError::InvalidField("参数集超限"));
            }
            let body = &rest[22..];
            if body.len() != sps_len + pps_len {
                return Err(ProtocolError::InvalidField("参数集长度不一致"));
            }
            if width == 0 || height == 0 {
                return Err(ProtocolError::InvalidField("视频尺寸为 0"));
            }
            Ok(VideoMessage::Meta(VideoStreamConfig {
                stream_id,
                config_id,
                width,
                height,
                fps,
                codec,
                color_space,
                color_range,
                parameter_sets_in_band: flags & META_FLAG_IN_BAND != 0,
                sps: body[..sps_len].to_vec(),
                pps: body[sps_len..].to_vec(),
            }))
        }
        TAG_VIDEO_FRAME => {
            if rest.len() != FRAME_FIXED_LEN - 1 {
                return Err(ProtocolError::InvalidField("帧头长度非法"));
            }
            let header = VideoFrameHeader {
                stream_id: read_u32(rest, 0)?,
                config_id: read_u32(rest, 4)?,
                frame_id: read_u32(rest, 8)?,
                pts_us: read_u64(rest, 12)?,
                keyframe: rest[20] & FRAME_FLAG_KEYFRAME != 0,
                total_len: read_u32(rest, 21)?,
                frag_count: read_u16(rest, 25)?,
            };
            if header.frag_count == 0 {
                return Err(ProtocolError::InvalidField("帧分片数为 0"));
            }
            if header.total_len as usize > VIDEO_MAX_FRAME_BYTES {
                return Err(ProtocolError::PayloadTooLarge {
                    actual: header.total_len as usize,
                    max: VIDEO_MAX_FRAME_BYTES,
                });
            }
            Ok(VideoMessage::Frame(header))
        }
        TAG_VIDEO_FRAGMENT => {
            if rest.len() < FRAGMENT_FIXED_LEN - 1 {
                return Err(ProtocolError::PacketTooShort {
                    actual: rest.len(),
                    min: FRAGMENT_FIXED_LEN - 1,
                });
            }
            let stream_id = read_u32(rest, 0)?;
            let frame_id = read_u32(rest, 4)?;
            let frag_index = read_u16(rest, 8)?;
            let frag_count = read_u16(rest, 10)?;
            let data = rest[12..].to_vec();
            if data.len() > VIDEO_FRAGMENT_BYTES {
                return Err(ProtocolError::PayloadTooLarge {
                    actual: data.len(),
                    max: VIDEO_FRAGMENT_BYTES,
                });
            }
            if frag_count == 0 || frag_index >= frag_count {
                return Err(ProtocolError::InvalidField("分片下标越界"));
            }
            Ok(VideoMessage::Fragment {
                stream_id,
                frame_id,
                frag_index,
                frag_count,
                data,
            })
        }
        TAG_VIDEO_DONE => {
            if rest.len() != DONE_LEN - 1 {
                return Err(ProtocolError::InvalidField("视频 Done 长度非法"));
            }
            Ok(VideoMessage::Done {
                stream_id: read_u32(rest, 0)?,
            })
        }
        _ => Err(ProtocolError::InvalidField("未知视频消息 tag")),
    }
}

// =====** 接收侧重排 =====**

/// 接收端事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VideoEvent {
    /// 元数据到达或变更；调用方应重建解码器并重新等待 IDR。
    ConfigChanged(VideoStreamConfig),
    /// 一个完整访问单元（Annex-B）。
    Frame(VideoFrame),
}

/// 已重组的完整帧。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoFrame {
    /// 当前元数据。
    pub config: VideoStreamConfig,
    /// 帧号。
    pub frame_id: u32,
    /// 采集时间戳（微秒）。
    pub pts_us: u64,
    /// 是否 IDR。
    pub keyframe: bool,
    /// Annex-B 字节（带外参数集已按需前置）。
    pub data: Vec<u8>,
}

struct Slot {
    header: VideoFrameHeader,
    shards: Vec<Option<Vec<u8>>>,
}

/// 视频消息接收器：解析、重排、丢包检测与 IDR 重同步。
pub struct VideoReceiver {
    window: usize,
    config: Option<VideoStreamConfig>,
    slots: std::collections::HashMap<u32, Slot>,
    order: std::collections::VecDeque<u32>,
    last_complete: Option<u32>,
    needs_keyframe: bool,
    completed: u64,
    dropped: u64,
    resyncs: u64,
    bytes: u64,
}

impl VideoReceiver {
    /// 创建接收器。
    pub fn new(window: usize) -> Self {
        Self {
            window: window.max(1),
            config: None,
            slots: std::collections::HashMap::new(),
            order: std::collections::VecDeque::new(),
            last_complete: None,
            needs_keyframe: true,
            completed: 0,
            dropped: 0,
            resyncs: 0,
            bytes: 0,
        }
    }

    /// 当前元数据。
    pub fn config(&self) -> Option<&VideoStreamConfig> {
        self.config.as_ref()
    }

    /// 成功重组帧数。
    pub fn completed(&self) -> u64 {
        self.completed
    }

    /// 丢弃帧数（含丢包后等待 IDR 的帧与超窗淘汰）。
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// 因丢包/乱序触发重新等待 IDR 的次数。
    pub fn resyncs(&self) -> u64 {
        self.resyncs
    }

    /// 累计输出字节数。
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// 是否正在等关键帧。
    pub fn needs_keyframe(&self) -> bool {
        self.needs_keyframe
    }

    /// 清空状态并重新等待元数据/IDR。
    pub fn reset(&mut self) {
        self.config = None;
        self.slots.clear();
        self.order.clear();
        self.last_complete = None;
        self.needs_keyframe = true;
    }

    /// 推入一条已解码的视频消息。
    ///
    /// # Errors
    /// 分片与帧头不一致等内部状态错误返回 [`ProtocolError`]；调用方可丢弃该消息。
    pub fn push(&mut self, message: VideoMessage) -> Result<Vec<VideoEvent>, ProtocolError> {
        match message {
            VideoMessage::Meta(config) => {
                let changed = self
                    .config
                    .as_ref()
                    .map_or(true, |old| old.requires_rebuild(&config));
                self.config = Some(config.clone());
                if changed {
                    self.slots.clear();
                    self.order.clear();
                    self.last_complete = None;
                    self.needs_keyframe = true;
                    self.resyncs += 1;
                    return Ok(vec![VideoEvent::ConfigChanged(config)]);
                }
                Ok(Vec::new())
            }
            VideoMessage::Done { .. } => {
                self.slots.clear();
                self.order.clear();
                self.last_complete = None;
                self.needs_keyframe = true;
                self.resyncs += 1;
                Ok(Vec::new())
            }
            VideoMessage::Frame(header) => self.on_frame_header(header),
            VideoMessage::Fragment {
                stream_id,
                frame_id,
                frag_index,
                frag_count,
                data,
            } => self.on_fragment(stream_id, frame_id, frag_index, frag_count, data),
        }
    }

    fn on_frame_header(&mut self, header: VideoFrameHeader) -> Result<Vec<VideoEvent>, ProtocolError> {
        let Some(config) = self.config.as_ref() else {
            return Err(ProtocolError::State("视频元数据未到达"));
        };
        if header.stream_id != config.stream_id || header.config_id != config.config_id {
            // 旧配置的帧：丢弃，等新元数据 + IDR。
            self.dropped += 1;
            return Ok(Vec::new());
        }

        // 帧号空洞 = 中间有帧丢了：立刻切回等 IDR，避免参考链错位花屏。
        if let Some(last) = self.last_complete {
            let gap = header.frame_id.wrapping_sub(last) > 1;
            if gap && !self.needs_keyframe {
                self.needs_keyframe = true;
                self.resyncs += 1;
            }
        }
        if self.needs_keyframe && !header.keyframe {
            self.dropped += 1;
            return Ok(Vec::new());
        }

        if !self.slots.contains_key(&header.frame_id) {
            while self.slots.len() >= self.window {
                if let Some(oldest) = self.order.pop_front() {
                    if self.slots.remove(&oldest).is_some() {
                        self.dropped += 1;
                        self.needs_keyframe = true;
                        self.resyncs += 1;
                    }
                } else {
                    break;
                }
            }
            self.slots.insert(
                header.frame_id,
                Slot {
                    header,
                    shards: vec![None; header.frag_count as usize],
                },
            );
            self.order.push_back(header.frame_id);
        }
        Ok(Vec::new())
    }

    fn on_fragment(
        &mut self,
        stream_id: u32,
        frame_id: u32,
        frag_index: u16,
        frag_count: u16,
        data: Vec<u8>,
    ) -> Result<Vec<VideoEvent>, ProtocolError> {
        if self.config.is_none() {
            return Err(ProtocolError::State("视频元数据未到达"));
        }
        let Some(slot) = self.slots.get_mut(&frame_id) else {
            // 帧头丢失：无法重组，等下一个 IDR。
            if !data.is_empty() {
                self.dropped += 1;
                if !self.needs_keyframe {
                    self.needs_keyframe = true;
                    self.resyncs += 1;
                }
            }
            return Ok(Vec::new());
        };
        if slot.header.stream_id != stream_id
            || slot.header.frag_count != frag_count
            || frag_index as usize >= slot.shards.len()
        {
            return Err(ProtocolError::State("分片与帧头不一致"));
        }
        let index = frag_index as usize;
        if slot.shards[index].is_none() {
            slot.shards[index] = Some(data);
        }
        if slot.shards.iter().any(Option::is_none) {
            return Ok(Vec::new());
        }

        let slot = self.slots.remove(&frame_id).expect("槽位存在");
        self.order.retain(|id| *id != frame_id);
        let mut out = Vec::with_capacity(slot.header.total_len as usize);
        for shard in slot.shards.iter().flatten() {
            out.extend_from_slice(shard);
        }
        out.truncate(slot.header.total_len as usize);
        let config = self.config.clone().expect("已校验");
        if !config.parameter_sets_in_band && slot.header.keyframe {
            out = prepend_parameter_sets(&config, out);
        }
        self.last_complete = Some(frame_id);
        self.needs_keyframe = false;
        self.completed += 1;
        self.bytes += out.len() as u64;
        Ok(vec![VideoEvent::Frame(VideoFrame {
            config,
            frame_id,
            pts_us: slot.header.pts_us,
            keyframe: slot.header.keyframe,
            data: out,
        })])
    }
}

/// 带外 SPS/PPS 前置到关键帧，保证平台解码器拿到 CSD。
fn prepend_parameter_sets(config: &VideoStreamConfig, data: Vec<u8>) -> Vec<u8> {
    if config.sps.is_empty() && config.pps.is_empty() {
        return data;
    }
    let mut out = Vec::with_capacity(
        data.len() + config.sps.len() + config.pps.len() + 8,
    );
    if !config.sps.is_empty() {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&config.sps);
    }
    if !config.pps.is_empty() {
        out.extend_from_slice(&[0, 0, 0, 1]);
        out.extend_from_slice(&config.pps);
    }
    out.extend_from_slice(&data);
    out
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, ProtocolError> {
    bytes
        .get(offset..offset + 2)
        .and_then(|slice| slice.try_into().ok())
        .map(u16::from_le_bytes)
        .ok_or(ProtocolError::PacketTooShort {
            actual: bytes.len(),
            min: offset + 2,
        })
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, ProtocolError> {
    bytes
        .get(offset..offset + 4)
        .and_then(|slice| slice.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or(ProtocolError::PacketTooShort {
            actual: bytes.len(),
            min: offset + 4,
        })
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, ProtocolError> {
    bytes
        .get(offset..offset + 8)
        .and_then(|slice| slice.try_into().ok())
        .map(u64::from_le_bytes)
        .ok_or(ProtocolError::PacketTooShort {
            actual: bytes.len(),
            min: offset + 8,
        })
}

// =====** 自检 =====**

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> VideoStreamConfig {
        VideoStreamConfig {
            stream_id: 0,
            config_id: 1,
            width: 640,
            height: 360,
            fps: 30,
            codec: VideoCodec::H264,
            color_space: VideoColorSpace::Bt709,
            color_range: VideoColorRange::Limited,
            parameter_sets_in_band: true,
            sps: Vec::new(),
            pps: Vec::new(),
        }
    }

    fn frame(id: u32, key: bool, total: u32) -> VideoFrameHeader {
        VideoFrameHeader {
            stream_id: 0,
            config_id: 1,
            frame_id: id,
            pts_us: id as u64 * 33_333,
            keyframe: key,
            total_len: total,
            frag_count: total.div_ceil(VIDEO_FRAGMENT_BYTES as u32) as u16,
        }
    }

    fn fragments(header: &VideoFrameHeader, data: &[u8]) -> Vec<VideoMessage> {
        data.chunks(VIDEO_FRAGMENT_BYTES)
            .enumerate()
            .map(|(index, chunk)| VideoMessage::Fragment {
                stream_id: header.stream_id,
                frame_id: header.frame_id,
                frag_index: index as u16,
                frag_count: header.frag_count,
                data: chunk.to_vec(),
            })
            .collect()
    }

    fn roundtrip(message: &VideoMessage) -> VideoMessage {
        decode_video_message(&encode_video_message(message).expect("编码")).expect("解码")
    }

    #[test]
    fn meta_roundtrip_and_limits() {
        let mut meta = config();
        meta.sps = vec![0x42; 20];
        meta.pps = vec![0xCE; 8];
        assert_eq!(roundtrip(&VideoMessage::Meta(meta.clone())), VideoMessage::Meta(meta));

        let mut too_big = config();
        too_big.sps = vec![0; VIDEO_MAX_PARAMETER_SET_BYTES + 1];
        assert!(encode_video_message(&VideoMessage::Meta(too_big)).is_err());
    }

    #[test]
    fn frame_header_and_fragment_roundtrip() {
        let header = frame(9, true, 1400);
        assert_eq!(roundtrip(&VideoMessage::Frame(header)), VideoMessage::Frame(header));

        let fragment = VideoMessage::Fragment {
            stream_id: 0,
            frame_id: 9,
            frag_index: 1,
            frag_count: 2,
            data: vec![0xAB; 48],
        };
        assert_eq!(roundtrip(&fragment), fragment);
    }

    #[test]
    fn oversized_fragment_is_rejected() {
        let fragment = VideoMessage::Fragment {
            stream_id: 0,
            frame_id: 1,
            frag_index: 0,
            frag_count: 1,
            data: vec![0; VIDEO_FRAGMENT_BYTES + 1],
        };
        assert!(encode_video_message(&fragment).is_err());
    }

    #[test]
    fn receiver_reassembles_out_of_order_fragments() {
        let mut rx = VideoReceiver::new(VIDEO_REASSEMBLY_WINDOW);
        let events = rx.push(VideoMessage::Meta(config())).expect("元数据");
        assert_eq!(events.len(), 1);

        let data: Vec<u8> = (0..2000).map(|i| (i % 251) as u8).collect();
        let header = frame(1, true, data.len() as u32);
        assert!(rx.push(VideoMessage::Frame(header)).unwrap().is_empty());
        let mut parts = fragments(&header, &data);
        parts.reverse();
        let mut out = None;
        for part in parts {
            let events = rx.push(part).expect("分片");
            if let Some(VideoEvent::Frame(frame)) = events.into_iter().next() {
                out = Some(frame);
            }
        }
        let frame = out.expect("应重组完成");
        assert_eq!(frame.data, data);
        assert!(frame.keyframe);
        assert_eq!(rx.completed(), 1);
    }

    #[test]
    fn gap_waits_for_next_idr() {
        let mut rx = VideoReceiver::new(VIDEO_REASSEMBLY_WINDOW);
        rx.push(VideoMessage::Meta(config())).unwrap();
        let key = frame(1, true, 10);
        rx.push(VideoMessage::Frame(key)).unwrap();
        for part in fragments(&key, &[1u8; 10]) {
            rx.push(part).unwrap();
        }
        assert_eq!(rx.completed(), 1);

        // 帧 2 完全丢失，直接来帧 3 的 P 帧。
        let p = frame(3, false, 10);
        rx.push(VideoMessage::Frame(p)).unwrap();
        assert!(rx.needs_keyframe(), "空洞后必须等 IDR");
        assert_eq!(rx.dropped(), 1);

        // 后面的 P 帧仍被拒绝。
        let p4 = frame(4, false, 10);
        rx.push(VideoMessage::Frame(p4)).unwrap();
        assert_eq!(rx.dropped(), 2);

        // 新 IDR 到达后恢复。
        let key5 = frame(5, true, 10);
        rx.push(VideoMessage::Frame(key5)).unwrap();
        for part in fragments(&key5, &[5u8; 10]) {
            rx.push(part).unwrap();
        }
        assert_eq!(rx.completed(), 2);
        assert!(!rx.needs_keyframe());
        assert!(rx.resyncs() >= 1);
    }

    #[test]
    fn config_change_triggers_rebuild_and_reset() {
        let mut rx = VideoReceiver::new(VIDEO_REASSEMBLY_WINDOW);
        rx.push(VideoMessage::Meta(config())).unwrap();
        let key = frame(1, true, 10);
        rx.push(VideoMessage::Frame(key)).unwrap();
        for part in fragments(&key, &[1u8; 10]) {
            rx.push(part).unwrap();
        }

        let mut next = config();
        next.config_id = 2;
        next.width = 1280;
        let events = rx.push(VideoMessage::Meta(next.clone())).unwrap();
        assert_eq!(events, vec![VideoEvent::ConfigChanged(next)]);
        assert!(rx.needs_keyframe());
    }

    #[test]
    fn out_of_band_parameter_sets_are_prepended_to_idr() {
        let mut rx = VideoReceiver::new(VIDEO_REASSEMBLY_WINDOW);
        let mut meta = config();
        meta.parameter_sets_in_band = false;
        meta.sps = vec![0x42, 0x00];
        meta.pps = vec![0xCE, 0x3C];
        rx.push(VideoMessage::Meta(meta)).unwrap();

        let key = frame(1, true, 4);
        rx.push(VideoMessage::Frame(key)).unwrap();
        for part in fragments(&key, &[5u8; 4]) {
            if let Some(VideoEvent::Frame(frame)) = rx.push(part).unwrap().into_iter().next() {
                assert_eq!(
                    frame.data,
                    vec![0, 0, 0, 1, 0x42, 0x00, 0, 0, 0, 1, 0xCE, 0x3C, 5, 5, 5, 5]
                );
                return;
            }
        }
        panic!("未收到帧");
    }
}
