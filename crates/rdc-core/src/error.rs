//! rdc-core / 跨层统一错误模型
//!
//! 层级：L3 编排层
//! 用途：把各 crate 的内部错误收敛为可跨 FFI 的 [`RdcError`]（docs/02 §2.7）
//! 调用方：rdc-bridge（映射为 Dart 侧结构）、rdc-cli（打印退出码）

use rdc_protocol::ErrorCode;

// =====** RdcError =====**

/// 统一的跨层错误。
///
/// 设计要点：Dart 侧只按 `code` 选择文案，`message` 只进日志，`user_hint` 才展示给用户。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RdcError {
    /// 可判别错误码。
    pub code: ErrorCode,
    /// 面向日志的详细描述。
    pub message: String,
    /// 面向用户的一句话建议；无建议时为 `None`。
    pub user_hint: Option<String>,
    /// 是否允许立即重试（决定 UI 是否显示"重试"按钮）。
    pub retryable: bool,
}

impl RdcError {
    /// 构造一个不可重试错误。
    ///
    /// # Arguments
    /// * `code` - 错误码
    /// * `message` - 日志描述
    ///
    /// # Returns
    /// `retryable = false`、无用户建议的错误。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            user_hint: None,
            retryable: false,
        }
    }

    /// 构造一个可重试错误。
    ///
    /// # Arguments
    /// * `code` - 错误码
    /// * `message` - 日志描述
    /// * `user_hint` - 展示给用户的一句话建议
    ///
    /// # Returns
    /// `retryable = true` 的错误。
    ///
    /// # Errors
    /// 不会失败。
    pub fn retryable(code: ErrorCode, message: impl Into<String>, user_hint: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            user_hint: Some(user_hint.into()),
            retryable: true,
        }
    }
}

/// 把传输层错误收敛为统一错误。
///
/// # Arguments
/// * `err` - rdc-transport 的错误
///
/// # Returns
/// 对应错误码的 [`RdcError`]；超时与对端不可达视为可重试。
///
/// # Errors
/// 不会失败。
impl From<rdc_transport::TransportError> for RdcError {
    fn from(err: rdc_transport::TransportError) -> Self {
        use rdc_transport::TransportError;
        match err {
            TransportError::InvalidRate => {
                RdcError::new(ErrorCode::InvalidArgument, "目标码率为 0 或非法")
            }
            TransportError::Closed => RdcError::new(ErrorCode::Transport, "传输已关闭"),
            TransportError::Timeout => {
                RdcError::retryable(ErrorCode::Transport, "传输超时", "请检查网络后重试")
            }
            TransportError::PeerUnreachable => {
                RdcError::retryable(ErrorCode::PeerNotFound, "对端不可达", "请确认对方在线且网络可达")
            }
            TransportError::Protocol(e) => RdcError::new(ErrorCode::Transport, e.to_string()),
            TransportError::Internal(msg) => RdcError::new(ErrorCode::Internal, msg),
            TransportError::Io(msg) => {
                RdcError::retryable(ErrorCode::Transport, format!("网络 IO 失败: {msg}"), "请检查网络后重试")
            }
        }
    }
}
