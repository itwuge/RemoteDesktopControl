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

/// 编码器选择结果：实例 + 实际后端 + 选择理由。
///
/// 为什么要把「理由」带出来：`backend.rs` 的契约要求「回退理由必须落日志与状态接口」，
/// 只返回 `Box<dyn VideoEncoder>` 时调用方拿不到为什么没选到更优后端。
pub struct EncoderSelection {
    /// 已创建的编码器实例（尚未 `configure`）。
    pub encoder: Box<dyn VideoEncoder>,
    /// 选择到的后端标识（`amf` / `mf-h264-hw` / `software-mft` 等）。
    pub backend: String,
    /// 选择理由（探测结果/覆盖项/跳过项，逐项可查）。
    pub reason: String,
}

/// 按当前平台创建指定格式的编码器。
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
    Ok(create_encoder_selected(kind)?.encoder)
}

/// 与 [`create_encoder`] 相同，但额外返回「后端 + 理由」，供日志与状态接口使用。
///
/// 选择顺序（Windows）：AMF → NVENC → oneVPL → MFT → 软编，按**实测能力**取第一个可用；
/// `RDC_ENCODER` 只做强制覆盖，不改变默认行为。
///
/// # Errors
/// 非桌面平台返回 [`CodecError::Unsupported`]。
pub fn create_encoder_selected(_kind: CodecKind) -> Result<EncoderSelection, CodecError> {
    #[cfg(target_os = "windows")]
    {
        // kind 在校验阶段过滤：Windows 后端 S3a 只实现 H.264。
        selection::select_windows()
    }

    #[cfg(target_os = "macos")]
    {
        // S1 忽略 kind：stage3 将按 kind 选择 VideoToolbox 的 HEVC / H.264 / AV1。
        Ok(EncoderSelection {
            encoder: Box::new(macos::PlatformEncoder::new()),
            backend: "videotoolbox".to_owned(),
            reason: "macOS 唯一后端（VideoToolbox）".to_owned(),
        })
    }

    #[cfg(target_os = "linux")]
    {
        // Linux 走外部 ffmpeg/libx264 软编；kind 由 configure 校验（当前仅 H.264）。
        Ok(EncoderSelection {
            encoder: Box::new(linux::PlatformEncoder::new()),
            backend: "ffmpeg-libx264".to_owned(),
            reason: "Linux 唯一后端（ffmpeg/libx264）".to_owned(),
        })
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        // 移动端与其它目标没有硬件编码后端，按契约回退到 Unsupported。
        Err(CodecError::Unsupported)
    }
}

// =====** 能力探测 =====**

/// Windows 后端选择：AMF → NVENC → oneVPL → MFT → 软编，按**实测能力**取第一个可用。
///
/// 规则：
/// - 默认路径必须走探测，不允许「默认就是 MFT」；
/// - `RDC_ENCODER` 只做强制覆盖（`amf` 覆盖后仍失败会继续沿回退链往下走）；
/// - 每次选择都落一行 `encoder.selected backend=<name> reason=<探测结果>`，回退不静默。
#[cfg(target_os = "windows")]
mod selection {
    use crate::backend::BackendKind;
    use crate::encoder::VideoEncoder;

    use super::{EncoderSelection, windows};

    /// 执行选择。
    ///
    /// # Errors
    /// 选择本身不失败；软编实例在 `configure` 阶段才可能报错。
    pub fn select_windows() -> Result<EncoderSelection, crate::error::CodecError> {
        let forced = std::env::var("RDC_ENCODER")
            .ok()
            .and_then(|value| BackendKind::from_env_value(&value));
        let mut skipped: Vec<String> = Vec::new();

        // 强制软编（诊断路径）：不再尝试任何硬件后端。
        if forced == Some(BackendKind::Software) {
            return Ok(finish(
                windows::PlatformEncoder::new_software(),
                "显式指定 RDC_ENCODER=software（诊断路径，不回退硬编）".to_owned(),
            ));
        }
        // 强制 MFT：跳过 AMF，直接落到 MFT 硬件路径。
        let try_amf = matches!(forced, None | Some(BackendKind::Amf));
        if forced == Some(BackendKind::Mft) {
            skipped.push("显式指定 RDC_ENCODER=mf".to_owned());
        }

        if try_amf {
            match windows::amf::AmfEncoder::try_new() {
                Some(encoder) if !encoder.probe(&[]).is_empty() => {
                    let reason = if forced == Some(BackendKind::Amf) {
                        "显式指定 RDC_ENCODER=amf，AMF 探测通过".to_owned()
                    } else {
                        "默认探测：AMF H.264 可用（回退链首位）".to_owned()
                    };
                    return Ok(finish(encoder, reason));
                }
                Some(_) => skipped.push(
                    "AMF 探测失败（rdc-amf.dll 可加载，但 H.264 能力为 0）".to_owned(),
                ),
                None => skipped
                    .push("AMF 不可用（rdc-amf.dll 缺失或 AMF 组件未初始化）".to_owned()),
            }
        }
        // NVENC / oneVPL：当前是占位实现，没有可探测的运行时能力。
        skipped.push("NVENC 未接入（占位实现）".to_owned());
        skipped.push("oneVPL 未接入（占位实现）".to_owned());

        // MFT 硬件：只枚举不激活，避免选择阶段就做一遍类型协商。
        if windows::has_hardware_h264_mft() {
            return Ok(finish(
                windows::PlatformEncoder::new(),
                format!("{}；落到 MFT 硬件路径", skipped.join("；")),
            ));
        }
        skipped.push("MFT 枚举不到硬件 H.264 编码器".to_owned());
        Ok(finish(
            windows::PlatformEncoder::new_software(),
            skipped.join("；"),
        ))
    }

    /// 统一落 `encoder.selected` 日志并组装返回值。
    fn finish(encoder: impl VideoEncoder + 'static, reason: String) -> EncoderSelection {
        let backend = encoder.backend_name().to_owned();
        tracing::info!("encoder.selected backend={backend} reason={reason}");
        EncoderSelection {
            encoder: Box::new(encoder),
            backend,
            reason,
        }
    }
}

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
