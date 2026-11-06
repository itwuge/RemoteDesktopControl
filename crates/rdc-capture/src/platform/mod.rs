//! rdc-capture / 平台采集后端选择
//!
//! 层级：L3 采集层 · 平台适配最内层
//! 用途：按 target_os 编译期选择采集实现，统一暴露 create_source 入口
//! 调用方：rdc-capture 的对外再导出、rdc-core（会话建立时创建采集源）

use crate::{CaptureError, CaptureSource};

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

/// 按当前平台创建采集源。
///
/// 调用线程：rdc-ctl（会话建立时调用一次）。
/// 阻塞性：不阻塞（真实后端在 [`CaptureSource::start`] 中才初始化硬件）。
///
/// # Arguments
/// * 无（平台由编译目标决定）。
///
/// # Returns
/// 当前平台的采集源实例；此时尚未绑定显示器与 sink。
///
/// # Errors
/// 非桌面平台返回 [`CaptureError::Unsupported`]；桌面平台 S1 阶段不返回错误。
pub fn create_source() -> Result<Box<dyn CaptureSource>, CaptureError> {
    #[cfg(target_os = "windows")]
    {
        Ok(Box::new(windows::PlatformCapture::new()))
    }

    #[cfg(target_os = "macos")]
    {
        Ok(Box::new(macos::PlatformCapture::new()))
    }

    #[cfg(target_os = "linux")]
    {
        Ok(Box::new(linux::PlatformCapture::new()))
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        // 移动端与其它目标没有采集后端，按契约回退到 Unsupported 而不是编译期报错。
        Err(CaptureError::Unsupported)
    }
}
