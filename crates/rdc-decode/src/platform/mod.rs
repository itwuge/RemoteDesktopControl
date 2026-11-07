//! rdc-decode / 平台门面
//!
//! 层级：L3 编解码层
//! 用途：按目标平台再导出解码与呈现实现；非 Windows 平台给出显式不可用错误，
//!       而不是静默降级（docs/06 §1：平台能力缺失必须显式上报）。
//! 调用方：rdc-decode 根模块、rdc-core 的解码任务

#[cfg(target_os = "windows")]
pub mod windows;

#[cfg(target_os = "windows")]
pub use windows::{decoder_names, DecodedFrame, MfH264Decoder, OutputKind, TextureFrame};

#[cfg(all(target_os = "windows", feature = "ffmpeg"))]
pub use windows::FfmpegH264Decoder;

/// 非 Windows 平台的解码器占位：S3b 只交付 Windows 后端。
#[cfg(not(target_os = "windows"))]
pub struct MfH264Decoder;

#[cfg(not(target_os = "windows"))]
impl MfH264Decoder {
    /// 非 Windows 平台没有 MF 解码器。
    pub fn open(
        _width: u32,
        _height: u32,
        _fps: u32,
        _prefer_hardware: bool,
    ) -> Result<Self, crate::error::DecodeError> {
        Err(crate::error::DecodeError::DecoderUnavailable(
            "当前平台未实现 H.264 解码后端".to_owned(),
        ))
    }
}
