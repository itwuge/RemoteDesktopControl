//! rdc-protocol / 错误码与编解码错误
//!
//! 层级：L2 能力层
//! 用途：定义可跨线传输、可跨 FFI 判别的错误码；`RdcError` 在 rdc-core 组装；
//!       同时定义包头/信令编解码失败的具体错误类型。
//! 调用方：所有 crate 的错误映射、rdc-bridge 的 Dart 侧文案选择

use serde::{Deserialize, Serialize};

// =====** ErrorCode =====**

/// 可判别错误码。
///
/// 设计要点：Dart 侧按 `code` 选择文案（docs/01 §4.3），因此每个值必须有稳定语义，
/// 禁止复用；新增只能追加。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
    /// 未分类内部错误。
    Internal,
    /// 参数非法（配置、地址、码率等）。
    InvalidArgument,
    /// 系统权限缺失（屏幕录制/辅助功能/无障碍）。
    NoPermission,
    /// 目标设备不存在或离线。
    PeerNotFound,
    /// 配对码错误或已过期。
    PairingCodeInvalid,
    /// P2P 打洞失败（在严格模式下不自动切中继）。
    P2pFailed,
    /// 中继未开启或票据无效。
    RelayUnavailable,
    /// 安全握手失败（验签/AEAD 校验不通过）。
    HandshakeFailed,
    /// 编码格式或硬件能力不支持。
    CodecUnsupported,
    /// 传输层错误（socket、超时、分片损坏）。
    Transport,
    /// 会话状态机拒绝该操作。
    InvalidState,
    /// 资源忙（队列满、重配进行中）。
    Busy,
    /// 线程 panic 被 supervisor 捕获（docs/01 §3.9）。
    ThreadPanic,
    /// 平台不支持该能力（如移动端采集）。
    Unsupported,
}

// =====** ProtocolError =====**

/// 协议编解码错误。
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    /// JSON 序列化/反序列化失败。
    #[error("协议 JSON 编解码失败: {0}")]
    Serialize(#[from] serde_json::Error),
    /// 未知通道数值：调用方应整包丢弃并计入 `unknown_stream_drops`。
    #[error("未知通道编号: {0}")]
    UnknownStream(u8),
    /// 协议主版本不兼容（旧字段，保留给 rdc-core 映射）。
    #[error("协议主版本不兼容: 对端 {peer}, 本端 {local}")]
    VersionMismatch {
        /// 对端主版本。
        peer: u16,
        /// 本端主版本。
        local: u16,
    },
    /// 传输包头版本不支持（应对端返回 `PROTOCOL_VERSION_UNSUPPORTED`）。
    #[error("传输协议版本不支持: 对端 {peer}, 本端 {local}")]
    TransportVersionUnsupported {
        /// 对端包头版本。
        peer: u8,
        /// 本端包头版本。
        local: u8,
    },
    /// 信令信封版本不支持。
    #[error("信令协议版本不支持: 对端 {peer}, 本端 {local}")]
    SignalVersionUnsupported {
        /// 对端 `v` 字段。
        peer: u8,
        /// 本端 `v` 字段。
        local: u8,
    },
    /// 包头魔数不符，说明收到的不是媒体包。
    #[error("包头魔数不符: 0x{magic0:02X} 0x{magic1:02X}")]
    InvalidMagic {
        /// 偏移 0 的字节。
        magic0: u8,
        /// 偏移 1 的字节。
        magic1: u8,
    },
    /// 报文长度不足。
    #[error("报文长度不足: 实际 {actual} 字节, 至少 {min} 字节")]
    PacketTooShort {
        /// 实际长度。
        actual: usize,
        /// 最小长度。
        min: usize,
    },
    /// 载荷超过单分片上限。
    #[error("载荷超限: 实际 {actual} 字节, 上限 {max} 字节")]
    PayloadTooLarge {
        /// 实际长度。
        actual: usize,
        /// 上限。
        max: usize,
    },
    /// 信令包超过 4 KB 上限。
    #[error("信令包超限: 实际 {actual} 字节, 上限 {max} 字节")]
    SignalPacketTooLarge {
        /// 实际长度。
        actual: usize,
        /// 上限。
        max: usize,
    },
    /// 关键字段缺失或取值非法。
    #[error("字段非法: {0}")]
    InvalidField(&'static str),
    /// 关键字段无法解析，并保留底层解析错误。
    ///
    /// 对外文案与 [`ProtocolError::InvalidField`] 一致；单独成变体是为了
    /// 在不改动人读文案的前提下保留 `base64::DecodeError`，供日志与排查定位（R-C-3）。
    #[error("字段非法: {message}")]
    InvalidFieldWithSource {
        /// 人读说明（与旧 `InvalidField` 的文案逐字一致）。
        message: &'static str,
        /// 底层解析错误。
        #[source]
        source: base64::DecodeError,
    },
    /// 编解码器不支持该配置（S3 起由 rdc-codec 返回，经 rdc-core 收敛）。
    #[error("编解码器不支持: {0}")]
    CodecUnsupported(&'static str),
    /// 解码队列/重组窗口等内部状态异常。
    #[error("协议状态异常: {0}")]
    State(&'static str),
}
