//! rdc-codec / 平台编码器后端选择
//!
//! 层级：L3 编解码层 · 平台适配最内层
//! 用途：按 target_os 编译期选择硬件编码实现，统一暴露 create_encoder 入口
//! 调用方：rdc-codec 的对外再导出、rdc-core（会话建立时创建编码器）

use crate::encoder::{CodecKind, ProbeReport, VideoEncoder};
use crate::error::CodecError;

// =====** 平台模块声明 =====**

#[cfg(target_os = "windows")]
pub(crate) mod windows;

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
        // kind 在校验阶段过滤：Windows 后端 S3a 只实现 H.264。
        let _ = kind;
        // 后端分层（docs/决策待办.md S7）：默认 MFT；`RDC_ENCODER=amf` 切到 AMF 原生编码，
        // 打开失败则回退 MFT，并把「最终后端 + 理由」写日志（不静默降级）。
        let requested = std::env::var("RDC_ENCODER")
            .ok()
            .and_then(|v| crate::backend::BackendKind::from_env_value(&v));
        if requested == Some(crate::backend::BackendKind::Amf) {
            match windows::amf::AmfEncoder::try_new() {
                Some(enc) if !enc.probe(&[]).is_empty() => {
                    tracing::info!("[enc] backend=amf reason=显式指定 RDC_ENCODER=amf");
                    return Ok(Box::new(enc));
                }
                _ => {
                    tracing::warn!(
                        "[enc] backend=mf reason=AMF 不可用（rdc-amf.dll 或 AMF 组件探测失败），按回退链退到 MFT"
                    );
                }
            }
        } else if requested == Some(crate::backend::BackendKind::Software) {
            // S7-7 块 3：显式软编诊断路径（只选同步软件 MFT，选不到就失败，不回退硬编）。
            tracing::info!("[enc] backend=mf-sw reason=显式指定 RDC_ENCODER=software");
            return Ok(Box::new(windows::PlatformEncoder::new_software()));
        } else if matches!(
            requested,
            Some(crate::backend::BackendKind::Nvenc) | Some(crate::backend::BackendKind::OneVpl)
        ) {
            tracing::warn!(
                "[enc] backend=mf reason=指定后端 {:?} 尚未接入，按回退链退到 MFT",
                requested
            );
        }
        tracing::info!("[enc] backend=mf reason=默认硬件后端（MFT）");
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
        // Linux 走外部 ffmpeg/libx264 软编；kind 由 configure 校验（当前仅 H.264）。
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

// =====** 能力探测 =====**

/// 探测当前平台的可用编码器能力。
///
/// 调用线程：rdc-ctl（会话建立前一次性调用，阻塞数百毫秒以内）。
/// 用途：S3a 实测报告与「无硬件时明确报不可用」的依据。
///
/// # Returns
/// 硬件/软件编码器能力报告；非 Windows 平台返回空报告（S3a 未落地）。
///
/// # Errors
/// 不会失败；探测错误写进 [`ProbeReport::error`]。
pub fn probe_encoders() -> ProbeReport {
    #[cfg(target_os = "windows")]
    {
        windows::probe_report()
    }

    #[cfg(target_os = "linux")]
    {
        linux::probe_report()
    }

    #[cfg(not(any(target_os = "windows", target_os = "linux")))]
    {
        ProbeReport::default()
    }
}
