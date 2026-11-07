//! rdc-protocol · 协议类型与版本常量的唯一真源
//!
//! 层级：L2 能力层（最底层，无任何 IO 与平台依赖）
//! 用途：定义媒体包头（32 字节）、通道表与优先级、能力位图、信令信封与消息、
//!       挑战-应答格式、时间码与编码器 trait；供 rdc-signal、rdc-relay、
//!       rdc-transport、rdc-connect、rdc-core 与 rdc-bridge 共同引用
//! 调用方：所有其它 crate（见 docs/02 §1 依赖图）
//!
//! 纪律：本 crate 只允许标准库 + serde + base64。任何文件不得出现 socket、
//! 线程或平台 API。S2 起本 crate 的线格式被单元测试冻结，改动等同协议变更。

// =====** 模块声明 =====**

pub mod capabilities;
pub mod challenge;
pub mod codec;
pub mod decode_queue;
pub mod error;
pub mod header;
pub mod message;
pub mod ports;
pub mod stream;
pub mod timestamp;
pub mod version;
pub mod video;

// =====** 对外再导出 =====**

pub use capabilities::{bits as capability_bits, Capabilities};
pub use codec::{CodecKind, EncodedFrame, Encoder, EncoderConfig, RawFrame};
pub use decode_queue::DecodeQueue;
pub use error::{ErrorCode, ProtocolError};
pub use header::{
    PacketFlags, PacketHeader, AEAD_TAG_LEN, DECODE_QUEUE_DEPTH, FRAGMENT_PAYLOAD_MAX,
    MAX_PACKET_SIZE, PACKET_HEADER_LEN, TRANSPORT_VERSION,
};
pub use message::{
    CallMode, CandidateDesc, CandidateProto, PunchRole, SignalEnvelope, SignalErrorCode,
    SignalMessage, SignalPath, MAX_SIGNAL_PACKET,
};
pub use ports::{PORT_HEALTH, PORT_RELAY_CONTROL, PORT_RELAY_MEDIA, PORT_SIGNAL};
pub use stream::StreamKind;
pub use timestamp::{deadline_us, FrameTiming, LatencyWindow, TIMING_CSV_HEADER};
pub use version::{
    version_string, PROTOCOL_VERSION, PROTOCOL_VERSION_MAJOR, PROTOCOL_VERSION_MINOR,
    SIGNAL_VERSION,
};
pub use video::{
    decode_video_message, encode_video_message, VideoCodec, VideoColorRange, VideoColorSpace,
    VideoEvent, VideoFrame, VideoFrameHeader, VideoMessage, VideoReceiver, VideoStreamConfig,
    VIDEO_FRAGMENT_BYTES, VIDEO_MAX_FRAME_BYTES, VIDEO_MAX_PARAMETER_SET_BYTES,
    VIDEO_MAX_STREAMS, VIDEO_MEDIA_FRAGMENT_BYTES, VIDEO_REASSEMBLY_WINDOW, VIDEO_STREAM_MAIN,
};
