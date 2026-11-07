//! rdc-clipboard / macOS 剪贴板桩
//!
//! 层级：L3 交互层 · 平台适配
//! 用途：占位；macOS 后端（NSPasteboard）尚未实现，所有调用返回 `Unsupported`。
//! 调用方：rdc-clipboard 的平台工厂

use super::Clipboard;
use crate::error::ClipboardError;

// =====** PlatformClipboard =====**

/// macOS 剪贴板（未实现）。
pub struct PlatformClipboard;

impl PlatformClipboard {
    /// 构造桩实现。
    ///
    /// # Returns
    /// 无状态实例。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new() -> Self {
        Self
    }
}

impl Clipboard for PlatformClipboard {
    fn poll_text(&mut self) -> Result<Option<String>, ClipboardError> {
        Err(ClipboardError::Unsupported)
    }

    fn write_text(&mut self, _text: &str) -> Result<(), ClipboardError> {
        Err(ClipboardError::Unsupported)
    }
}
