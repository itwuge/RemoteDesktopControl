//! rdc-connect / 错误类型
//!
//! 层级：L2 能力层
//! 用途：定义参数、配对码、打洞、中继开关等连接期错误，并透传 transport /
//!       security / protocol 三层错误
//! 调用方：rdc-core 的错误映射（收敛进 RdcError），rdc-cli / rdc-bridge 的文案选择

// =====** ConnectError =====**

/// 连接期错误。
#[derive(Debug, thiserror::Error)]
pub enum ConnectError {
    /// 连接参数非法（地址格式错误、超时值越界等）。
    #[error("连接参数非法")]
    InvalidArgument,
    /// 配对码错误或已过期。
    #[error("配对码无效或已过期")]
    PairingCodeInvalid,
    /// P2P 打洞失败，且调用方未允许回退中继。
    #[error("P2P 打洞失败（已尝试 {attempts} 次）")]
    P2pFailed {
        /// 尝试过的候选组合数量，用于提示与统计。
        attempts: u32,
    },
    /// 中继通道被策略禁用，无法回退。
    #[error("中继未启用")]
    RelayDisabled,
    /// 传输层错误。
    #[error(transparent)]
    Transport(#[from] rdc_transport::TransportError),
    /// 安全层错误（握手、验签、票据校验）。
    #[error(transparent)]
    Security(#[from] rdc_security::SecurityError),
    /// 协议报文不可解析。
    #[error(transparent)]
    Protocol(#[from] rdc_protocol::ProtocolError),
}
