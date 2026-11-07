//! rdc-codec · 视频编解码抽象与平台后端
//!
//! 层级：L3 编解码层（位于 rdc-capture 之后、rdc-transport 之前）
//! 用途：编码格式/能力/配置类型，编码器与解码器 trait，按 target_os 选择硬件后端
//! 调用方：rdc-core（配置编码器、请求关键帧与运行期重配）、rdc-transport（分片 EncodedPacket）

// =====** 模块声明 =====**

pub mod backend;
pub mod decoder;
pub mod encoder;
pub mod error;
pub mod metrics;
#[cfg(target_os = "windows")]
pub mod mux;
pub mod nv12;
pub mod params;
pub mod platform;
#[cfg(target_os = "windows")]
pub mod verify;

// =====** 对外再导出 =====**

pub use decoder::VideoDecoder;
pub use encoder::{
    CodecKind, EncodedPacket, EncoderCapability, EncoderCaps, ProbeReport, VideoChroma, VideoConfig,
    VideoEncoder,
};
pub use error::CodecError;
pub use metrics::{EncodeSnapshot, EncodeStats};
pub use platform::{create_encoder, create_encoder_selected, probe_encoders, EncoderSelection};
#[cfg(target_os = "windows")]
pub use mux::{extract_sps_pps, Mp4Writer};
#[cfg(target_os = "windows")]
pub use verify::{verify_mp4, VerifyReport};
