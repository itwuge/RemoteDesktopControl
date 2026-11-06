//! rdc-codec / 默认参数常量
//!
//! 层级：L3 编解码层
//! 用途：提供会话默认码率、帧率与编码器线程上限，避免各 crate 重复硬编码
//! 调用方：rdc-core（构造 [`crate::encoder::VideoConfig`]）、rdc-codec 平台后端

// =====** 默认值 =====**

/// 默认目标码率：8 Mbps。
///
/// 取值兼顾 1080p60 桌面场景的清晰度与弱网可用性；实际码率由 rdc-core 按网络估计调整。
pub const DEFAULT_BITRATE_BPS: u32 = 8_000_000;

/// 默认帧率：60 Hz。
pub const DEFAULT_FPS: u32 = 60;

/// 单个编码器的最大工作线程数（也是硬件实例并发上限）。
///
/// 4 是主流硬件编码器（NVENC/AMF/QSV/VideoToolbox）的并发会话安全上限。
pub const ENCODER_THREADS_MAX: usize = 4;
