//! rdc-decode / Windows 平台适配
//!
//! 层级：L3 编解码层 · 平台适配最内层
//! 用途：把 Media Foundation 解码器与 D3D11 上屏窗口收敛到统一入口。
//! 调用方：rdc-decode::platform（跨平台门面）、rdc-cli 的 S3b 实测工具

pub mod mf_decoder;

pub use mf_decoder::{
    activation_name, decoder_names, enumerate, ensure_mf, DecodedFrame, MfH264Decoder,
};
