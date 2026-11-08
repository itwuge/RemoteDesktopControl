//! rdc-clipboard / 剪贴板错误类型
//!
//! 层级：L3 交互层
//! 用途：描述剪贴板同步在大小、载荷、校验与平台访问上的失败原因
//! 调用方：rdc-clipboard 各模块、rdc-core（收敛为 RdcError）、rdc-bridge（错误码映射）

use thiserror::Error;

// =====** ClipboardError =====**

/// 剪贴板同步错误。
///
/// 设计要点：`TooLarge` 与 `HashMismatch` 都不可重试，对端应丢弃本条而不是重传；
/// `Platform` 保留系统错误原文，便于日志定位（不含敏感内容）。
#[derive(Debug, Error)]
pub enum ClipboardError {
    /// 文本超过单条上限（默认 1 MiB）。
    #[error("剪贴板内容超限：{size} 字节 > {limit} 字节")]
    TooLarge {
        /// 实际字节数（已归一化换行）。
        size: usize,
        /// 允许上限。
        limit: usize,
    },
    /// 载荷长度不符、截断或未知类型。
    #[error("剪贴板载荷非法")]
    MalformedPayload,
    /// 载荷正文不是合法 UTF-8。
    ///
    /// 对外文案与 [`ClipboardError::MalformedPayload`] 完全一致；单独成变体是为了
    /// 在不改动人读文案的前提下保留底层解析错误，供日志与排查定位（R-C-3）。
    #[error("剪贴板载荷非法")]
    MalformedText {
        /// UTF-8 解析失败原因。
        #[source]
        source: std::str::Utf8Error,
    },
    /// 载荷自带的 SHA-256 与内容不一致（损坏或被篡改）。
    #[error("剪贴板内容哈希不匹配")]
    HashMismatch,
    /// 该平台或该类型尚未实现（图片/文件剪贴板）。
    #[error("剪贴板能力不受支持")]
    Unsupported,
    /// 系统剪贴板调用失败。
    #[error("剪贴板系统调用失败：{0}")]
    Platform(String),
}
