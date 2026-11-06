//! rdc-capture · 屏幕采集抽象与平台后端
//!
//! 层级：L3 采集层（位于 rdc-runtime 之上、rdc-codec 之前）
//! 用途：定义显示器枚举、色度格式、采集配置、帧类型与采集源 trait；按 target_os 分文件提供后端
//! 调用方：rdc-core（创建采集源并驱动 start/reconfigure/stop）、rdc-codec（消费采集帧）

// =====** 模块声明 =====**

pub mod error;
pub mod frame;
pub mod params;
pub mod platform;
pub mod source;

// =====** 对外再导出 =====**

pub use error::CaptureError;
pub use frame::{Frame, FrameMeta};
pub use platform::create_source;
pub use source::{CaptureConfig, CaptureSource, ChromaFormat, DisplayInfo, FrameSink};
