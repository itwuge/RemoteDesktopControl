//! rdc-protocol / 媒体包头编解码（32 字节固定结构）
//!
//! 层级：L2 能力层（零 IO、零平台依赖）
//! 用途：定义 UDP 媒体包头、标志位、分片/重组/解码队列常量，并提供**唯一的**
//!       编码与解码入口；任何 crate 都不得自行拼装头部字节（docs/04 §1.1）。
//! 调用方：rdc-transport（发包/收包）、rdc-connect（控制通道封装）、
//!         rdc-relay（按头部字段转发）、rdc-core（统计）
//!
//! 兼容性纪律：偏移与字段宽度是协议的一部分。新增字段只能占用 `reserved` 或走
//! TLV 扩展，**禁止重排既有偏移**；改动本文件等同于协议主版本变更。

use crate::error::ProtocolError;

// =====** 协议常量 =====**

/// 包头固定长度（字节）。
pub const PACKET_HEADER_LEN: usize = 32;

/// AEAD 认证标签长度（字节），紧接载荷之后。
pub const AEAD_TAG_LEN: usize = 16;

/// 单包总长上限（含头 + 载荷 + tag），避免 IP 分片（docs/04 §1.2）。
pub const MAX_PACKET_SIZE: usize = 1400;

/// 单分片明文载荷上限 = 1400 - 32 - 16 = 1352 字节。
pub const FRAGMENT_PAYLOAD_MAX: usize = MAX_PACKET_SIZE - PACKET_HEADER_LEN - AEAD_TAG_LEN;

/// 包头魔数第 0 字节 `R`；信令报文以 `{` 开头，两者不会混淆。
pub const HEADER_MAGIC_0: u8 = b'R';

/// 包头魔数第 1 字节 `D`。
pub const HEADER_MAGIC_1: u8 = b'D';

/// 传输协议版本（包头偏移 2）。主版本不兼容时返回 `PROTOCOL_VERSION_UNSUPPORTED`。
pub const TRANSPORT_VERSION: u8 = 1;

/// 重组窗口（帧）：同一帧的分片只在这个窗口内等待，超时整帧丢弃。
pub const REASSEMBLY_WINDOW_FRAMES: u8 = 4;

/// 解码队列深度：只保留最新 2 帧，防止解码延迟随队列增长而漂移（docs/04 §5）。
pub const DECODE_QUEUE_DEPTH: usize = 2;

/// 抖动缓冲下限（帧）。
pub const JITTER_BUFFER_MIN_FRAMES: u8 = 1;

/// 抖动缓冲上限（帧）。
pub const JITTER_BUFFER_MAX_FRAMES: u8 = 3;

/// “无 FEC”哨兵值：`fec_group` 与 `fec_index` 未启用时都必须写它。
pub const NO_FEC: u16 = 0xFFFF;

// =====** PacketFlags =====**

/// 包头 `flags` 字节（偏移 3）。
///
/// 用 newtype 而不是 `u8`：读写都必须经过命名访问器，避免调用方误用位运算把
/// `temporal_id`（bit4~5）当成布尔位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PacketFlags(u8);

impl PacketFlags {
    /// bit0：关键帧 / 可解码起点。
    pub const KEYFRAME: u8 = 1 << 0;
    /// bit1：该包为重传（NACK 触发）。
    pub const RETRANSMIT: u8 = 1 << 1;
    /// bit2：FEC 校验包（不是数据包）。
    pub const FEC_PARITY: u8 = 1 << 2;
    /// bit3：经中继转发（中继据此避免二次标记）。
    pub const VIA_RELAY: u8 = 1 << 3;
    /// bit6：双路径副本（同一包在另一路径也有发送）。
    pub const DUAL_PATH: u8 = 1 << 6;
    /// bit7：保留，发送必须置 0。
    pub const RESERVED: u8 = 1 << 7;

    /// `temporal_id` 的位掩码。
    const TEMPORAL_MASK: u8 = 0b11 << 4;

    /// 从原始字节构造（接收路径）。
    ///
    /// # Arguments
    /// * `raw` - 包头里的 flags 字节
    ///
    /// # Returns
    /// 标志位视图，保留全部位（含未知位），便于原样回写。
    ///
    /// # Errors
    /// 不会失败。
    pub fn from_raw(raw: u8) -> Self {
        Self(raw)
    }

    /// 返回原始字节（发送路径直接写入包头）。
    ///
    /// # Returns
    /// 8 位标志字节。
    ///
    /// # Errors
    /// 不会失败。
    pub fn raw(self) -> u8 {
        self.0
    }

    /// 是否关键帧 / 可解码起点。
    pub fn is_keyframe(self) -> bool {
        self.0 & Self::KEYFRAME != 0
    }

    /// 设置关键帧位。
    pub fn set_keyframe(&mut self, on: bool) {
        set_bit(&mut self.0, Self::KEYFRAME, on);
    }

    /// 是否重传包。
    pub fn is_retransmit(self) -> bool {
        self.0 & Self::RETRANSMIT != 0
    }

    /// 设置重传位。
    pub fn set_retransmit(&mut self, on: bool) {
        set_bit(&mut self.0, Self::RETRANSMIT, on);
    }

    /// 是否 FEC 校验包。
    pub fn is_fec_parity(self) -> bool {
        self.0 & Self::FEC_PARITY != 0
    }

    /// 设置 FEC 校验包位。
    pub fn set_fec_parity(&mut self, on: bool) {
        set_bit(&mut self.0, Self::FEC_PARITY, on);
    }

    /// 是否经中继转发。
    pub fn is_via_relay(self) -> bool {
        self.0 & Self::VIA_RELAY != 0
    }

    /// 设置中继位。
    pub fn set_via_relay(&mut self, on: bool) {
        set_bit(&mut self.0, Self::VIA_RELAY, on);
    }

    /// 是否双路径副本。
    pub fn is_dual_path(self) -> bool {
        self.0 & Self::DUAL_PATH != 0
    }

    /// 设置双路径位。
    pub fn set_dual_path(&mut self, on: bool) {
        set_bit(&mut self.0, Self::DUAL_PATH, on);
    }

    /// 读取 `temporal_id`（0~3）。
    pub fn temporal_id(self) -> u8 {
        // 右移到最低位后只取 2 位，确保不会把 bit6/bit7 混进来。
        (self.0 & Self::TEMPORAL_MASK) >> 4
    }

    /// 写入 `temporal_id`；传入 >3 的值会被截断为 2 位（调用方应先校验）。
    pub fn set_temporal_id(&mut self, id: u8) {
        self.0 = (self.0 & !Self::TEMPORAL_MASK) | ((id & 0b11) << 4);
    }
}

/// 按位设置/清除一个标志位。
fn set_bit(raw: &mut u8, mask: u8, on: bool) {
    if on {
        *raw |= mask;
    } else {
        *raw &= !mask;
    }
}

// =====** PacketHeader =====**

/// 媒体包头（32 字节）。
///
/// 字段顺序与 `encode` 的写入顺序严格一致，见 docs/04 §1.1 偏移表。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketHeader {
    /// 偏移 2：传输协议版本。
    pub version: u8,
    /// 偏移 3：标志位。
    pub flags: PacketFlags,
    /// 偏移 4：会话标识（一次连接内稳定）。
    pub session_id: u32,
    /// 偏移 8：通道编号，见 [`crate::stream::StreamKind`]。
    pub stream: u8,
    /// 偏移 9：优先级，见 [`crate::stream::StreamKind::priority`]。
    pub priority: u8,
    /// 偏移 10：明文载荷长度（不含头与 tag）。
    pub payload_len: u16,
    /// 偏移 12：该通道单调递增序号（u32 回绕安全）。
    pub seq: u32,
    /// 偏移 16：采集时间戳（微秒，主机单调时钟），全程不改写。
    pub ts_us: u64,
    /// 偏移 24：FEC 组号；无 FEC 时为 [`NO_FEC`]。
    pub fec_group: u16,
    /// 偏移 26：组内序号（数据包 0..K，校验包 K..K+M）。
    pub fec_index: u16,
    /// 偏移 28：帧号（u16，按帧递增）。
    pub frame_id: u16,
    /// 偏移 30：对齐保留；发送置 0，接收忽略非零值。
    pub reserved: u16,
}

impl PacketHeader {
    /// 构造一个“无 FEC”的包头。
    ///
    /// # Arguments
    /// * `session_id` - 会话标识
    /// * `stream` - 通道编号
    /// * `seq` - 通道序号
    /// * `ts_us` - 采集时间戳（微秒）
    /// * `frame_id` - 帧号
    /// * `payload_len` - 明文载荷长度
    ///
    /// # Returns
    /// 版本为 [`TRANSPORT_VERSION`]、优先级按通道表自动填写的包头。
    ///
    /// # Errors
    /// 载荷超长返回 [`ProtocolError::PayloadTooLarge`]（1352 字节上限）。
    pub fn new(
        session_id: u32,
        stream: u8,
        seq: u32,
        ts_us: u64,
        frame_id: u16,
        payload_len: u16,
    ) -> Result<Self, ProtocolError> {
        let priority = crate::stream::StreamKind::from_u8(stream)
            .ok_or(ProtocolError::UnknownStream(stream))?
            .priority();
        if payload_len as usize > FRAGMENT_PAYLOAD_MAX {
            return Err(ProtocolError::PayloadTooLarge {
                actual: payload_len as usize,
                max: FRAGMENT_PAYLOAD_MAX,
            });
        }
        Ok(Self {
            version: TRANSPORT_VERSION,
            flags: PacketFlags::default(),
            session_id,
            stream,
            priority,
            payload_len,
            seq,
            ts_us,
            fec_group: NO_FEC,
            fec_index: NO_FEC,
            frame_id,
            reserved: 0,
        })
    }

    /// 编码为 32 字节数组。
    ///
    /// # Returns
    /// 大端序（网络字节序）排列的固定长度头部。
    ///
    /// # Errors
    /// 载荷长度超过 [`FRAGMENT_PAYLOAD_MAX`] 时返回 [`ProtocolError::PayloadTooLarge`]，
    /// 防止调用方构造出会被 IP 分片的包。
    pub fn encode(&self) -> Result<[u8; PACKET_HEADER_LEN], ProtocolError> {
        // 编码前再次校验，因为字段是 pub 的，可能被外部直接改写。
        if self.payload_len as usize > FRAGMENT_PAYLOAD_MAX {
            return Err(ProtocolError::PayloadTooLarge {
                actual: self.payload_len as usize,
                max: FRAGMENT_PAYLOAD_MAX,
            });
        }
        let mut out = [0u8; PACKET_HEADER_LEN];
        out[0] = HEADER_MAGIC_0;
        out[1] = HEADER_MAGIC_1;
        out[2] = self.version;
        out[3] = self.flags.raw();
        out[4..8].copy_from_slice(&self.session_id.to_be_bytes());
        out[8] = self.stream;
        out[9] = self.priority;
        out[10..12].copy_from_slice(&self.payload_len.to_be_bytes());
        out[12..16].copy_from_slice(&self.seq.to_be_bytes());
        out[16..24].copy_from_slice(&self.ts_us.to_be_bytes());
        out[24..26].copy_from_slice(&self.fec_group.to_be_bytes());
        out[26..28].copy_from_slice(&self.fec_index.to_be_bytes());
        out[28..30].copy_from_slice(&self.frame_id.to_be_bytes());
        // 发送端把保留字段强制写 0：即使结构体被外部改过，也不把非零值带上线。
        out[30..32].copy_from_slice(&0u16.to_be_bytes());
        Ok(out)
    }

    /// 解码 32 字节头部。
    ///
    /// # Arguments
    /// * `bytes` - 至少 32 字节的报文前缀；多余字节会被忽略（载荷由调用方处理）。
    ///
    /// # Returns
    /// 解析后的头部；`reserved` 非零不会报错（前向兼容要求忽略它）。
    ///
    /// # Errors
    /// 长度不足返回 [`ProtocolError::PacketTooShort`]；魔数不符返回
    /// [`ProtocolError::InvalidMagic`]；载荷长度超限返回 [`ProtocolError::PayloadTooLarge`]。
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() < PACKET_HEADER_LEN {
            return Err(ProtocolError::PacketTooShort {
                actual: bytes.len(),
                min: PACKET_HEADER_LEN,
            });
        }
        if bytes[0] != HEADER_MAGIC_0 || bytes[1] != HEADER_MAGIC_1 {
            return Err(ProtocolError::InvalidMagic {
                magic0: bytes[0],
                magic1: bytes[1],
            });
        }
        let payload_len = u16::from_be_bytes([bytes[10], bytes[11]]);
        if payload_len as usize > FRAGMENT_PAYLOAD_MAX {
            return Err(ProtocolError::PayloadTooLarge {
                actual: payload_len as usize,
                max: FRAGMENT_PAYLOAD_MAX,
            });
        }
        Ok(Self {
            version: bytes[2],
            flags: PacketFlags::from_raw(bytes[3]),
            session_id: u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]),
            stream: bytes[8],
            priority: bytes[9],
            payload_len,
            seq: u32::from_be_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]),
            ts_us: u64::from_be_bytes([
                bytes[16], bytes[17], bytes[18], bytes[19], bytes[20], bytes[21], bytes[22], bytes[23],
            ]),
            fec_group: u16::from_be_bytes([bytes[24], bytes[25]]),
            fec_index: u16::from_be_bytes([bytes[26], bytes[27]]),
            frame_id: u16::from_be_bytes([bytes[28], bytes[29]]),
            reserved: u16::from_be_bytes([bytes[30], bytes[31]]),
        })
    }

    /// 校验传输版本是否与本端一致。
    ///
    /// # Returns
    /// 一致返回 `Ok(())`。
    ///
    /// # Errors
    /// 版本不一致返回 [`ProtocolError::VersionMismatch`]，由调用方翻译为
    /// `PROTOCOL_VERSION_UNSUPPORTED` 并拒绝升级/降级。
    pub fn ensure_supported(&self) -> Result<(), ProtocolError> {
        if self.version != TRANSPORT_VERSION {
            return Err(ProtocolError::TransportVersionUnsupported {
                peer: self.version,
                local: TRANSPORT_VERSION,
            });
        }
        Ok(())
    }

    /// 按通道表校正 `priority`，返回新头部。
    ///
    /// 为什么接收端也要校正：中继与恶意对端可能填写错误优先级抢占带宽，
    /// 服务端不信任线上字段，只信任 `stream` 与本地表。
    ///
    /// # Returns
    /// `priority` 已按 [`crate::stream::StreamKind::priority`] 重写的头部。
    ///
    /// # Errors
    /// 未知通道返回 [`ProtocolError::UnknownStream`]，调用方应整包丢弃并计数。
    pub fn with_canonical_priority(mut self) -> Result<Self, ProtocolError> {
        self.priority = crate::stream::StreamKind::from_u8(self.stream)
            .ok_or(ProtocolError::UnknownStream(self.stream))?
            .priority();
        Ok(self)
    }
}

// =====** 单元测试：冻结线格式 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 编解码必须逐字节可复现：这是与其它实现互通的唯一保证。
    #[test]
    fn header_roundtrip_is_byte_exact() {
        let mut h = PacketHeader::new(0x1122_3344, 3, 0x0102_0304, 0x0102_0304_0506_0708, 7, 40)
            .expect("合法头部");
        h.flags.set_keyframe(true);
        h.flags.set_temporal_id(2);
        h.fec_group = 9;
        h.fec_index = 3;
        let bytes = h.encode().expect("编码成功");
        assert_eq!(bytes.len(), 32);
        // 逐字段断言偏移，防止后续重构悄悄移动字段。
        assert_eq!(&bytes[0..2], b"RD");
        assert_eq!(bytes[2], TRANSPORT_VERSION);
        assert_eq!(bytes[3], 0b0010_0001);
        assert_eq!(&bytes[4..8], &0x1122_3344u32.to_be_bytes());
        assert_eq!(bytes[8], 3);
        assert_eq!(bytes[9], 2); // 控制通道 P2
        assert_eq!(&bytes[10..12], &40u16.to_be_bytes());
        assert_eq!(&bytes[12..16], &0x0102_0304u32.to_be_bytes());
        assert_eq!(&bytes[16..24], &0x0102_0304_0506_0708u64.to_be_bytes());
        assert_eq!(&bytes[24..26], &9u16.to_be_bytes());
        assert_eq!(&bytes[26..28], &3u16.to_be_bytes());
        assert_eq!(&bytes[28..30], &7u16.to_be_bytes());
        assert_eq!(&bytes[30..32], &[0, 0]);
        assert_eq!(PacketHeader::decode(&bytes).expect("解码成功"), h);
    }

    /// 接收端必须忽略 reserved 非零值（前向兼容）。
    #[test]
    fn decode_ignores_nonzero_reserved() {
        let h = PacketHeader::new(1, 0, 1, 100, 0, 10).expect("合法头部");
        let mut bytes = h.encode().expect("编码成功");
        bytes[30] = 0xAA;
        bytes[31] = 0xBB;
        let decoded = PacketHeader::decode(&bytes).expect("解码成功");
        assert_eq!(decoded.reserved, 0xAABB);
        assert_eq!(decoded.stream, 0);
    }

    /// 魔数不符必须报错，不能把信令 JSON 当媒体包解析。
    #[test]
    fn decode_rejects_wrong_magic() {
        let mut bytes = [0u8; 32];
        bytes[0] = b'{';
        let err = PacketHeader::decode(&bytes).expect_err("魔数错误");
        assert!(matches!(err, ProtocolError::InvalidMagic { .. }));
    }

    /// 截断报文必须报错而不是越界读取。
    #[test]
    fn decode_rejects_short_packet() {
        let err = PacketHeader::decode(&[0u8; 31]).expect_err("长度不足");
        assert!(matches!(err, ProtocolError::PacketTooShort { .. }));
    }

    /// 载荷超限必须在编码期拒绝：这是“不产生 IP 分片”的硬约束。
    #[test]
    fn payload_over_limit_is_rejected() {
        let err = PacketHeader::new(1, 0, 1, 1, 1, 1353).expect_err("超限");
        assert!(matches!(err, ProtocolError::PayloadTooLarge { .. }));
    }

    /// 分片上限常量必须与文档公式一致（1400-32-16）。
    #[test]
    fn fragment_limit_matches_document() {
        assert_eq!(FRAGMENT_PAYLOAD_MAX, 1352);
        assert_eq!(PACKET_HEADER_LEN + FRAGMENT_PAYLOAD_MAX + AEAD_TAG_LEN, 1400);
    }

    /// temporal_id 只占 bit4~5，不得污染其它位。
    #[test]
    fn temporal_id_is_isolated() {
        let mut flags = PacketFlags::default();
        flags.set_keyframe(true);
        flags.set_dual_path(true);
        flags.set_temporal_id(3);
        assert_eq!(flags.temporal_id(), 3);
        assert!(flags.is_keyframe());
        assert!(flags.is_dual_path());
        assert_eq!(flags.raw() & 0b1100_0000, 0b0100_0000);
        flags.set_temporal_id(0);
        assert_eq!(flags.temporal_id(), 0);
    }
}
