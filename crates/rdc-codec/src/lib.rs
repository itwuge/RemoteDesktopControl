//! rdc-codec · 视频编解码抽象与平台后端
//!
//! 层级：L3 编解码层（位于 rdc-capture 之后、rdc-transport 之前）
//! 用途：编码格式/能力/配置类型，编码器与解码器 trait，按 target_os 选择硬件后端
//! 调用方：rdc-core（配置编码器、请求关键帧与运行期重配）、rdc-transport（分片 EncodedPacket）

// =====** 模块声明 =====**

pub mod decoder;
pub mod encoder;
pub mod error;
pub mod params;
pub mod platform;

// =====** 对外再导出 =====**

pub use decoder::VideoDecoder;
pub use encoder::{
    CodecKind, EncodedPacket, EncoderCaps, VideoChroma, VideoConfig, VideoEncoder,
};
pub use error::CodecError;
pub use platform::create_encoder;
