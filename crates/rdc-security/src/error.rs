//! rdc-security / 错误类型
//!
//! 层级：L2 能力层
//! 用途：定义密钥长度、AEAD、握手状态、签名与票据校验失败的可判别错误，
//!       并透传 rdc-protocol 的报文解析错误
//! 调用方：rdc-connect / rdc-core 的错误映射，最终由 rdc-core 收敛进 RdcError

// =====** SecurityError =====**

/// 安全层错误。
///
/// 敏感值约束：Display 文案**不得**引用底层 `#[source]`；源错误只供日志/排查按
/// 类型取回，绝不参与用户可见文本。各变体的 `source` 为 `Option`，因为部分失败
/// 是前置校验（如长度不足），没有底层错误。
#[derive(Debug, thiserror::Error)]
pub enum SecurityError {
    /// 密钥长度不符合算法要求。
    #[error("密钥长度错误: 期望 {expected} 字节, 实际 {actual} 字节")]
    InvalidKeyLength {
        /// 算法要求的字节数。
        expected: usize,
        /// 实际收到的字节数。
        actual: usize,
        /// 底层 Base64 解码错误；长度前置校验时为 `None`。
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
    /// AEAD 加密或解密失败（标签校验不通过）。
    #[error("AEAD 加解密失败")]
    AeadFailure {
        /// 底层算法库错误（内容不透明，不含密钥/明文）；前置长度校验时为 `None`。
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
    /// 握手状态机不接受当前操作。
    #[error("握手状态非法: {state}")]
    HandshakeStateInvalid {
        /// 当前状态的可读名称，用于日志定位。
        state: String,
    },
    /// 签名校验不通过。
    #[error("签名校验失败")]
    SignatureInvalid {
        /// 底层公钥/签名解析或验签错误；无底层错误时为 `None`。
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
    /// 报文序号被重放或超出接收窗口。
    #[error("检测到重放或序号超出窗口: seq={seq}, 窗口最高={highest}")]
    ReplayDetected {
        /// 收到的序号。
        seq: u64,
        /// 接收窗口当前最高序号。
        highest: u64,
    },
    /// PAKE 交互失败（消息长度/侧别不匹配或口令不一致）。
    #[error("PAKE 交互失败: {message}")]
    PakeFailure {
        /// 失败原因（静态文案，不含口令或消息内容）。
        message: &'static str,
        /// 底层 PAKE/Base64 错误；状态机前置检查时为 `None`。
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
    /// 配对码格式非法。
    #[error("配对码格式非法")]
    PairingCodeInvalid,
    /// 握手确认 HMAC 校验失败（通常意味着配对码错误）。
    #[error("握手确认失败")]
    ConfirmationFailed {
        /// 底层 Base64 解码或 HMAC 比较错误；状态机前置检查时为 `None`。
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
    /// 中继票据无效或已过期。
    #[error("中继票据无效")]
    TicketInvalid,
    /// 对端协议报文不可解析。
    ///
    /// 保留原始错误而不是折叠成字符串：上层需要按 `ErrorCode` 选择 Dart 文案。
    #[error(transparent)]
    Protocol(#[from] rdc_protocol::ProtocolError),
}
