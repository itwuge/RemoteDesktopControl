//! rdc-decode · H.264 解码、解码队列与 D3D11 上屏
//!
//! 层级：L3 编解码层（下行半边）
//! 用途：接收 rdc-transport 的重组码流，用 Media Foundation 硬件解码成 NV12，
//!       按时间码排序后经 D3D11 VideoProcessor 上屏；同时产出解码/呈现统计。
//! 调用方：rdc-core 的媒体接收任务、rdc-cli 的 S3b 实测工具
//!
//! 模块纪律：
//! - [`queue`] 与 [`stats`] 是纯逻辑，可在任意平台单测；
//! - [`platform`] 与 [`present`] 只在 Windows 编译，平台 API 不得泄漏到上层。

pub mod error;
pub mod platform;
pub mod queue;
pub mod stats;

#[cfg(target_os = "windows")]
pub mod present;

pub use error::DecodeError;
pub use platform::MfH264Decoder;
#[cfg(target_os = "windows")]
pub use platform::{decoder_names, OutputKind};
pub use queue::{DecodeQueue, DEFAULT_DEPTH, DEFAULT_WAIT_LIMIT_US};
pub use stats::{DecodeSnapshot, DecodeStats, WINDOW_CAPACITY};

#[cfg(target_os = "windows")]
pub use platform::DecodedFrame;
#[cfg(target_os = "windows")]
pub use present::{h264_decoder_profile_count, PresentWindow};
