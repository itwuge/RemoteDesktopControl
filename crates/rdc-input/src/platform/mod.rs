//! rdc-input / 平台注入器选择
//!
//! 层级：L3 输入层 · 平台适配最内层
//! 用途：按 target_os 编译期选择输入注入实现，统一暴露 create_injector 入口
//! 调用方：rdc-input 的对外再导出、rdc-core（会话建立时创建并交给 rdc-in 线程）

use crate::injector::InputInjector;
use crate::InputError;

// =====** 平台模块声明 =====**

#[cfg(target_os = "windows")]
mod windows;

#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "linux")]
mod linux;

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
mod unsupported;

// =====** 入口 =====**

/// 按当前平台创建输入注入器。
///
/// 调用线程：rdc-ctl（会话建立时调用一次）。
/// 阻塞性：可能阻塞（平台权限检查），禁止在 rdc-in 线程内调用。
///
/// # Arguments
/// * 无（平台由编译目标决定）。
///
/// # Returns
/// 当前平台的注入器实例。
///
/// # Errors
/// 非桌面平台返回 [`InputError::Unsupported`]；桌面平台 S1 阶段不返回错误。
pub fn create_injector() -> Result<Box<dyn InputInjector>, InputError> {
    #[cfg(target_os = "windows")]
    {
        Ok(Box::new(windows::PlatformInjector::new()))
    }

    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(macos::PlatformInjector::new()))
    }

    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(linux::PlatformInjector::new()))
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        // 移动端与其它目标没有注入后端，按契约回退到 Unsupported。
        Err(InputError::Unsupported)
    }
}
