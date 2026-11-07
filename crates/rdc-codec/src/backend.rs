//! rdc-codec / 编码器后端分层与选择
//!
//! 层级：L3 编解码层 · 后端策略（不落具体硬件 API）
//! 用途：统一后端枚举、能力探测字段、选择与回退顺序、厂商参数映射表。
//! 调用方：rdc-codec::platform::create_encoder、rdc-bridge（状态接口与启动日志）。
//!
//! 设计约束（docs/决策待办.md S7 口径）：
//! - 选择逻辑只按「运行时探测结果」决定，不按显卡型号或厂商字符串猜；
//! - 探测失败按 AMF → NVENC → oneVPL → MFT → 软编 顺序回退，回退理由必须落日志与状态接口；
//! - 未在对应硬件上验证的路径（NVENC / oneVPL）必须显式标记，不得声称通过。

use crate::encoder::CodecKind;

// =====** 后端枚举 =====**

/// 编码器后端。顺序即回退顺序（`Software` 永远最后）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackendKind {
    /// AMD AMF（VCE/AV1 组件，D3D11 纹理直入）。
    Amf,
    /// NVIDIA NVENC。
    Nvenc,
    /// Intel oneVPL。
    OneVpl,
    /// 微软 Media Foundation MFT（兜底硬件路径）。
    Mft,
    /// 软件编码（最后兜底）。
    Software,
}

impl BackendKind {
    /// 稳定标识，用于日志、状态接口与环境变量取值。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Amf => "amf",
            Self::Nvenc => "nvenc",
            Self::OneVpl => "onevpl",
            Self::Mft => "mf",
            Self::Software => "software",
        }
    }

    /// 回退顺序：AMF → NVENC → oneVPL → MFT → 软编。
    pub fn fallback_chain() -> &'static [BackendKind] {
        &[
            BackendKind::Amf,
            BackendKind::Nvenc,
            BackendKind::OneVpl,
            BackendKind::Mft,
            BackendKind::Software,
        ]
    }

    /// 解析环境变量 `RDC_ENCODER` 的取值；`auto`/空/未知值返回 `None`（走能力探测）。
    pub fn from_env_value(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "amf" => Some(Self::Amf),
            "nvenc" => Some(Self::Nvenc),
            "onevpl" | "vpl" => Some(Self::OneVpl),
            "mf" | "mft" => Some(Self::Mft),
            "software" | "soft" => Some(Self::Software),
            "auto" | "" => None,
            _ => None,
        }
    }
}

// =====** 能力探测字段 =====**

/// 单个后端的能力探测结果（探测失败时 `available=false`，`note` 写原因）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendCaps {
    /// 后端种类。
    pub backend: BackendKind,
    /// 后端实现名（如 `amf-h264-d3d11` / `mf-h264-hw`）。
    pub name: String,
    /// 是否为硬件后端。
    pub hardware: bool,
    /// 可用编码格式（H.264/HEVC/AV1）。
    pub codecs: Vec<CodecKind>,
    /// 是否支持 4:4:4。
    pub supports_444: bool,
    /// 是否支持 10-bit。
    pub supports_10bit: bool,
    /// 最大分辨率。
    pub max_width: u32,
    pub max_height: u32,
    /// 最大帧率。
    pub max_fps: u32,
    /// 是否支持低延迟模式（内部排队深度可压到 1）。
    pub low_latency: bool,
    /// 是否支持真正的 CBR（含 VBV 与填充）。
    pub true_cbr: bool,
    /// 驱动版本（探测不到时为空）。
    pub driver_version: String,
    /// 是否在本机对应硬件上实测验证过。
    pub verified_here: bool,
    /// 探测结论/未验证说明（必须写进状态接口与汇报）。
    pub note: String,
}

impl BackendCaps {
    /// 是否可用（`codecs` 非空即视为探测成功）。
    pub fn available(&self) -> bool {
        !self.codecs.is_empty()
    }
}

// =====** 选择结果 =====**

/// 后端选择结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Selection {
    /// 最终选中的后端。
    pub backend: BackendKind,
    /// 选择/回退理由（写日志与状态接口，不允许静默降级）。
    pub reason: String,
    /// 是否在本机硬件上实测过。
    pub verified_here: bool,
    /// 探测到但未选中的后端（用于报告）。
    pub probed: Vec<BackendKind>,
}

/// 选择后端。
///
/// # Arguments
/// * `requested` - 显式指定（来自 `RDC_ENCODER`）；`None` 表示按能力探测。
/// * `caps` - 各后端探测结果（失败项 `available()==false`）。
///
/// # Returns
/// 选中的后端与理由；没有任何可用后端时回退软编并写明原因。
pub fn select_backend(requested: Option<BackendKind>, caps: &[BackendCaps]) -> Selection {
    let probed: Vec<BackendKind> = caps
        .iter()
        .filter(|c| c.available())
        .map(|c| c.backend)
        .collect();

    if let Some(kind) = requested {
        if let Some(c) = caps.iter().find(|c| c.backend == kind && c.available()) {
            return Selection {
                backend: kind,
                reason: format!("显式指定 RDC_ENCODER={}", kind.as_str()),
                verified_here: c.verified_here,
                probed,
            };
        }
        // 显式指定但探测不可用：继续按回退链选，但理由必须说明。
        let reason = format!(
            "显式指定 {} 不可用（{}），按回退链继续",
            kind.as_str(),
            caps.iter()
                .find(|c| c.backend == kind)
                .map(|c| c.note.as_str())
                .unwrap_or("未探测")
        );
        let mut sel = pick_by_chain(caps, &probed);
        sel.reason = format!("{}；{}", sel.reason, reason);
        return sel;
    }

    pick_by_chain(caps, &probed)
}

fn pick_by_chain(caps: &[BackendCaps], probed: &[BackendKind]) -> Selection {
    for kind in BackendKind::fallback_chain() {
        if let Some(c) = caps.iter().find(|c| c.backend == *kind && c.available()) {
            let reason = if *kind == BackendKind::Software {
                format!("未探测到任何硬件编码器，回退软件编码；{}", c.note)
            } else {
                format!("能力探测命中 {}（{}）", kind.as_str(), c.name)
            };
            return Selection {
                backend: *kind,
                reason,
                verified_here: c.verified_here,
                probed: probed.to_vec(),
            };
        }
    }
    Selection {
        backend: BackendKind::Software,
        reason: "所有后端探测失败（含软编兜底不可用），见各后端探测 note".to_string(),
        verified_here: false,
        probed: probed.to_vec(),
    }
}

// =====** 厂商参数映射表 =====**

/// 一条参数在四类后端上的等价/近似映射。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParamMapping {
    /// 语义名（低延迟/码控/排队深度等）。
    pub param: &'static str,
    /// AMF 属性。
    pub amf: &'static str,
    /// NVENC 参数。
    pub nvenc: &'static str,
    /// oneVPL 参数。
    pub onevpl: &'static str,
    /// MFT / ICodecAPI。
    pub mft: &'static str,
    /// 等价性说明（`exact` / `approx` / `none`）。
    pub equivalence: &'static str,
}

/// 显式映射表（语义差异必须写明，不能假设三家等价）。
pub fn mapping_table() -> &'static [ParamMapping] {
    &[
        ParamMapping {
            param: "low_latency_mode",
            amf: "AMF_VIDEO_ENCODER_LOWLATENCY_MODE + Usage=LOW_LATENCY",
            nvenc: "NV_ENC_PRESET_LOW_LATENCY + tuning_info.zerolatency",
            onevpl: "mfxInfoMFX.LowDelayBRC + AsyncDepth=1",
            mft: "CODECAPI_AVLowLatencyMode",
            equivalence: "approx：都只能「降低」排队，不保证深度=1；AMF 的 ULTRA_LOW_LATENCY 在本机 RX580 返回 INVALID_ARG(3)，必须按能力探测回退",
        },
        ParamMapping {
            param: "queue_depth",
            amf: "无属性；由提交节奏约束（本实现一进一出）",
            nvenc: "NV_ENC_INITIALIZE_PARAMS.enableEncodeAsync / 提交节奏",
            onevpl: "AsyncDepth=1",
            mft: "MF_TRANSFORM_ASYNC + 同步 ProcessInput/Output",
            equivalence: "none：只有 oneVPL 有显式 AsyncDepth；其余靠调用方节奏。启动日志必须打印实际驻留帧数",
        },
        ParamMapping {
            param: "rate_control_cbr",
            amf: "AMF_VIDEO_ENCODER_RATE_CONTROL_METHOD=CBR + ENFORCE_HRD + FILLER_DATA",
            nvenc: "NV_ENC_PARAMS_RC_CBR + vbvBufferSize",
            onevpl: "MFX_RATECONTROL_CBR + LowDelayBRC/MBBRC",
            mft: "CODECAPI_AVEncCommonRateControlMode + MeanBitRate",
            equivalence: "approx：三家 CBR 都允许 IDR 帧超发；本机 AMF 实测均值 8.1Mbps、峰值 18~20Mbps（VBV=1 帧预算）",
        },
        ParamMapping {
            param: "vbv_buffer_size",
            amf: "AMF_VIDEO_ENCODER_VBV_BUFFER_SIZE（按 bitrate/fps 给一帧预算）",
            nvenc: "NV_ENC_RC_PARAMS.vbvBufferSize（按帧预算给）",
            onevpl: "mfxInfoMFX.BufferSizeInKB",
            mft: "CODECAPI_AVEncCommonBufferSize",
            equivalence: "exact：都以「缓冲能装多少」表达；单位不同（bit vs KB），换算见实现",
        },
        ParamMapping {
            param: "gop_idr_period",
            amf: "AMF_VIDEO_ENCODER_IDR_PERIOD",
            nvenc: "NV_ENC_RC_PARAMS.gopLength",
            onevpl: "mfxInfoMFX.GopPicSize",
            mft: "CODECAPI_AVEncMPVGOPSize",
            equivalence: "exact：语义一致（IDR 周期，帧数）",
        },
        ParamMapping {
            param: "intra_refresh",
            amf: "AMF_VIDEO_ENCODER_INTRA_REFRESH_NUM_MBS_PER_SLOT",
            nvenc: "NV_ENC_RC_PARAMS.enableIntraRefresh",
            onevpl: "mfxInfoMFX.IntRefType",
            mft: "无稳定等价（MF 不暴露 intra refresh）",
            equivalence: "none/approx：启用时必须同时把 GOP 设为「无限」，否则周期 IDR 抖动；本实现默认关闭 intra refresh，IDR=1s",
        },
        ParamMapping {
            param: "b_frames",
            amf: "AMF_VIDEO_ENCODER_B_PIC_PATTERN=0",
            nvenc: "NV_ENC_INITIALIZE_PARAMS.maxBPictures=0",
            onevpl: "mfxInfoMFX.GopRefDist=1",
            mft: "CODECAPI_AVEncMPVDefaultBPictureCount=0",
            equivalence: "exact：低延迟模式一律 0",
        },
        ParamMapping {
            param: "max_reference_frames",
            amf: "AMF_VIDEO_ENCODER_MAX_NUM_REFRAMES=1",
            nvenc: "NV_ENC_RC_PARAMS 参考帧数 / 默认 1",
            onevpl: "mfxInfoMFX.NumRefFrame=1",
            mft: "CODECAPI_AVEncVideoMaxNumRefFrame=1",
            equivalence: "exact：低延迟模式一律 1",
        },
        ParamMapping {
            param: "lookahead_pre_analysis",
            amf: "AMF_VIDEO_ENCODER_PRE_ANALYSIS_ENABLE=false",
            nvenc: "NV_ENC_RC_PARAMS.lookaheadDepth=0",
            onevpl: "mfxInfoMFX.LookAheadDepth=0",
            mft: "无直接开关（由低延迟模式抑制）",
            equivalence: "approx：MFT 无显式 lookahead 开关",
        },
        ParamMapping {
            param: "repeat_sps_pps",
            amf: "无等价（AMF 由驱动自决）",
            nvenc: "NV_ENC_CONFIG.repeatSPSPPS",
            onevpl: "mfxInfoMFX.ExtParam RepPack/RepPack",
            mft: "无等价",
            equivalence: "none：AMF 无 repeat_sps_pps，不能假设逐 IDR 重复头发",
        },
        ParamMapping {
            param: "filler_data",
            amf: "AMF_VIDEO_ENCODER_FILLER_DATA_ENABLE",
            nvenc: "RC_PARAMS 的 fillerData / vbv 填充",
            onevpl: "mfxInfoMFX.ExtParam（BRC 填充）",
            mft: "无直接开关",
            equivalence: "approx：MFT 不保证严格 CBR 填充",
        },
        ParamMapping {
            param: "quality_preset",
            amf: "AMF_VIDEO_ENCODER_QUALITY_PRESET=SPEED",
            nvenc: "NV_ENC_PRESET_P1..P7（low latency 系列）",
            onevpl: "mfxInfoMFX.…TargetUsage=7(speed)",
            mft: "CODECAPI_AVEncCommonQualityVsSpeed",
            equivalence: "approx：预设内容不同，不能逐项对齐",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps(kind: BackendKind, codecs: Vec<CodecKind>, verified: bool) -> BackendCaps {
        BackendCaps {
            backend: kind,
            name: format!("{}-test", kind.as_str()),
            hardware: kind != BackendKind::Software,
            codecs,
            supports_444: false,
            supports_10bit: true,
            max_width: 3840,
            max_height: 2160,
            max_fps: 60,
            low_latency: true,
            true_cbr: true,
            driver_version: "test".into(),
            verified_here: verified,
            note: String::new(),
        }
    }

    #[test]
    fn fallback_prefers_amf_then_mft() {
        let list = vec![
            caps(BackendKind::Amf, vec![CodecKind::H264], true),
            caps(BackendKind::Mft, vec![CodecKind::H264], true),
        ];
        let sel = select_backend(None, &list);
        assert_eq!(sel.backend, BackendKind::Amf);
        assert!(sel.verified_here);
    }

    #[test]
    fn unavailable_explicit_request_falls_back_with_reason() {
        let list = vec![
            caps(BackendKind::Amf, vec![], false),
            caps(BackendKind::Mft, vec![CodecKind::H264], true),
        ];
        let sel = select_backend(Some(BackendKind::Amf), &list);
        assert_eq!(sel.backend, BackendKind::Mft);
        assert!(sel.reason.contains("显式指定 amf 不可用"));
    }

    #[test]
    fn no_hardware_reports_software_and_reason() {
        let list = vec![caps(BackendKind::Software, vec![CodecKind::H264], false)];
        let sel = select_backend(None, &list);
        assert_eq!(sel.backend, BackendKind::Software);
        assert!(sel.reason.contains("未探测到任何硬件编码器"));
    }

    #[test]
    fn mapping_table_covers_low_latency_and_rate_control() {
        let table = mapping_table();
        for key in ["low_latency_mode", "rate_control_cbr", "queue_depth", "intra_refresh"] {
            assert!(table.iter().any(|m| m.param == key), "缺少映射: {key}");
        }
        // 语义无法等价的项必须显式写明，不能默认 exact。
        let intra = table.iter().find(|m| m.param == "intra_refresh").unwrap();
        assert!(intra.equivalence.starts_with("none"));
    }

    #[test]
    fn env_values_map_to_backends() {
        assert_eq!(BackendKind::from_env_value("AMF"), Some(BackendKind::Amf));
        assert_eq!(BackendKind::from_env_value("mf"), Some(BackendKind::Mft));
        assert_eq!(BackendKind::from_env_value("auto"), None);
        assert_eq!(BackendKind::from_env_value("bogus"), None);
    }
}
