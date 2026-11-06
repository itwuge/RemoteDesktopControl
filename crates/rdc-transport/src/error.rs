//! rdc-transport / 错误类型
//!
//! 层级：L2 能力层
//! 用途：定义码率参数、连接关闭、超时、对端不可达与内部错误，并透传协议解析错误
//! 调用方：rdc-core / rdc-connect 的错误映射，最终由 rdc-core 收敛进 RdcError

// =====** TransportError =====**

/// 传输层错误。
#[derive(Debug, thiserror::Error)]
pub enum TransportError {
    /// 码率、冗余度或窗口参数为 0 等非法取值。
    #[error("码率或冗余参数非法")]
    InvalidRate,
    /// 发送 / 接收任务已关闭。
    #[error("传输任务已关闭")]
    Closed,
    /// 等待对端 ACK 或握手响应超时。
    #[error("等待对端响应超时")]
    Timeout,
    /// 对端地址不可达（打洞或中继均失败）。
    #[error("对端不可达")]
    PeerUnreachable,
    /// 线上报文无法解析。
    #[error(transparent)]
    Protocol(#[from] rdc_protocol::ProtocolError),
    /// 未归类的内部错误；只允许携带可读描述，不得放入密钥等敏感数据。
    #[error("传输内部错误: {0}")]
    Internal(String),
}
