//! rdc-core / S5 屏幕墙低码率子码流
//!
//! 层级：L3 编排层
//! 用途：生成 480p / 10 fps 的缩略帧，复用到多条会话（观看者），并提供
//!       「给定 N 路会话，取每路最新缩略帧」的接口。
//! 调用方：rdc-cli 的 `demo-wall`；后续 rdc-bridge（纹理句柄由 UI 层填）。
//!
//! # 编码实例选择（明确写出）
//!
//! 采用 **一个共享的缩略图编码器**（选项 B）：进程内只创建 **1 个** H.264 编码实例，
//! 以 480p / 10 fps / 低码率参数编码；N 路观看者复用这同一份编码输出，只做分片分发，
//! **不随观看者数量增加编码实例**。
//!
//! 为什么不用「从主编码器输出缩放」（选项 A）：主码流是已压缩的 H.264 码流，
//! 「缩放」必须先解码再缩放再加码，等于额外引入一条解码器依赖，代价大于单实例缩略编码。
//!
//! 代价（如实记录）：
//! 1. 所有观看者共享同一份分辨率/帧率/质量参数，不能按观看者单独调整；
//! 2. 仍额外占 1 个编码实例（不随 N 增长）；硬件编码实例上限很紧时需降级为快照模式；
//! 3. 缩放采用最近邻（RGBA 直采），每帧成本 O(480p)，与 N 无关；代价是画质较差；
//! 4. 与 docs/04 §2.3「每路独立编码实例」的既定约束冲突，已记 D41 待裁决。
//!
//! # 通道与分片
//!
//! 复用已加密的控制流（AEAD + 重放窗口），单条消息 ≤ 4 KiB；缩略帧按
//! [`WALL_FRAGMENT_BYTES`] 切分，先发帧头（时间戳/尺寸/总长）再发分片。
//! 时间戳由 host 用「进程单调时钟」生成，端到端预览延迟 = 接收时刻 − 帧时间戳。

use rdc_codec::{CodecKind, EncodedPacket, VideoChroma, VideoConfig, VideoEncoder};
use rdc_connect::{ConnectError, EstablishedSession};
use rdc_runtime::FramePool;

// =====** 参数 =====**

/// 子码流宽度（480p 16:9，16 像素对齐，满足 NV12/编码器块约束）。
pub const WALL_WIDTH: u32 = 848;
/// 子码流高度。
pub const WALL_HEIGHT: u32 = 480;
/// 子码流帧率（docs/04 §2.3 要求 5~10 fps）。
pub const WALL_FPS: u32 = 10;
/// 子码流目标码率（400 kbps，落在 0.2~0.4 Mbps 区间上沿）。
pub const WALL_BITRATE_BPS: u32 = 400_000;
/// 缩略帧分片载荷上限（单条控制消息 ≤ 4 KiB，留足余量）。
pub const WALL_FRAGMENT_BYTES: usize = 1352;

/// 帧头消息 tag。
const TAG_FRAME_HEADER: u8 = 0x51;
/// 分片消息 tag。
const TAG_FRAME_FRAGMENT: u8 = 0x52;
/// 帧头固定长度：tag(1)+stream(4)+frame(4)+pts(8)+w(2)+h(2)+total(4)。
const HEADER_LEN: usize = 1 + 4 + 4 + 8 + 2 + 2 + 4;
/// 分片固定头：tag(1)+stream(4)+frame(4)+index(2)+count(2)。
const FRAGMENT_HEADER_LEN: usize = 1 + 4 + 4 + 2 + 2;

// =====** WallError =====**

/// 屏幕墙错误。
#[derive(Debug, thiserror::Error)]
pub enum WallError {
    /// 会话通道收发失败。
    #[error("会话通道失败：{0}")]
    Link(#[from] ConnectError),
    /// 缩略图编码失败。
    #[error("缩略图编码失败：{0}")]
    Codec(#[from] rdc_codec::CodecError),
    /// 消息格式或状态机不符合预期。
    #[error("屏幕墙协议错误：{0}")]
    Protocol(String),
}

// =====** WallFrame =====**

/// 一帧缩略图（编码后的字节 + 时间戳 + 尺寸）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WallFrame {
    /// 子码流编号（同一会话内用于区分多路墙流）。
    pub stream_id: u32,
    /// 帧号（单调递增）。
    pub frame_id: u32,
    /// 帧时间戳（host 进程单调时钟，单位：微秒）。
    pub pts_us: u64,
    /// 编码后宽。
    pub width: u16,
    /// 编码后高。
    pub height: u16,
    /// 编码字节（H.264 Annex-B 访问单元）。
    pub data: Vec<u8>,
}

// =====** WallMessage =====**

/// 子码流消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WallMessage {
    /// 帧头：告知接下来的分片属于哪一帧、总长与时间戳。
    Header {
        /// 子码流编号。
        stream_id: u32,
        /// 帧号。
        frame_id: u32,
        /// 时间戳（微秒）。
        pts_us: u64,
        /// 编码宽。
        width: u16,
        /// 编码高。
        height: u16,
        /// 编码总字节数。
        total_len: u32,
    },
    /// 帧分片。
    Fragment {
        /// 子码流编号。
        stream_id: u32,
        /// 帧号（必须与最近一个帧头一致）。
        frame_id: u32,
        /// 分片下标（从 0 开始，必须连续）。
        frag_index: u16,
        /// 该帧分片总数。
        frag_count: u16,
        /// 分片内容。
        data: Vec<u8>,
    },
}

/// 编码一条子码流消息。
///
/// # Arguments
/// * `message` - 待编码消息
///
/// # Returns
/// 明文字节（加密由 `send_control` 完成）。
///
/// # Errors
/// 分片超过 [`WALL_FRAGMENT_BYTES`] 或字段越界时返回 [`WallError::Protocol`]。
pub fn encode_message(message: &WallMessage) -> Result<Vec<u8>, WallError> {
    let mut out = Vec::new();
    match message {
        WallMessage::Header {
            stream_id,
            frame_id,
            pts_us,
            width,
            height,
            total_len,
        } => {
            out.push(TAG_FRAME_HEADER);
            out.extend_from_slice(&stream_id.to_le_bytes());
            out.extend_from_slice(&frame_id.to_le_bytes());
            out.extend_from_slice(&pts_us.to_le_bytes());
            out.extend_from_slice(&width.to_le_bytes());
            out.extend_from_slice(&height.to_le_bytes());
            out.extend_from_slice(&total_len.to_le_bytes());
            debug_assert_eq!(out.len(), HEADER_LEN);
        }
        WallMessage::Fragment {
            stream_id,
            frame_id,
            frag_index,
            frag_count,
            data,
        } => {
            if data.len() > WALL_FRAGMENT_BYTES {
                return Err(WallError::Protocol(format!(
                    "分片 {} 字节超过上限 {WALL_FRAGMENT_BYTES}",
                    data.len()
                )));
            }
            out.push(TAG_FRAME_FRAGMENT);
            out.extend_from_slice(&stream_id.to_le_bytes());
            out.extend_from_slice(&frame_id.to_le_bytes());
            out.extend_from_slice(&frag_index.to_le_bytes());
            out.extend_from_slice(&frag_count.to_le_bytes());
            out.extend_from_slice(data);
            debug_assert_eq!(out.len(), FRAGMENT_HEADER_LEN + data.len());
        }
    }
    Ok(out)
}

/// 解码一条子码流消息。
///
/// # Arguments
/// * `bytes` - 明文消息
///
/// # Returns
/// 解析后的消息。
///
/// # Errors
/// 长度不足、未知 tag 或分片越界时返回 [`WallError::Protocol`]。
pub fn decode_message(bytes: &[u8]) -> Result<WallMessage, WallError> {
    let (&tag, rest) = bytes
        .split_first()
        .ok_or_else(|| WallError::Protocol("空消息".to_owned()))?;
    match tag {
        TAG_FRAME_HEADER => {
            if rest.len() != HEADER_LEN - 1 {
                return Err(WallError::Protocol(format!(
                    "帧头长度 {} != {}",
                    rest.len() + 1,
                    HEADER_LEN
                )));
            }
            let stream_id = read_u32(rest, 0)?;
            let frame_id = read_u32(rest, 4)?;
            let pts_us = read_u64(rest, 8)?;
            let width = read_u16(rest, 16)?;
            let height = read_u16(rest, 18)?;
            let total_len = read_u32(rest, 20)?;
            Ok(WallMessage::Header {
                stream_id,
                frame_id,
                pts_us,
                width,
                height,
                total_len,
            })
        }
        TAG_FRAME_FRAGMENT => {
            if rest.len() < FRAGMENT_HEADER_LEN - 1 {
                return Err(WallError::Protocol("分片头截断".to_owned()));
            }
            let stream_id = read_u32(rest, 0)?;
            let frame_id = read_u32(rest, 4)?;
            let frag_index = read_u16(rest, 8)?;
            let frag_count = read_u16(rest, 10)?;
            let data = rest[12..].to_vec();
            if data.len() > WALL_FRAGMENT_BYTES {
                return Err(WallError::Protocol(format!(
                    "分片 {} 字节超过上限 {WALL_FRAGMENT_BYTES}",
                    data.len()
                )));
            }
            if frag_count == 0 || frag_index >= frag_count {
                return Err(WallError::Protocol(format!(
                    "分片下标 {frag_index}/{frag_count} 越界"
                )));
            }
            Ok(WallMessage::Fragment {
                stream_id,
                frame_id,
                frag_index,
                frag_count,
                data,
            })
        }
        other => Err(WallError::Protocol(format!("未知 tag {other}"))),
    }
}

/// 读取小端 u32。
fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, WallError> {
    bytes
        .get(offset..offset + 4)
        .and_then(|slice| slice.try_into().ok())
        .map(u32::from_le_bytes)
        .ok_or_else(|| WallError::Protocol(format!("读取 u32 @{offset} 越界")))
}

/// 读取小端 u64。
fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, WallError> {
    bytes
        .get(offset..offset + 8)
        .and_then(|slice| slice.try_into().ok())
        .map(u64::from_le_bytes)
        .ok_or_else(|| WallError::Protocol(format!("读取 u64 @{offset} 越界")))
}

/// 读取小端 u16。
fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, WallError> {
    bytes
        .get(offset..offset + 2)
        .and_then(|slice| slice.try_into().ok())
        .map(u16::from_le_bytes)
        .ok_or_else(|| WallError::Protocol(format!("读取 u16 @{offset} 越界")))
}

// =====** 发送 =====**

/// 向一路会话发送一帧缩略图（帧头 + 连续分片）。
///
/// # Arguments
/// * `session` - 已加密会话
/// * `frame` - 待发送帧
///
/// # Returns
/// 实际发送的分片数。
///
/// # Errors
/// 通道写失败或消息编码失败时返回 [`WallError`]。
pub async fn send_frame(
    session: &mut EstablishedSession,
    frame: &WallFrame,
) -> Result<usize, WallError> {
    let total_len = u32::try_from(frame.data.len())
        .map_err(|_| WallError::Protocol("帧长度超过 u32".to_owned()))?;
    let header = encode_message(&WallMessage::Header {
        stream_id: frame.stream_id,
        frame_id: frame.frame_id,
        pts_us: frame.pts_us,
        width: frame.width,
        height: frame.height,
        total_len,
    })?;
    session.send_control(&header).await?;
    let frag_count = frame.data.len().div_ceil(WALL_FRAGMENT_BYTES);
    let frag_count_u16 = u16::try_from(frag_count)
        .map_err(|_| WallError::Protocol("分片数超过 u16".to_owned()))?;
    for (index, chunk) in frame.data.chunks(WALL_FRAGMENT_BYTES).enumerate() {
        let message = WallMessage::Fragment {
            stream_id: frame.stream_id,
            frame_id: frame.frame_id,
            frag_index: index as u16,
            frag_count: frag_count_u16,
            data: chunk.to_vec(),
        };
        session.send_control(&encode_message(&message)?).await?;
    }
    Ok(frag_count)
}

// =====** 接收重组 =====**

/// 正在重组的帧。
#[derive(Debug)]
struct PendingFrame {
    /// 帧号。
    frame_id: u32,
    /// 时间戳（微秒）。
    pts_us: u64,
    /// 宽度。
    width: u16,
    /// 高度。
    height: u16,
    /// 期望总长。
    total_len: u32,
    /// 已收到的分片数。
    received: u16,
    /// 分片总数（由首个分片给出）。
    frag_count: u16,
    /// 已拼接数据。
    data: Vec<u8>,
}

/// 观看者侧的子码流重组器。
#[derive(Debug)]
pub struct WallReceiver {
    /// 本路子码流编号（只接受同编号消息，避免多流串扰）。
    stream_id: u32,
    /// 在途帧。
    pending: Option<PendingFrame>,
    /// 完整帧计数。
    completed: u64,
    /// 因新帧头而丢弃的未完成帧计数。
    dropped: u64,
}

impl WallReceiver {
    /// 构造重组器。
    ///
    /// # Arguments
    /// * `stream_id` - 本路子码流编号
    ///
    /// # Returns
    /// 空重组器。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new(stream_id: u32) -> Self {
        Self {
            stream_id,
            pending: None,
            completed: 0,
            dropped: 0,
        }
    }

    /// 完整帧计数。
    ///
    /// # Returns
    /// 已重组成的帧数。
    ///
    /// # Errors
    /// 不会失败。
    pub fn completed(&self) -> u64 {
        self.completed
    }

    /// 丢弃计数（新帧头覆盖未完成帧）。
    ///
    /// # Returns
    /// 丢弃的帧数。
    ///
    /// # Errors
    /// 不会失败。
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// 接收直到重组成一帧。
    ///
    /// # Arguments
    /// * `session` - 已加密会话（必须由本重组器独占使用）
    ///
    /// # Returns
    /// 完整缩略帧。
    ///
    /// # Errors
    /// 通道读失败、消息畸形或分片乱序时返回 [`WallError`]。
    pub async fn recv_frame(
        &mut self,
        session: &mut EstablishedSession,
    ) -> Result<WallFrame, WallError> {
        loop {
            let message = decode_message(&session.recv_control().await?)?;
            if let Some(frame) = self.apply(message)? {
                return Ok(frame);
            }
        }
    }

    /// 应用一条消息；重组成完整帧时返回它。
    ///
    /// # Arguments
    /// * `message` - 解析后的消息
    ///
    /// # Returns
    /// 完整帧或 `None`。
    ///
    /// # Errors
    /// 分片与帧头不一致（乱序/越界）时返回 [`WallError::Protocol`]。
    pub fn apply(&mut self, message: WallMessage) -> Result<Option<WallFrame>, WallError> {
        match message {
            WallMessage::Header {
                stream_id,
                frame_id,
                pts_us,
                width,
                height,
                total_len,
            } => {
                if stream_id != self.stream_id {
                    return Ok(None);
                }
                if self.pending.is_some() {
                    // 单条控制流有序，正常不会跨帧；出现即上一帧不完整，计入丢弃。
                    self.dropped += 1;
                }
                if width == 0 || height == 0 {
                    return Err(WallError::Protocol("帧尺寸为 0".to_owned()));
                }
                self.pending = Some(PendingFrame {
                    frame_id,
                    pts_us,
                    width,
                    height,
                    total_len,
                    received: 0,
                    frag_count: 0,
                    data: Vec::with_capacity(total_len as usize),
                });
                // 空帧（total_len=0）立即完成。
                if total_len == 0 {
                    return self.finish_pending();
                }
                Ok(None)
            }
            WallMessage::Fragment {
                stream_id,
                frame_id,
                frag_index,
                frag_count,
                data,
            } => {
                if stream_id != self.stream_id {
                    return Ok(None);
                }
                let Some(pending) = self.pending.as_mut() else {
                    return Err(WallError::Protocol("分片先于帧头到达".to_owned()));
                };
                if pending.frame_id != frame_id {
                    return Err(WallError::Protocol(format!(
                        "分片帧号 {} 与帧头 {} 不一致",
                        frame_id, pending.frame_id
                    )));
                }
                if frag_index != pending.received {
                    return Err(WallError::Protocol(format!(
                        "分片乱序：期望 {}，收到 {frag_index}",
                        pending.received
                    )));
                }
                if pending.frag_count != 0 && pending.frag_count != frag_count {
                    return Err(WallError::Protocol("同一帧分片总数不一致".to_owned()));
                }
                pending.frag_count = frag_count;
                pending.received += 1;
                pending.data.extend_from_slice(&data);
                if pending.data.len() > pending.total_len as usize {
                    return Err(WallError::Protocol("重组长度超过帧头声明".to_owned()));
                }
                if pending.received == pending.frag_count
                    && pending.data.len() == pending.total_len as usize
                {
                    return self.finish_pending();
                }
                Ok(None)
            }
        }
    }

    /// 把在途帧转成 [`WallFrame`]。
    fn finish_pending(&mut self) -> Result<Option<WallFrame>, WallError> {
        let Some(pending) = self.pending.take() else {
            return Ok(None);
        };
        if pending.data.len() != pending.total_len as usize {
            return Err(WallError::Protocol(format!(
                "帧长度 {} != 声明 {}",
                pending.data.len(),
                pending.total_len
            )));
        }
        self.completed += 1;
        Ok(Some(WallFrame {
            stream_id: self.stream_id,
            frame_id: pending.frame_id,
            pts_us: pending.pts_us,
            width: pending.width,
            height: pending.height,
            data: pending.data,
        }))
    }
}

// =====** 最新帧接口 =====**

/// 一路会话的最新缩略帧摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewFrame {
    /// 子码流编号。
    pub stream_id: u32,
    /// 帧号。
    pub frame_id: u32,
    /// 帧时间戳（host 微秒）。
    pub pts_us: u64,
    /// 宽。
    pub width: u16,
    /// 高。
    pub height: u16,
    /// 编码字节数。
    pub bytes: usize,
    /// 纹理句柄（UI 层上屏用；无 UI 时为空）。
    pub texture_id: Option<u32>,
}

impl PreviewFrame {
    /// 由完整帧构造摘要。
    ///
    /// # Arguments
    /// * `frame` - 完整缩略帧
    /// * `texture_id` - 可选纹理句柄
    ///
    /// # Returns
    /// 摘要。
    ///
    /// # Errors
    /// 不会失败。
    pub fn from_frame(frame: &WallFrame, texture_id: Option<u32>) -> Self {
        Self {
            stream_id: frame.stream_id,
            frame_id: frame.frame_id,
            pts_us: frame.pts_us,
            width: frame.width,
            height: frame.height,
            bytes: frame.data.len(),
            texture_id,
        }
    }
}

/// 屏幕墙预览汇总：给定 N 路会话，取每路最新缩略帧。
#[derive(Debug, Clone, Default)]
pub struct WallPreviewHub {
    /// 流编号注册顺序（决定 `latest` 的返回顺序）。
    streams: Vec<u32>,
    /// 每路最新帧。
    latest: std::collections::BTreeMap<u32, PreviewFrame>,
}

impl WallPreviewHub {
    /// 注册 N 路流。
    ///
    /// # Arguments
    /// * `stream_ids` - 流编号列表（重复项被忽略）
    ///
    /// # Returns
    /// 空汇总。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new(stream_ids: &[u32]) -> Self {
        let mut hub = Self::default();
        for id in stream_ids {
            if !hub.streams.contains(id) {
                hub.streams.push(*id);
            }
        }
        hub
    }

    /// 更新某一路的最新帧。
    ///
    /// # Arguments
    /// * `frame` - 完整缩略帧
    ///
    /// # Returns
    /// 是否属于已注册的流（未注册流被忽略）。
    ///
    /// # Errors
    /// 不会失败。
    pub fn update(&mut self, frame: &WallFrame) -> bool {
        if !self.streams.contains(&frame.stream_id) {
            return false;
        }
        self.latest
            .insert(frame.stream_id, PreviewFrame::from_frame(frame, None));
        true
    }

    /// 返回每路最新帧（按注册顺序；尚无帧的位置为 `None`）。
    ///
    /// # Returns
    /// 与 `streams` 等长的向量。
    ///
    /// # Errors
    /// 不会失败。
    pub fn latest(&self) -> Vec<Option<PreviewFrame>> {
        self.streams
            .iter()
            .map(|id| self.latest.get(id).cloned())
            .collect()
    }

    /// 已注册的流数。
    ///
    /// # Returns
    /// 流条数。
    ///
    /// # Errors
    /// 不会失败。
    pub fn len(&self) -> usize {
        self.streams.len()
    }

    /// 是否没有流。
    ///
    /// # Returns
    /// 无流时返回 `true`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn is_empty(&self) -> bool {
        self.streams.is_empty()
    }
}

// =====** 缩放 =====**

/// 最近邻缩放 RGBA（源 → 目标）。
///
/// 为什么用最近邻：这是「从采集帧缩放」这条路上的固定成本，最近邻每帧只读一次
/// 目标像素，480p 约 40 万像素；双线性会翻 4 倍读取且对缩略图收益有限。
///
/// # Arguments
/// * `src` - 源 RGBA 像素
/// * `src_w` / `src_h` - 源尺寸
/// * `dst` - 目标缓冲（长度必须为 `dst_w * dst_h * 4`）
/// * `dst_w` / `dst_h` - 目标尺寸
///
/// # Returns
/// 成功返回 `Ok(())`。
///
/// # Errors
/// 尺寸为 0 或缓冲长度不符时返回 [`WallError::Protocol`]。
pub fn downscale_rgba(
    src: &[u8],
    src_w: u32,
    src_h: u32,
    dst: &mut [u8],
    dst_w: u32,
    dst_h: u32,
) -> Result<(), WallError> {
    let expect_src = src_w as usize * src_h as usize * 4;
    let expect_dst = dst_w as usize * dst_h as usize * 4;
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return Err(WallError::Protocol("缩放尺寸为 0".to_owned()));
    }
    if src.len() < expect_src {
        return Err(WallError::Protocol(format!(
            "源缓冲 {} < 期望 {expect_src}",
            src.len()
        )));
    }
    if dst.len() < expect_dst {
        return Err(WallError::Protocol(format!(
            "目标缓冲 {} < 期望 {expect_dst}",
            dst.len()
        )));
    }
    let src_stride = src_w as usize * 4;
    let dst_stride = dst_w as usize * 4;
    for y in 0..dst_h as usize {
        let sy = (y as u64 * src_h as u64 / dst_h as u64) as usize;
        let src_row = sy * src_stride;
        let dst_row = y * dst_stride;
        for x in 0..dst_w as usize {
            let sx = (x as u64 * src_w as u64 / dst_w as u64) as usize;
            let s = src_row + sx * 4;
            let d = dst_row + x * 4;
            dst[d..d + 4].copy_from_slice(&src[s..s + 4]);
        }
    }
    Ok(())
}

// =====** 共享缩略图编码器 =====**

/// 进程内唯一的缩略图编码器（多路观看者共享）。
///
/// 生命周期：创建 → 每帧 [`ThumbnailEncoder::encode`] → 结束时 [`ThumbnailEncoder::flush`]。
/// 只创建 **1 个** 编码实例，与观看者数量无关（见模块注释的代价说明）。
pub struct ThumbnailEncoder {
    /// 唯一编码实例。
    encoder: Box<dyn VideoEncoder>,
    /// 目标尺寸缓冲池（RGBA）。
    pool: FramePool,
    /// 目标宽。
    width: u32,
    /// 目标高。
    height: u32,
    /// 已产出包数。
    packets: u64,
}

impl ThumbnailEncoder {
    /// 创建并配置共享编码器。
    ///
    /// # Arguments
    /// * `width` / `height` - 目标尺寸
    /// * `fps` - 目标帧率
    /// * `bitrate_bps` - 目标码率
    ///
    /// # Returns
    /// 编码器。
    ///
    /// # Errors
    /// 无可用编码器或参数不被接受时返回 [`WallError::Codec`]。
    pub fn new(
        width: u32,
        height: u32,
        fps: u32,
        bitrate_bps: u32,
    ) -> Result<Self, WallError> {
        let mut encoder = rdc_codec::create_encoder(CodecKind::H264)?;
        let config = VideoConfig {
            codec: CodecKind::H264,
            width,
            height,
            fps,
            bitrate_bps,
            chroma: VideoChroma::Yuv420,
            bit_depth: 8,
            low_latency: true,
        };
        // 低码率参数在个别驱动上会被拒绝；失败时回退到 1 Mbps 并保留原分辨率/帧率。
        if let Err(first) = encoder.configure(config) {
            let fallback = VideoConfig {
                bitrate_bps: bitrate_bps.max(1_000_000),
                ..config
            };
            encoder.configure(fallback).map_err(|_| first)?;
        }
        let block = width as usize * height as usize * 4;
        Ok(Self {
            encoder,
            pool: FramePool::new(4, block),
            width,
            height,
            packets: 0,
        })
    }

    /// 编码一帧：缩放 → 提交 → 取回可用包。
    ///
    /// # Arguments
    /// * `src` - 源 RGBA（整帧）
    /// * `src_w` / `src_h` - 源尺寸
    /// * `pts_ns` - 该帧采集时间戳（纳秒）
    ///
    /// # Returns
    /// 本次取回的编码包（硬件流水线可能为空）。
    ///
    /// # Errors
    /// 缓冲池耗尽、缩放失败或提交失败时返回 [`WallError`]。
    pub fn encode(
        &mut self,
        src: &[u8],
        src_w: u32,
        src_h: u32,
        pts_ns: u64,
    ) -> Result<Vec<EncodedPacket>, WallError> {
        let Some(mut buffer) = self.pool.acquire() else {
            return Err(WallError::Protocol("缩略图缓冲池耗尽".to_owned()));
        };
        downscale_rgba(
            src,
            src_w,
            src_h,
            buffer.as_mut_slice(),
            self.width,
            self.height,
        )?;
        self.encoder.push(buffer, pts_ns, false)?;
        let mut out = Vec::new();
        while let Some(packet) = self.encoder.poll_packet() {
            out.push(packet);
        }
        self.packets += out.len() as u64;
        Ok(out)
    }

    /// 冲刷流水线，取回滞留帧（结束前调用）。
    ///
    /// # Returns
    /// 剩余编码包。
    ///
    /// # Errors
    /// 冲刷失败时返回 [`WallError::Codec`]。
    pub fn flush(&mut self) -> Result<Vec<EncodedPacket>, WallError> {
        let packets = self.encoder.flush()?;
        self.packets += packets.len() as u64;
        Ok(packets)
    }

    /// 已产出包数。
    ///
    /// # Returns
    /// 编码包累计数。
    ///
    /// # Errors
    /// 不会失败。
    pub fn packets(&self) -> u64 {
        self.packets
    }

    /// 后端名称（写入报告）。
    ///
    /// # Returns
    /// 平台后端标识。
    ///
    /// # Errors
    /// 不会失败。
    pub fn backend_name(&self) -> &str {
        self.encoder.backend_name()
    }

    /// 目标尺寸。
    ///
    /// # Returns
    /// `(宽, 高)`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 帧头/分片编解码往返。
    #[test]
    fn message_roundtrip() {
        let header = WallMessage::Header {
            stream_id: 7,
            frame_id: 42,
            pts_us: 1_234_567,
            width: WALL_WIDTH as u16,
            height: WALL_HEIGHT as u16,
            total_len: 3000,
        };
        let bytes = encode_message(&header).expect("编码");
        assert_eq!(bytes.len(), HEADER_LEN);
        assert_eq!(decode_message(&bytes).expect("解码"), header);

        let fragment = WallMessage::Fragment {
            stream_id: 7,
            frame_id: 42,
            frag_index: 1,
            frag_count: 3,
            data: vec![0xA5; 100],
        };
        let bytes = encode_message(&fragment).expect("编码");
        assert_eq!(bytes.len(), FRAGMENT_HEADER_LEN + 100);
        assert_eq!(decode_message(&bytes).expect("解码"), fragment);
    }

    /// 畸形消息一律 Protocol，不 panic。
    #[test]
    fn malformed_messages_rejected() {
        assert!(decode_message(&[]).is_err());
        assert!(decode_message(&[0x99]).is_err());
        assert!(decode_message(&[TAG_FRAME_HEADER]).is_err());
        let too_big = WallMessage::Fragment {
            stream_id: 1,
            frame_id: 1,
            frag_index: 0,
            frag_count: 2,
            data: vec![0u8; WALL_FRAGMENT_BYTES + 1],
        };
        assert!(encode_message(&too_big).is_err());
        let bad_index = WallMessage::Fragment {
            stream_id: 1,
            frame_id: 1,
            frag_index: 2,
            frag_count: 2,
            data: vec![0u8; 10],
        };
        let bytes = bytes_with_bad_index(&bad_index);
        assert!(decode_message(&bytes).is_err());
    }

    /// 构造一个分片下标越界的合法长度消息（绕过 encode 的校验）。
    fn bytes_with_bad_index(fragment: &WallMessage) -> Vec<u8> {
        let WallMessage::Fragment {
            stream_id,
            frame_id,
            frag_index,
            frag_count,
            data,
        } = fragment
        else {
            unreachable!()
        };
        let mut out = vec![TAG_FRAME_FRAGMENT];
        out.extend_from_slice(&stream_id.to_le_bytes());
        out.extend_from_slice(&frame_id.to_le_bytes());
        out.extend_from_slice(&frag_index.to_le_bytes());
        out.extend_from_slice(&frag_count.to_le_bytes());
        out.extend_from_slice(data);
        out
    }

    /// 重组：帧头 + 连续分片 → 完整帧；乱序分片报错。
    #[test]
    fn receiver_reassembles_in_order() {
        let mut receiver = WallReceiver::new(3);
        let payload = vec![0x5A; 2705];
        let frag_count = payload.len().div_ceil(WALL_FRAGMENT_BYTES);
        receiver
            .apply(WallMessage::Header {
                stream_id: 3,
                frame_id: 9,
                pts_us: 100,
                width: 848,
                height: 480,
                total_len: payload.len() as u32,
            })
            .expect("帧头");
        for (index, chunk) in payload.chunks(WALL_FRAGMENT_BYTES).enumerate() {
            let frame = receiver
                .apply(WallMessage::Fragment {
                    stream_id: 3,
                    frame_id: 9,
                    frag_index: index as u16,
                    frag_count: frag_count as u16,
                    data: chunk.to_vec(),
                })
                .expect("分片");
            if index + 1 == frag_count {
                let frame = frame.expect("最后一帧应完整");
                assert_eq!(frame.data, payload);
                assert_eq!(frame.pts_us, 100);
                assert_eq!(frame.width, 848);
            } else {
                assert!(frame.is_none());
            }
        }
        assert_eq!(receiver.completed(), 1);
        assert_eq!(receiver.dropped(), 0);

        // 乱序：直接发第 1 片（期望 0）。
        let mut receiver = WallReceiver::new(1);
        receiver
            .apply(WallMessage::Header {
                stream_id: 1,
                frame_id: 1,
                pts_us: 0,
                width: 2,
                height: 2,
                total_len: 8,
            })
            .expect("帧头");
        assert!(receiver
            .apply(WallMessage::Fragment {
                stream_id: 1,
                frame_id: 1,
                frag_index: 1,
                frag_count: 2,
                data: vec![1; 4],
            })
            .is_err());
    }

    /// 不同流编号的消息被忽略，不污染本路重组。
    #[test]
    fn receiver_ignores_other_stream() {
        let mut receiver = WallReceiver::new(1);
        assert!(receiver
            .apply(WallMessage::Header {
                stream_id: 2,
                frame_id: 5,
                pts_us: 0,
                width: 4,
                height: 4,
                total_len: 16,
            })
            .expect("忽略")
            .is_none());
        assert!(receiver
            .apply(WallMessage::Fragment {
                stream_id: 2,
                frame_id: 5,
                frag_index: 0,
                frag_count: 1,
                data: vec![0; 16],
            })
            .expect("忽略")
            .is_none());
        assert_eq!(receiver.completed(), 0);
    }

    /// 缩放：2x2 → 1x1 取左上，4x4 → 2x2 隔点采样。
    #[test]
    fn downscale_nearest() {
        let src: Vec<u8> = (0..4 * 4 * 4).map(|i| i as u8).collect();
        let mut dst = vec![0u8; 2 * 2 * 4];
        downscale_rgba(&src, 4, 4, &mut dst, 2, 2).expect("缩放");
        // 目标 (0,0) 取源 (0,0)，(1,1) 取源 (2,2)。
        assert_eq!(&dst[0..4], &src[0..4]);
        assert_eq!(&dst[12..16], &src[(2 * 16 + 2 * 4)..(2 * 16 + 2 * 4 + 4)]);
        assert!(downscale_rgba(&src, 0, 4, &mut dst, 2, 2).is_err());
        assert!(downscale_rgba(&src, 4, 4, &mut dst[..4], 2, 2).is_err());
    }

    /// 最新帧汇总：按注册顺序返回，未到达为 None，未注册流被忽略。
    #[test]
    fn hub_latest_frames() {
        let mut hub = WallPreviewHub::new(&[11, 12, 13]);
        assert_eq!(hub.len(), 3);
        assert!(hub.latest().iter().all(Option::is_none));
        let frame = WallFrame {
            stream_id: 12,
            frame_id: 1,
            pts_us: 50,
            width: 848,
            height: 480,
            data: vec![0; 100],
        };
        assert!(hub.update(&frame));
        let latest = hub.latest();
        assert!(latest[0].is_none());
        let mid = latest[1].as_ref().expect("第 2 路有帧");
        assert_eq!(mid.bytes, 100);
        assert_eq!(mid.texture_id, None);
        assert!(latest[2].is_none());
        let unknown = WallFrame {
            stream_id: 99,
            ..frame
        };
        assert!(!hub.update(&unknown));
    }
}
