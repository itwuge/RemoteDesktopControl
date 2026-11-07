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
    /// 选中后端是否为硬件编码（以实际选中的后端为准，不取探测列表推断）。
    pub hardware: bool,
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
pub fn create_encoder_selected(kind: CodecKind) -> Result<EncoderSelection, CodecError> {
    #[cfg(target_os = "windows")]
    {
        // kind 在校验阶段过滤：Windows 后端 S3a 只实现 H.264。
        let _ = kind;
        selection::select_windows()
    }

    #[cfg(target_os = "macos")]
    {
        // S1 忽略 kind：stage3 将按 kind 选择 VideoToolbox 的 HEVC / H.264 / AV1。
        let _ = kind;
        Ok(EncoderSelection {
            encoder: Box::new(macos::PlatformEncoder::new()),
            backend: "videotoolbox".to_owned(),
            reason: "macOS 唯一后端（VideoToolbox）".to_owned(),
            hardware: true,
        })
    }

    #[cfg(target_os = "linux")]
    {
        // Linux 走外部 ffmpeg/libx264 软编；kind 由 configure 校验（当前仅 H.264）。
        let _ = kind;
        Ok(EncoderSelection {
            encoder: Box::new(linux::PlatformEncoder::new()),
            backend: "ffmpeg-libx264".to_owned(),
            reason: "Linux 唯一后端（ffmpeg/libx264）".to_owned(),
            hardware: false,
        })
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        // 移动端与其它目标没有硬件编码后端，按契约回退到 Unsupported。
        let _ = kind;
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
    use crate::backend::{self, BackendCaps, BackendKind};
    use crate::encoder::{CodecKind, VideoEncoder};

    use super::{windows, EncoderSelection};

    /// 执行选择。
    ///
    /// 为什么不再手写 if/else：回退顺序与「不可用理由」统一由
    /// [`backend::select_backend`] 决定，这里只负责把各后端探测成
    /// [`BackendCaps`] 并把选中的后端实例化，避免两套选择逻辑各自演化。
    ///
    /// # Errors
    /// 选择本身不失败；软编实例在 `configure` 阶段才可能报错。
    pub fn select_windows() -> Result<EncoderSelection, crate::error::CodecError> {
        let forced = std::env::var("RDC_ENCODER")
            .ok()
            .and_then(|value| BackendKind::from_env_value(&value));

        let mut caps: Vec<BackendCaps> = Vec::new();
        let mut notes: Vec<String> = Vec::new();
        let mut amf_ready: Option<windows::amf::AmfEncoder> = None;
        let mut nvenc_ready: Option<windows::nvenc::NvencEncoder> = None;
        let mut nvenc_note: Option<String> = None;

        // 强制软编/强制 MFT 的既有语义是「不探测更优硬件后端」；
        // 保留该短路只为了避免无谓的 DLL 探测，最终选择仍交给 `select_backend`。
        let probe_hard_chain = !matches!(
            forced,
            Some(BackendKind::Software) | Some(BackendKind::Mft)
        );

        if probe_hard_chain {
            // --- AMF：探测成功的实例直接留给选中时使用，避免二次加载 DLL。 ---
            match windows::amf::AmfEncoder::try_new() {
                Some(encoder) => {
                    let (h264, _hevc) = encoder.probe_caps();
                    if h264 {
                        caps.push(available_caps(
                            BackendKind::Amf,
                            "amf-h264-d3d11",
                            true,
                            3840,
                            2160,
                            60,
                        ));
                        amf_ready = Some(encoder);
                    } else {
                        let note =
                            "AMF 探测失败（rdc-amf.dll 可加载，但 H.264 能力为 0）".to_owned();
                        caps.push(unavailable_caps(BackendKind::Amf, note.clone()));
                        notes.push(note);
                    }
                }
                None => {
                    let note = "AMF 不可用（rdc-amf.dll 缺失或 AMF 组件未初始化）".to_owned();
                    caps.push(unavailable_caps(BackendKind::Amf, note.clone()));
                    notes.push(note);
                }
            }

            // --- NVENC：Phase 1 只探测系统 nvEncodeAPI64.dll，不创建会话。 ---
            match windows::nvenc::NvencEncoder::try_new() {
                Ok(encoder) => {
                    let probed = encoder.probe(&[CodecKind::H264]);
                    if probed.is_empty() {
                        // Phase 1 无真实编码能力：写明 code=5，并丢弃实例。
                        let note = encoder.note().to_owned();
                        caps.push(unavailable_caps(BackendKind::Nvenc, note.clone()));
                        notes.push(note.clone());
                        nvenc_note = Some(note);
                        drop(encoder);
                    } else {
                        caps.push(available_caps(
                            BackendKind::Nvenc,
                            "nvenc-h264",
                            true,
                            3840,
                            2160,
                            60,
                        ));
                        nvenc_ready = Some(encoder);
                    }
                }
                Err(detail) => {
                    let note = detail.note();
                    caps.push(unavailable_caps(BackendKind::Nvenc, note.clone()));
                    notes.push(note.clone());
                    nvenc_note = Some(note);
                }
            }

            // --- oneVPL：保留占位，不伪造能力。 ---
            let note = "oneVPL 未接入（占位实现）".to_owned();
            caps.push(unavailable_caps(BackendKind::OneVpl, note.clone()));
            notes.push(note);
        }

        // --- MFT 硬件：只枚举不激活，避免选择阶段就做类型协商。 ---
        // 强制 MFT 时也必须保留这条探测，否则无法判断「落到硬件还是软编」。
        if probe_hard_chain || forced == Some(BackendKind::Mft) {
            if windows::has_hardware_h264_mft() {
                caps.push(available_caps(
                    BackendKind::Mft,
                    "mf-h264-hw",
                    true,
                    3840,
                    2160,
                    60,
                ));
            } else {
                let note = "MFT 枚举不到硬件 H.264 编码器".to_owned();
                caps.push(unavailable_caps(BackendKind::Mft, note.clone()));
                notes.push(note);
            }
        }

        // --- 软件 MFT：永远最后兜底；真实可用性由 configure 校验。 ---
        caps.push(available_caps(
            BackendKind::Software,
            "mf-h264-sw",
            false,
            1920,
            1080,
            60,
        ));

        let selection = backend::select_backend(forced, &caps);

        // 显式指定 NVENC 但能力不可用：必须留下带 code= 的专用回退行。
        if forced == Some(BackendKind::Nvenc) && selection.backend != BackendKind::Nvenc {
            if let Some(note) = nvenc_note.as_deref() {
                eprintln!(
                    "[enc] nvenc.fallback {note} fallback={} reason={}",
                    selection.backend.as_str(),
                    selection.reason
                );
            }
        }

        // 把「跳过的后端原因」补进最终理由，保证状态接口能解释为什么没用更优后端。
        let mut reason = selection.reason;
        for note in notes {
            if !reason.contains(&note) {
                reason.push('；');
                reason.push_str(&note);
            }
        }

        let selected = selection.backend;
        let encoder: Box<dyn VideoEncoder> = match selected {
            BackendKind::Amf => match amf_ready {
                Some(encoder) => Box::new(encoder),
                // 探测与实例不一致属逻辑异常：退到 MFT，绝不 panic。
                None => Box::new(windows::PlatformEncoder::new()),
            },
            BackendKind::Nvenc => match nvenc_ready {
                Some(encoder) => Box::new(encoder),
                None => Box::new(windows::PlatformEncoder::new()),
            },
            // oneVPL 未实现：真实可用时也会有明确 note；这里只做安全兜底。
            BackendKind::OneVpl => Box::new(windows::PlatformEncoder::new()),
            BackendKind::Mft => Box::new(windows::PlatformEncoder::new()),
            BackendKind::Software => Box::new(windows::PlatformEncoder::new_software()),
        };
        // 「硬件」标记以实际选中的后端为准；oneVPL 未实现，按非硬件兜底记录。
        let hardware = matches!(
            selected,
            BackendKind::Amf | BackendKind::Nvenc | BackendKind::Mft
        );
        Ok(finish(encoder, reason, hardware))
    }

    /// 组装「可用」能力记录。
    fn available_caps(
        backend: BackendKind,
        name: &str,
        hardware: bool,
        max_width: u32,
        max_height: u32,
        max_fps: u32,
    ) -> BackendCaps {
        BackendCaps {
            backend,
            name: name.to_owned(),
            hardware,
            codecs: vec![CodecKind::H264],
            supports_444: false,
            supports_10bit: false,
            max_width,
            max_height,
            max_fps,
            low_latency: true,
            true_cbr: hardware,
            driver_version: String::new(),
            // 本项目的运行期结论必须由实测报告给出，探测阶段不自我声明已验证。
            verified_here: false,
            note: "探测通过（运行期结论见实测报告）".to_owned(),
        }
    }

    /// 组装「不可用」能力记录：`codecs` 为空即视为不可用。
    fn unavailable_caps(backend: BackendKind, note: String) -> BackendCaps {
        BackendCaps {
            backend,
            name: format!("{}-unavailable", backend.as_str()),
            hardware: backend != BackendKind::Software,
            codecs: Vec::new(),
            supports_444: false,
            supports_10bit: false,
            max_width: 0,
            max_height: 0,
            max_fps: 0,
            low_latency: false,
            true_cbr: false,
            driver_version: String::new(),
            verified_here: false,
            note,
        }
    }

    /// 统一落 `encoder.selected` 日志并组装返回值。
    fn finish(
        encoder: Box<dyn VideoEncoder>,
        reason: String,
        hardware: bool,
    ) -> EncoderSelection {
        let backend = encoder.backend_name().to_owned();
        eprintln!("encoder.selected backend={backend} hardware={hardware} reason={reason}");
        EncoderSelection {
            encoder,
            backend,
            reason,
            hardware,
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
