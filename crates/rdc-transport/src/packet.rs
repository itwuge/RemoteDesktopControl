//! rdc-transport / 报文头
//!
//! 层级：L2 能力层
//! 用途：线上 32 字节固定包头的唯一入口。线格式与编解码实现已由 rdc-protocol 冻结
//!       （docs/04 §1.1），本模块只做再导出，避免出现两份会漂移的定义。
//! 调用方：rdc-transport 的 udp / 调度 / NACK / FEC 模块，rdc-core 的收发任务

use rdc_protocol::PacketHeader;

/// 线上定长报文头（32 字节，大端序）。
///
/// 字段偏移、魔数、载荷上限与版本校验见 `rdc_protocol::header`；
/// 本 crate 内所有收发路径必须使用同一类型，禁止另建结构体复制字段。
pub use rdc_protocol::PacketHeader as Header;

/// 报文头的定长字节数（32，含 magic 与 reserved）。
pub const HEADER_LEN: usize = rdc_protocol::PACKET_HEADER_LEN;

/// 单包明文载荷上限（1352 字节，保证含头与 AEAD tag 不超过 1400）。
pub const PAYLOAD_MAX: usize = rdc_protocol::FRAGMENT_PAYLOAD_MAX;

/// 报文头类型别名，保持调用方书写习惯。
pub type WireHeader = PacketHeader;


// =====** 自检 =====**

#[cfg(test)]
mod tests {
    use super::*;
    use rdc_protocol::{PacketFlags, StreamKind};

    /// 本 crate 的常量必须与协议真源一致，避免调整一端漏掉另一端。
    #[test]
    fn constants_match_protocol() {
        assert_eq!(HEADER_LEN, 32);
        assert_eq!(PAYLOAD_MAX, 1400 - 32 - 16);
    }

    /// 用协议层编解码走一遍 32 字节包头，确认再导出的是同一实现。
    #[test]
    fn header_roundtrip_through_reexport() {
        let mut header =
            Header::new(0x0102_0304, StreamKind::VideoMain.as_u8(), 42, 123_456, 7, 1352)
                .expect("合法头部");
        header.flags.set_keyframe(true);
        header.fec_group = 5;
        header.fec_index = 2;
        let bytes = header.encode().expect("编码成功");
        assert_eq!(bytes.len(), HEADER_LEN);
        let decoded = Header::decode(&bytes).expect("解码成功");
        assert_eq!(decoded, header);
        assert!(decoded.flags.is_keyframe());
        assert_eq!(decoded.flags.raw(), PacketFlags::KEYFRAME);
    }

    /// 超过 1352 字节的载荷必须在构造期被拒绝，防止 IP 分片。
    #[test]
    fn oversized_payload_is_rejected() {
        assert!(Header::new(1, 0, 0, 0, 0, 1353).is_err());
    }
}
