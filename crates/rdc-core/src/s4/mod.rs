//! rdc-core / S4 功能层接线
//!
//! 层级：L3 编排层
//! 用途：把输入、剪贴板、文件传输三块能力接到已加密的会话控制流上，
//!       提供可被 CLI / 桥接层直接驱动的任务函数。
//! 调用方：rdc-cli 的 `demo-input` / `demo-keys` / `demo-clipboard` / `demo-file`。
//!
//! 通道：复用 [`EstablishedSession`] 的控制流（AEAD + 重放窗口）；三条任务共用
//! 一个消息空间，靠 [`message::S4Message`] 的 tag 分流。
//!
//! 已知缺口（记 docs/决策待办）：
//! - `StreamKind` 的多流优先级未接入：现有会话只暴露一条有序控制流，
//!   输入 P0 插队无法体现，文件分片会与输入共用同一条流。
//! - 文件 4 MiB 块在本层按 1352 字节分片，接收端重组后才交给 `rdc-filetransfer`。

pub mod clipboard;
pub mod file;
pub mod input;
pub mod message;

use rdc_connect::EstablishedSession;

pub use message::{encode, FILE_FRAGMENT_BYTES};

// =====** S4Error =====**

/// S4 功能层错误。
#[derive(Debug, thiserror::Error)]
pub enum S4Error {
    /// 会话通道收发失败。
    #[error("会话通道失败：{0}")]
    Link(#[from] rdc_connect::ConnectError),
    /// 输入注入失败。
    #[error("输入注入失败：{0}")]
    Input(#[from] rdc_input::InputError),
    /// 剪贴板读写失败。
    #[error("剪贴板失败：{0}")]
    Clipboard(#[from] rdc_clipboard::ClipboardError),
    /// 文件传输状态机失败。
    #[error("文件传输失败：{0}")]
    FileTransfer(#[from] rdc_filetransfer::FileTransferError),
    /// 消息格式或状态机不符合预期。
    #[error("协议错误：{0}")]
    Protocol(String),
    /// 消息格式错误，且底层解析错误已保留。
    #[error("协议错误：{message}")]
    ProtocolWithSource {
        /// 与旧实现逐字一致的短文案。
        message: String,
        /// 底层错误（`TryFromSliceError` 等），用于日志与 downcast 排查。
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },
}

// =====** 收发包装 =====**

/// 发送一条 S4 消息（自动加密）。
///
/// # Arguments
/// * `session` - 已建立的安全会话
/// * `message` - 待发送消息
///
/// # Returns
/// 发送成功返回 `Ok(())`。
///
/// # Errors
/// 编码失败或网络写失败返回 [`S4Error`]。
pub async fn send(
    session: &mut EstablishedSession,
    message: &message::S4Message,
) -> Result<(), S4Error> {
    let payload = message::encode(message)?;
    session.send_control(&payload).await?;
    Ok(())
}

/// 接收一条 S4 消息（自动解密）。
///
/// # Arguments
/// * `session` - 已建立的安全会话
///
/// # Returns
/// 解析后的消息。
///
/// # Errors
/// 网络读、AEAD 校验或消息解析失败返回 [`S4Error`]。
pub async fn recv(session: &mut EstablishedSession) -> Result<message::S4Message, S4Error> {
    let payload = session.recv_control().await?;
    message::decode(&payload)
}
