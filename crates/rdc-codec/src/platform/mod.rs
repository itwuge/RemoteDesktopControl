//! rdc-codec / 平台编码器后端选择
//!
//! 层级：L3 编解码层 · 平台适配最内层
//! 用途：按 target_os 编译期选择硬件编码实现，统一暴露 create_encoder 入口
//! 调用方：rdc-codec 的对外再导出、rdc-core（会话建立时创建编码器）

use crate::encoder::{CodecKind, VideoEncoder};
use crate::error::CodecError;

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

/// 按当前平台创建指定格式的编码器。
///
/// 调用线程：rdc-ctl（会话建立时调用一次）。
/// 阻塞性：不阻塞（真实后端的硬件会话在 `configure` 中创建）。
///
/// # Arguments
/// * `kind` - 目标编码格式；S1 仅记录，stage3 由各平台后端按此选择硬件引擎。
///
/// # Returns
/// 当前平台的编码器实例，尚未 configure。
///
/// # Errors
/// 非桌面平台返回 [`CodecError::Unsupported`]；桌面平台 S1 阶段不返回错误。
pub fn create_encoder(kind: CodecKind) -> Result<Box<dyn VideoEncoder>, CodecError> {
    #[cfg(target_os = "windows")]
    {
        // S1 忽略 kind：stage3 将按 kind 选择 NVENC / AMF / QSV。
        let _ = kind;
        Ok(Box::new(windows::PlatformEncoder::new()))
    }

    #[cfg(target_os = "macos")]
    {
        // S1 忽略 kind：stage3 将按 kind 选择 VideoToolbox 的 HEVC / H.264 / AV1。
        let _ = kind;
        Ok(Box::new(macos::PlatformEncoder::new()))
    }

    #[cfg(target_os = "linux")]
    {
        // S1 忽略 kind：stage3 将按 kind 选择 VAAPI 的 HEVC / H.264 / AV1。
        let _ = kind;
        Ok(Box::new(linux::PlatformEncoder::new()))
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        // 移动端与其它目标没有硬件编码后端，按契约回退到 Unsupported。
        let _ = kind;
        Err(CodecError::Unsupported)
    }
}
