//! rdc-protocol / 能力位图（Capabilities）
//!
//! 层级：L2 能力层
//! 用途：版本协商能力位图的唯一真源（docs/03 §1、docs/07 §3）。
//! 调用方：rdc-connect（注册与协商）、rdc-signal（字段长度校验）、rdc-codec（编解码器选择）、
//!         rdc-core（功能开关）
//!
//! 设计：u64 位图而不是布尔字段列表。新增能力只占一个未用位，老客户端读到未定义位
//! 直接忽略；协商结果是双方按位与，任何一方不支持的功能都不会被启用。

use serde::{Deserialize, Serialize};

// =====** 能力位常量 =====**

/// 能力位编号（写进 `Capabilities::BITS[name]` 的位掩码）。
pub mod bits {
    /// bit0：HEVC 硬编/硬解。
    pub const HEVC: u64 = 1 << 0;
    /// bit1：4:4:4 色度。
    pub const CHROMA_444: u64 = 1 << 1;
    /// bit2：10-bit 位深。
    pub const BIT_DEPTH_10: u64 = 1 << 2;
    /// bit3：intra-refresh（无关键帧的长 GOP 刷新）。
    pub const INTRA_REFRESH: u64 = 1 << 3;
    /// bit4：temporal 分层。
    pub const TEMPORAL: u64 = 1 << 4;
    /// bit5：QP map + 屏幕内容编码（SCC）。
    pub const QP_MAP_SCC: u64 = 1 << 5;
    /// bit6：双路径发送。
    pub const DUAL_PATH: u64 = 1 << 6;
    /// bit7：视频子码流（屏幕墙）。
    pub const SUB_STREAM: u64 = 1 << 7;
    /// bit8：剪贴板通道。
    pub const CLIPBOARD: u64 = 1 << 8;
    /// bit9：文件传输通道。
    pub const FILE_TRANSFER: u64 = 1 << 9;
    /// bit10：多会话。
    pub const MULTI_SESSION: u64 = 1 << 10;
    /// bit11：屏幕墙。
    pub const SCREEN_WALL: u64 = 1 << 11;
    /// bit12：账号同源互信。
    pub const ACCOUNT_TRUST: u64 = 1 << 12;
}

/// 本协议版本已定义的全部能力位。
pub const KNOWN_BITS: u64 = (1 << 13) - 1;

// =====** Capabilities =====**

/// 能力位图。
//[u64] 是线格式：注册与协商报文里就是一个整数，不拆成数组，避免两端位序歧义。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Capabilities(pub u64);

impl Capabilities {
    /// 空能力集。
    pub const NONE: Self = Self(0);

    /// 本端默认能力（S2 回环 Demo 的最小集合：HEVC、temporal、剪贴板、文件）。
    ///
    /// 为什么不是“全开”：能力位代表真实实现，S2 还没有屏幕墙与多会话，
    /// 提前置位会让对端协商出并不存在的功能。
    pub const DEFAULT: Self = Self(
        bits::HEVC | bits::TEMPORAL | bits::CLIPBOARD | bits::FILE_TRANSFER,
    );

    /// 从原始位图构造。
    ///
    /// # Arguments
    /// * `raw` - 线上收到的位图
    ///
    /// # Returns
    /// 不裁剪未知位的能力集（未知位由 [`Self::known`] 在协商时丢弃）。
    ///
    /// # Errors
    /// 不会失败。
    pub const fn from_raw(raw: u64) -> Self {
        Self(raw)
    }

    /// 返回原始位图，直接写入报文。
    ///
    /// # Returns
    /// 64 位整数。
    ///
    /// # Errors
    /// 不会失败。
    pub const fn raw(self) -> u64 {
        self.0
    }

    /// 判断某能力（一个位或其组合）是否置位。
    ///
    /// # Arguments
    /// * `mask` - [`bits`] 中的常量或其组合
    ///
    /// # Returns
    /// 所有指定位都置位时返回 `true`（组合语义是“全部满足”）。
    ///
    /// # Errors
    /// 不会失败。
    pub const fn contains(self, mask: u64) -> bool {
        self.0 & mask == mask
    }

    /// 设置/清除一个能力位。
    ///
    /// # Arguments
    /// * `mask` - 单个位
    /// * `on` - true 置位，false 清除
    ///
    /// # Returns
    /// 修改后的能力集（Copy 语义，便于链式构造）。
    ///
    /// # Errors
    /// 不会失败。
    pub const fn with(mut self, mask: u64, on: bool) -> Self {
        if on {
            self.0 |= mask;
        } else {
            self.0 &= !mask;
        }
        self
    }

    /// 只保留本协议已定义的能力位。
    ///
    /// # Returns
    /// 与 [`KNOWN_BITS`] 按位与后的能力集。
    ///
    /// # Errors
    /// 不会失败。
    pub const fn known(self) -> Self {
        Self(self.0 & KNOWN_BITS)
    }

    /// 双方能力协商：按位与。
    ///
    /// 为什么必须与：任一方不支持的功能都不能启用；不能用“或”把单端能力
    /// 当成双方能力，否则会协商出不存在的编码器。
    ///
    /// # Arguments
    /// * `peer` - 对端能力集
    ///
    /// # Returns
    /// 双方共同支持的能力集。
    ///
    /// # Errors
    /// 不会失败。
    pub const fn negotiate(self, peer: Self) -> Self {
        Self(self.0 & peer.0 & KNOWN_BITS)
    }

    /// 是否包含未知位（用于记录兼容性告警，不阻断连接）。
    ///
    /// # Returns
    /// 存在本协议未定义的位时返回 `true`。
    ///
    /// # Errors
    /// 不会失败。
    pub const fn has_unknown_bits(self) -> bool {
        self.0 & !KNOWN_BITS != 0
    }
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 协商必须是按位与，且裁掉未知位。
    #[test]
    fn negotiate_is_bitwise_and() {
        let a = Capabilities(bits::HEVC | bits::TEMPORAL | bits::SCREEN_WALL);
        let b = Capabilities(bits::HEVC | bits::CLIPBOARD | bits::SCREEN_WALL);
        let common = a.negotiate(b);
        assert!(common.contains(bits::HEVC));
        assert!(common.contains(bits::SCREEN_WALL));
        assert!(!common.contains(bits::TEMPORAL));
        assert!(!common.contains(bits::CLIPBOARD));
    }

    /// 未知位不得进入协商结果：老客户端读到新位必须静默忽略。
    #[test]
    fn unknown_bits_are_dropped() {
        let peer = Capabilities(1 << 40);
        assert!(peer.has_unknown_bits());
        assert_eq!(peer.negotiate(Capabilities(KNOWN_BITS)).raw(), 0);
        assert!(!Capabilities::DEFAULT.has_unknown_bits());
    }

    /// 线格式是一个整数，序列化后必须能原样还原。
    #[test]
    fn serde_roundtrip_is_integer() {
        let caps = Capabilities(bits::HEVC | bits::DUAL_PATH | bits::ACCOUNT_TRUST);
        let json = serde_json::to_string(&caps).expect("序列化");
        assert_eq!(json, (bits::HEVC | bits::DUAL_PATH | bits::ACCOUNT_TRUST).to_string());
        let back: Capabilities = serde_json::from_str(&json).expect("反序列化");
        assert_eq!(back, caps);
    }

    /// 位定义不得重叠，且 KNOWN_BITS 覆盖全部已定义位。
    #[test]
    fn bits_do_not_overlap() {
        let list = [
            bits::HEVC,
            bits::CHROMA_444,
            bits::BIT_DEPTH_10,
            bits::INTRA_REFRESH,
            bits::TEMPORAL,
            bits::QP_MAP_SCC,
            bits::DUAL_PATH,
            bits::SUB_STREAM,
            bits::CLIPBOARD,
            bits::FILE_TRANSFER,
            bits::MULTI_SESSION,
            bits::SCREEN_WALL,
            bits::ACCOUNT_TRUST,
        ];
        let mut acc = 0u64;
        for bit in list {
            assert_eq!(bit.count_ones(), 1, "每个能力必须是单一位");
            assert_eq!(acc & bit, 0, "能力位不得重叠");
            acc |= bit;
        }
        assert_eq!(acc, KNOWN_BITS);
    }
}
