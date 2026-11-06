//! rdc-input / 非桌面平台注入器桩
//!
//! 层级：L3 输入层 · 平台适配
//! 用途：为移动端与其它目标提供可编译的注入器，注入一律返回 Unsupported
//! 调用方：rdc-input::platform::create_injector（非桌面目标）

use crate::injector::{InputInjector, Viewport};
use crate::{InputError, InputEvent};

// =====** PlatformInjector =====**

/// 非桌面平台的注入器桩：无状态、不做平台调用。
pub struct PlatformInjector;

impl PlatformInjector {
    /// 构造桩实例。
    ///
    /// # Arguments
    /// * 无。
    ///
    /// # Returns
    /// 无状态桩实例。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new() -> Self {
        Self
    }
}

// =====** InputInjector 实现 =====**

impl InputInjector for PlatformInjector {
    /// 注入单个输入事件（永远拒绝）。
    ///
    /// # Arguments
    /// * `_event` - 输入事件（被忽略）。
    ///
    /// # Returns
    /// 不会返回 `Ok`。
    ///
    /// # Errors
    /// 恒返回 [`InputError::Unsupported`]。
    fn inject(&mut self, _event: InputEvent) -> Result<(), InputError> {
        Err(InputError::Unsupported)
    }

    /// 更新视口（空操作）。
    ///
    /// # Arguments
    /// * `_viewport` - 新视口（被忽略）。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    fn set_viewport(&mut self, _viewport: Viewport) {
        // 无后端可持有视口，保留空操作以维持契约。
    }
}
