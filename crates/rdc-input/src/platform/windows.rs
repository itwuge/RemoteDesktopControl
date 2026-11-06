//! rdc-input / Windows 平台注入器（S1 空壳）
//!
//! 层级：L3 输入层 · 平台适配
//! 用途：Windows 输入注入入口；stage4 落地 SendInput
//! 调用方：rdc-input::platform::create_injector（由 rdc-ctl 在会话建立时调用）

use crate::injector::{InputInjector, Viewport};
use crate::{InputError, InputEvent};

// =====** PlatformInjector =====**

/// Windows 输入注入器（S1 空壳，未调用任何平台 API）。
///
/// TODO(stage4)：使用 `SendInput` 注入指针、键盘与滚轮；多显示器场景先归一化到
/// 0..65535 虚拟桌面坐标再注入，避免 DPI 感知差异。
pub struct PlatformInjector;

impl PlatformInjector {
    /// 构造空壳注入器。
    ///
    /// # Arguments
    /// * 无。
    ///
    /// # Returns
    /// 空壳实例；真实注入后端在 stage4 接入。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new() -> Self {
        Self
    }
}

// =====** InputInjector 实现 =====**

impl InputInjector for PlatformInjector {
    /// 注入单个输入事件。
    ///
    /// # Arguments
    /// * `_event` - 输入事件（S1 未使用）。
    ///
    /// # Returns
    /// stage4 之前不会正常返回。
    ///
    /// # Errors
    /// stage4 在 `SendInput` 被系统拒绝时返回 [`InputError::InjectionRejected`]。
    fn inject(&mut self, _event: InputEvent) -> Result<(), InputError> {
        // TODO(stage4)：SendInput 注入；UIPI/安全桌面会拒绝，需映射为 InjectionRejected。
        todo!("TODO(stage4)：Windows 输入注入（SendInput）在 stage4 实现")
    }

    /// 更新视口。
    ///
    /// # Arguments
    /// * `_viewport` - 新视口（S1 未使用）。
    ///
    /// # Returns
    /// stage4 之前不会正常返回。
    ///
    /// # Errors
    /// stage4 之前不会正常返回。
    fn set_viewport(&mut self, _viewport: Viewport) {
        // TODO(stage4)：缓存视口，与虚拟桌面坐标归一化一起实现。
        todo!("TODO(stage4)：Windows 视口更新在 stage4 实现")
    }
}
