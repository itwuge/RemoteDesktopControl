//! rdc-transport / 报文头
//!
//! 层级：L2 能力层
//! 用途：定义线上定长报文头的字段与编解码接口；S1 只固定字段宽度与常量
//! 调用方：rdc-core（收发的分包与组包）、rdc-relay（只读 session_id 转发）、
//!         rdc-transport 的 FEC / NACK 模块（按 seq 定位分片）

use crate::error::TransportError;
use rdc_protocol::StreamKind;

// =====** PacketHeader =====**

/// 线上定长报文头。
///
/// 字段顺序与字节宽度是协议的一部分（docs/04 §2），新增字段只能追加并同步
/// `HEADER_LEN`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketHeader {
    /// 会话标识，中继按此值做多路复用。
    pub session_id: u64,
    /// 流内单调递增序号，NACK 与重排都依赖它。
    pub seq: u32,
    /// 载荷所属通道。
    pub stream: StreamKind,
    /// 载荷字节数（不含报文头）。
    pub payload_len: u16,
}

/// 报文头的定长字节数。
///
/// 8(session_id) + 4(seq) + 1(stream) + 2(payload_len) = 15。
pub const HEADER_LEN: usize = 15;

impl PacketHeader {
    /// 把小端序编码的报文头。
    ///
    /// # Returns
    /// 固定 `HEADER_LEN` 字节的缓冲区，可直接拼接载荷写出。
    ///
    /// # Errors
    /// 不会失败。
    ///
    /// # Panics
    /// S1 空壳：实现落地前调用会 panic。
    pub fn encode(&self) -> Vec<u8> {
        todo!("S1 空壳：报文头编码在 S2 实现")
    }

    /// 从字节流解析报文头。
    ///
    /// # Arguments
    /// * `bytes` - 至少包含 `HEADER_LEN` 字节的切片
    ///
    /// # Returns
    /// 解析出的报文头。
    ///
    /// # Errors
    /// 长度不足或 `stream` 数值未定义时返回 [`TransportError::Protocol`]。
    ///
    /// # Panics
    /// S1 空壳：实现落地前调用会 panic。
    pub fn decode(_bytes: &[u8]) -> Result<Self, TransportError> {
        todo!("S1 空壳：报文头解析在 S2 实现")
    }
}

// =====** 自检 =====**

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_len_matches_field_sum() {
        // 字段宽度之和必须与常量一致，否则编解码会错位。
        assert_eq!(HEADER_LEN, 8 + 4 + 1 + 2);
    }

    #[test]
    fn header_exposes_stream_kind() {
        let header = PacketHeader {
            session_id: 1,
            seq: 0,
            stream: rdc_protocol::StreamKind::Control,
            payload_len: 0,
        };
        assert_eq!(header.stream.as_u8(), 3);
    }
}
