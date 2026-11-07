//! rdc-clipboard / 平台剪贴板入口
//!
//! 层级：L3 交互层 · 平台适配
//! 用途：定义统一的剪贴板访问 trait，并按 target_os 选择实现
//! 调用方：rdc-core（剪贴板轮询线程与接收循环）

use crate::error::ClipboardError;

// =====** 模块声明 =====**

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "windows")]
mod windows;

// =====** Clipboard trait =====**

/// 系统剪贴板文本访问。
///
/// 约定：
/// - 只处理文本；图片/文件剪贴板尚未实现，调用方不得假设有对应能力。
/// - [`Self::poll_text`] 必须能识别“本次变化由 [`Self::write_text`] 自己引起”，
///   返回 `None` 而不是把内容再报一次（与 `sync` 的哈希回环防护形成双保险）。
/// - 实现只由单一轮询线程持有，不需要内部加锁。
pub trait Clipboard: Send {
    /// 轮询本机文本剪贴板。
    ///
    /// # Returns
    /// 自上次调用以来出现新的外部内容时返回 LF 归一化文本；无变化、自写回环或
    /// 剪贴板为空时返回 `None`。
    ///
    /// # Errors
    /// 系统剪贴板被占用或读取失败时返回 [`ClipboardError::Platform`]；
    /// 平台无实现时返回 [`ClipboardError::Unsupported`]。
    fn poll_text(&mut self) -> Result<Option<String>, ClipboardError>;

    /// 直接读取当前剪贴板文本（不更新轮询基线）。
    ///
    /// 用途：演示/诊断需要“写完之后立刻回读”，而 [`Self::poll_text`] 按设计会
    /// 把自写变化当作回环返回 `None`。本方法不参与回环判定。
    ///
    /// # Returns
    /// 当前剪贴板文本（LF 归一化）；剪贴板为空时返回 `None`。
    ///
    /// # Errors
    /// 系统剪贴板被占用或读取失败时返回 [`ClipboardError::Platform`]；
    /// 平台无实现时返回 [`ClipboardError::Unsupported`]。
    fn read_text(&mut self) -> Result<Option<String>, ClipboardError>;

    /// 把 LF 文本写入本机剪贴板（实现内部按平台换行转换）。
    ///
    /// # Arguments
    /// * `text` - 来自远端的 LF 归一化文本
    ///
    /// # Returns
    /// 写入成功返回 `Ok(())`，并记录本次写入的序列号用于回环识别。
    ///
    /// # Errors
    /// 剪贴板被占用、内存分配失败或写入被拒绝时返回 [`ClipboardError::Platform`]；
    /// 平台无实现时返回 [`ClipboardError::Unsupported`]。
    fn write_text(&mut self, text: &str) -> Result<(), ClipboardError>;
}

// =====** 工厂 =====**

/// 创建当前平台的剪贴板实现。
///
/// # Returns
/// 平台实现；不支持的平台返回总是报 `Unsupported` 的桩实现。
///
/// # Errors
/// 不会失败；平台能力缺失在调用时以错误返回。
pub fn create_clipboard() -> Box<dyn Clipboard> {
    #[cfg(target_os = "windows")]
    {
        Box::new(windows::PlatformClipboard::new())
    }
    #[cfg(target_os = "macos")]
    {
        Box::new(macos::PlatformClipboard::new())
    }
    #[cfg(target_os = "linux")]
    {
        Box::new(linux::PlatformClipboard::new())
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        Box::new(UnsupportedClipboard)
    }
}

/// 无实现平台的兜底：所有调用返回 `Unsupported`。
#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
pub struct UnsupportedClipboard;

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
impl Clipboard for UnsupportedClipboard {
    fn poll_text(&mut self) -> Result<Option<String>, ClipboardError> {
        Err(ClipboardError::Unsupported)
    }

    fn write_text(&mut self, _text: &str) -> Result<(), ClipboardError> {
        Err(ClipboardError::Unsupported)
    }

    fn read_text(&mut self) -> Result<Option<String>, ClipboardError> {
        Err(ClipboardError::Unsupported)
    }
}
