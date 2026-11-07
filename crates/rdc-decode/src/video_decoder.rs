//! rdc-decode / 解码后端抽象与选择
//!
//! 层级：L3 编解码层
//! 用途：把「用哪个 H.264 解码后端」收敛到一个 trait 后面，MF 与 FFmpeg 各实现一份；
//!       通过环境变量 `RDC_DECODER=mf|ffmpeg` 选择，MF 始终保留为回退路径。
//! 调用方：rdc-cli 的 S3b-4 实测工具、后续 rdc-core 的解码任务
//!
//! 选择规则：
//! - 以 `ffmpeg` feature 构建时默认 FFmpeg（S3f 实测 p99 35.8~41.2 ms vs MF 340 ms）；
//!   有 D3D11 设备且 `prefer_hardware` 时默认走 d3d11va 硬解并输出 GPU 纹理
//!   （S3e 实测 decode 0.86 ms/帧、delay=0）；`RDC_FFMPEG_MODE=soft` 可强制单线程软解 + 系统内存 NV12。
//! - `RDC_DECODER=mf` 显式回退 Media Foundation；未启用 feature 时只能 MF。
//! - FFmpeg 打开失败时回落 MF，并在 stderr 明确记录（报告里的 `decoder_path` 反映实际值）。

use crate::error::DecodeError;
use crate::platform::{DecodedFrame, MfH264Decoder, OutputKind};

#[cfg(feature = "ffmpeg")]
use crate::platform::FfmpegH264Decoder;

/// 解码后端。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecoderBackend {
    /// Media Foundation（含 DXVA 加速）。
    Mf,
    /// FFmpeg（LGPL 动态链接）。
    Ffmpeg,
}

impl DecoderBackend {
    /// 从 `RDC_DECODER` 读取选择；未设置时按构建 feature 取默认。
    ///
    /// 默认值：启用 `ffmpeg` feature 走 FFmpeg，否则 MF。
    /// `RDC_DECODER=mf` 始终可显式回退，`RDC_DECODER=ffmpeg` 在未编译该后端时会继续落到 MF。
    pub fn from_env() -> Self {
        match std::env::var("RDC_DECODER").unwrap_or_default().as_str() {
            "mf" => Self::Mf,
            "ffmpeg" => Self::Ffmpeg,
            _ => {
                #[cfg(feature = "ffmpeg")]
                {
                    Self::Ffmpeg
                }
                #[cfg(not(feature = "ffmpeg"))]
                {
                    Self::Mf
                }
            }
        }
    }

    /// 报告用字符串。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mf => "mf",
            Self::Ffmpeg => "ffmpeg",
        }
    }
}

/// 解码器能力与状态快照（报告用）。
#[derive(Debug, Clone)]
pub struct DecoderInfo {
    /// 实际后端名。
    pub backend: &'static str,
    /// 具体实现名（MFT 名称 / FFmpeg 配置）。
    pub name: String,
    /// 是否真正 GPU 硬解。
    pub hardware: bool,
    /// 输出缓冲类型。
    pub output: OutputKind,
    /// MFT 声明的 DXVA 能力（仅 MF 有该概念）。
    pub dxva_aware: bool,
    /// 是否枚举到独立硬件 MFT（仅 MF 有该概念）。
    pub hw_mft: bool,
    /// 附加说明（DXVA 能力、每帧解码耗时等）。
    pub detail: String,
}

/// 统一 H.264 解码接口。
pub trait VideoDecoder {
    /// 送入一个 Annex-B 访问单元，每解出一帧回调一次。
    fn feed_each(
        &mut self,
        annexb: &[u8],
        ts_us: u64,
        on_frame: &mut dyn FnMut(DecodedFrame) -> Result<(), DecodeError>,
    ) -> Result<(), DecodeError>;

    /// 只取输出、不送输入。
    fn pump(&mut self) -> Result<Vec<DecodedFrame>, DecodeError>;

    /// 冲刷并取尽剩余帧。
    fn flush(&mut self) -> Result<Vec<DecodedFrame>, DecodeError>;

    /// 已送 − 已出（解码器内部排队帧数）。
    fn in_flight(&self) -> u64;

    /// 能力与状态快照。
    fn info(&self) -> DecoderInfo;
}

impl VideoDecoder for MfH264Decoder {
    fn feed_each(
        &mut self,
        annexb: &[u8],
        ts_us: u64,
        on_frame: &mut dyn FnMut(DecodedFrame) -> Result<(), DecodeError>,
    ) -> Result<(), DecodeError> {
        MfH264Decoder::feed_each(self, annexb, ts_us, on_frame)
    }

    fn pump(&mut self) -> Result<Vec<DecodedFrame>, DecodeError> {
        MfH264Decoder::pump(self)
    }

    fn flush(&mut self) -> Result<Vec<DecodedFrame>, DecodeError> {
        MfH264Decoder::flush(self)
    }

    fn in_flight(&self) -> u64 {
        MfH264Decoder::in_flight(self)
    }

    fn info(&self) -> DecoderInfo {
        DecoderInfo {
            backend: "mf",
            name: self.name().to_owned(),
            hardware: self.hardware_active(),
            output: self.output_kind(),
            dxva_aware: self.dxva_aware(),
            hw_mft: self.uses_hardware_mft(),
            detail: format!(
                "dxva_aware={}, hw_mft={}",
                self.dxva_aware(),
                self.uses_hardware_mft()
            ),
        }
    }
}

#[cfg(feature = "ffmpeg")]
impl VideoDecoder for FfmpegH264Decoder {
    fn feed_each(
        &mut self,
        annexb: &[u8],
        ts_us: u64,
        on_frame: &mut dyn FnMut(DecodedFrame) -> Result<(), DecodeError>,
    ) -> Result<(), DecodeError> {
        FfmpegH264Decoder::feed_each(self, annexb, ts_us, on_frame)
    }

    fn pump(&mut self) -> Result<Vec<DecodedFrame>, DecodeError> {
        FfmpegH264Decoder::pump(self)
    }

    fn flush(&mut self) -> Result<Vec<DecodedFrame>, DecodeError> {
        FfmpegH264Decoder::flush(self)
    }

    fn in_flight(&self) -> u64 {
        FfmpegH264Decoder::in_flight(self)
    }

    fn info(&self) -> DecoderInfo {
        let (feeds, frames, decode_ms, pack_ms) = self.perf();
        let per_frame_decode = if frames > 0 {
            decode_ms / frames as f64
        } else {
            0.0
        };
        let per_frame_pack = if frames > 0 {
            pack_ms / frames as f64
        } else {
            0.0
        };
        let (width, height) = self.dimensions();
        DecoderInfo {
            backend: "ffmpeg",
            name: self.name().to_owned(),
            hardware: self.hardware_active(),
            output: self.output_kind(),
            dxva_aware: false,
            hw_mft: false,
            detail: format!(
                "{width}x{height}, feeds={feeds}, frames={frames}, delay={}, decode={per_frame_decode:.2}ms/帧, nv12_pack={per_frame_pack:.2}ms/帧",
                self.delay_frames()
            ),
        }
    }
}

/// 按配置打开解码器；FFmpeg 失败时显式回落 MF。
///
/// # Arguments
/// * `device` - 呈现窗口的 D3D11 设备（硬解需要）
/// * `width` / `height` / `fps` - 期望输出参数
/// * `prefer_hardware` - 是否优先硬件路径
///
/// # Returns
/// 已打开的解码器与「首选后端」标记；报告应使用 [`VideoDecoder::info`] 的实际值。
///
/// # Errors
/// 两个后端都打不开时返回 [`DecodeError::DecoderUnavailable`]。
pub fn open_video_decoder(
    device: Option<&windows::Win32::Graphics::Direct3D11::ID3D11Device>,
    width: u32,
    height: u32,
    fps: u32,
    prefer_hardware: bool,
) -> Result<Box<dyn VideoDecoder>, DecodeError> {
    match DecoderBackend::from_env() {
        DecoderBackend::Ffmpeg => {
            #[cfg(feature = "ffmpeg")]
            {
                // 默认硬件：有设备就试 d3d11va（S7 玻璃到玻璃口径需要 GPU 直出纹理）；
                // `RDC_FFMPEG_MODE=soft` 显式强制软解，便于矩阵对照。
                let hw = prefer_hardware
                    && device.is_some()
                    && std::env::var("RDC_FFMPEG_MODE").as_deref() != Ok("soft");
                match FfmpegH264Decoder::open_with_device(device, width, height, fps, hw) {
                    Ok(decoder) => return Ok(Box::new(decoder)),
                    Err(error) => {
                        eprintln!("[decode] FFmpeg 后端打开失败，回退 MF: {error}");
                    }
                }
            }
            #[cfg(not(feature = "ffmpeg"))]
            {
                eprintln!("[decode] 未以 ffmpeg feature 构建，回退 MF");
            }
            match device {
                Some(device) => {
                    MfH264Decoder::open_with_device(device, width, height, fps, prefer_hardware)
                        .map(|decoder| Box::new(decoder) as Box<dyn VideoDecoder>)
                }
                None => MfH264Decoder::open(width, height, fps, prefer_hardware)
                    .map(|decoder| Box::new(decoder) as Box<dyn VideoDecoder>),
            }
        }
        DecoderBackend::Mf => match device {
            Some(device) => {
                MfH264Decoder::open_with_device(device, width, height, fps, prefer_hardware)
                    .map(|decoder| Box::new(decoder) as Box<dyn VideoDecoder>)
            }
            None => MfH264Decoder::open(width, height, fps, prefer_hardware)
                .map(|decoder| Box::new(decoder) as Box<dyn VideoDecoder>),
        },
    }
}
