//! rdc-decode / Windows 平台适配
//!
//! 层级：L3 编解码层 · 平台适配最内层
//! 用途：把 Media Foundation 解码器与 D3D11 上屏窗口收敛到统一入口。
//! 调用方：rdc-decode::platform（跨平台门面）、rdc-cli 的 S3b 实测工具

pub mod mf_decoder;

#[cfg(feature = "ffmpeg")]
pub mod ffmpeg_decoder;

pub use mf_decoder::{
    activation_name, decoder_names, ensure_mf, enumerate, DecodedFrame, MfH264Decoder, OutputKind,
    TextureFrame,
};

#[cfg(feature = "ffmpeg")]
pub use ffmpeg_decoder::FfmpegH264Decoder;
