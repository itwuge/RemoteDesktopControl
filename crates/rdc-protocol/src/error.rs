//! rdc-protocol / 错误码
//!
//! 层级：L2 能力层
//! 用途：定义可跨线传输、可跨 FFI 判别的错误码；`RdcError` 在 rdc-core 组装
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
    /// JSON 序列化失败。
    #[error("协议序列化失败: {0}")]
    Serialize(#[from] serde_json::Error),
    /// 未知通道数值。
    #[error("未知通道编号: {0}")]
    UnknownStream(u8),
    /// 协议主版本不兼容。
    #[error("协议主版本不兼容: 对端 {peer}, 本端 {local}")]
    VersionMismatch {
        /// 对端主版本。
        peer: u16,
        /// 本端主版本。
        local: u16,
    },
}
