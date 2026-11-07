//! rdc-linux-e2e / 最小推流协议
//!
//! 线格式：`kind: u8` + `len: u32 LE` + `payload[len]`。
//! 类型：
//! - [`KIND_CONFIG`]  host→client：`[width u32][height u32][fps u32]`（LE）。
//! - [`KIND_VIDEO`]   host→client：`[pts_ns u64][flags u8][Annex-B H.264]`。
//!   `pts_ns` 是 host 侧 `CLOCK_MONOTONIC` 纳秒（采集到达时刻）。
//! - [`KIND_INPUT`]   client→host：rdc-input 既有线格式载荷（≤14 字节）。
//! - [`KIND_BYE`]     任一方发送，主动断开。
//!
//! 为什么自建协议：本 crate 只做链路演示，复用生产协议会把 S8-2 变成协议改造；
//! 但输入事件载荷仍走 `rdc_input::encode_event/decode_event`，保证与生产一致。

use std::io::{self, Read, Write};

// =====** 常量 =====**

/// 视频包：编码访问单元。
pub const KIND_VIDEO: u8 = 1;
/// 输入包：rdc-input 线格式。
pub const KIND_INPUT: u8 = 2;
/// 配置包：分辨率与帧率。
pub const KIND_CONFIG: u8 = 3;
/// 断开包。
pub const KIND_BYE: u8 = 4;

/// 单包载荷上限（防御畸形输入；1080p 关键帧远小于 8MB）。
pub const MAX_PAYLOAD: usize = 8 * 1024 * 1024;

/// 视频载荷头长度：pts(8) + flags(1)。
pub const VIDEO_HEADER: usize = 9;

// =====** Packet =====**

/// 一个协议包。
#[derive(Debug, Clone)]
pub struct Packet {
    /// 包类型。
    pub kind: u8,
    /// 载荷。
    pub payload: Vec<u8>,
}

impl Packet {
    /// 构造并序列化为带帧头的字节。
    ///
    /// # Returns
    /// `kind + len + payload`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(5 + self.payload.len());
        out.push(self.kind);
        out.extend_from_slice(&(self.payload.len() as u32).to_le_bytes());
        out.extend_from_slice(&self.payload);
        out
    }

    /// 在给定写入端发送本包。
    ///
    /// # Arguments
    /// * `writer` - 目标写入端（TCP 流或管道）。
    ///
    /// # Returns
    /// 写入成功返回 `Ok(())`。
    ///
    /// # Errors
    /// 底层 IO 失败时返回错误。
    pub fn write_to(&self, writer: &mut impl Write) -> io::Result<()> {
        writer.write_all(&self.to_bytes())
    }
}

/// 从读取端读一个包。
///
/// # Arguments
/// * `reader` - 源读取端。
///
/// # Returns
/// 读到包返回 `Some`；对端正常关闭（读完帧头前 EOF）返回 `None`。
///
/// # Errors
/// 帧头被截断、载荷超限或底层 IO 失败时返回错误。
pub fn read_packet(reader: &mut impl Read) -> io::Result<Option<Packet>> {
    let mut head = [0u8; 5];
    match reader.read_exact(&mut head) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let kind = head[0];
    let len = u32::from_le_bytes([head[1], head[2], head[3], head[4]]) as usize;
    if len > MAX_PAYLOAD {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("载荷 {len} 字节超过上限 {MAX_PAYLOAD}"),
        ));
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload)?;
    Ok(Some(Packet { kind, payload }))
}

// =====** 配置载荷 =====**

/// 分辨率与帧率配置。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamConfig {
    /// 画面宽（像素）。
    pub width: u32,
    /// 画面高（像素）。
    pub height: u32,
    /// 帧率。
    pub fps: u32,
}

impl StreamConfig {
    /// 编码为载荷字节。
    ///
    /// # Returns
    /// 12 字节载荷。
    ///
    /// # Errors
    /// 不会失败。
    pub fn to_payload(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(12);
        out.extend_from_slice(&self.width.to_le_bytes());
        out.extend_from_slice(&self.height.to_le_bytes());
        out.extend_from_slice(&self.fps.to_le_bytes());
        out
    }

    /// 从载荷解析。
    ///
    /// # Arguments
    /// * `payload` - 12 字节载荷。
    ///
    /// # Returns
    /// 长度正确返回配置，否则 `None`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn from_payload(payload: &[u8]) -> Option<Self> {
        if payload.len() != 12 {
            return None;
        }
        Some(Self {
            width: u32::from_le_bytes(payload[0..4].try_into().ok()?),
            height: u32::from_le_bytes(payload[4..8].try_into().ok()?),
            fps: u32::from_le_bytes(payload[8..12].try_into().ok()?),
        })
    }
}

// =====** 视频载荷 =====**

/// 组装视频载荷。
///
/// # Arguments
/// * `pts_ns` - 采集时刻（host 侧 CLOCK_MONOTONIC 纳秒）。
/// * `keyframe` - 是否关键帧。
/// * `data` - Annex-B 码流。
///
/// # Returns
/// `[pts u64][flags u8][data]`。
///
/// # Errors
/// 不会失败。
pub fn video_payload(pts_ns: u64, keyframe: bool, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(VIDEO_HEADER + data.len());
    out.extend_from_slice(&pts_ns.to_le_bytes());
    out.push(u8::from(keyframe));
    out.extend_from_slice(data);
    out
}

/// 解析视频载荷。
///
/// # Arguments
/// * `payload` - 视频载荷。
///
/// # Returns
/// `(pts_ns, keyframe, 码流切片)`；长度不足返回 `None`。
///
/// # Errors
/// 不会失败。
pub fn parse_video(payload: &[u8]) -> Option<(u64, bool, &[u8])> {
    if payload.len() < VIDEO_HEADER {
        return None;
    }
    let pts = u64::from_le_bytes(payload[0..8].try_into().ok()?);
    let keyframe = payload[8] != 0;
    Some((pts, keyframe, &payload[VIDEO_HEADER..]))
}

// =====** 自检 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 帧头与载荷必须可逆。
    #[test]
    fn packet_roundtrip() {
        let packet = Packet {
            kind: KIND_VIDEO,
            payload: vec![1, 2, 3],
        };
        let bytes = packet.to_bytes();
        let mut cursor = std::io::Cursor::new(bytes);
        let back = read_packet(&mut cursor).expect("可读").expect("非空");
        assert_eq!(back.kind, KIND_VIDEO);
        assert_eq!(back.payload, vec![1, 2, 3]);
        assert!(read_packet(&mut cursor).expect("可读").is_none());
    }

    /// 视频载荷的时间戳与码流必须可逆。
    #[test]
    fn video_payload_roundtrip() {
        let payload = video_payload(1234, true, &[0, 0, 1, 9]);
        let (pts, keyframe, data) = parse_video(&payload).expect("可解析");
        assert_eq!(pts, 1234);
        assert!(keyframe);
        assert_eq!(data, &[0, 0, 1, 9]);
    }

    /// 配置载荷必须可逆。
    #[test]
    fn config_roundtrip() {
        let config = StreamConfig {
            width: 1280,
            height: 800,
            fps: 60,
        };
        assert_eq!(StreamConfig::from_payload(&config.to_payload()), Some(config));
    }
}
