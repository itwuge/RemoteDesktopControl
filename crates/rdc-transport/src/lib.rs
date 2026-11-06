//! rdc-transport · 报文头、FEC、NACK、pacer 与拥塞控制
//!
//! 层级：L2 能力层（依赖 rdc-protocol / rdc-security / rdc-runtime）
//! 用途：32 字节包头收发、通道优先级调度、令牌桶 pacer、NACK 重传、Reed-Solomon FEC、
//!       抖动缓冲与帧重组、GCC 拥塞控制；S3b 起同时提供可直接收发的 UDP 端点。
//! 调用方：rdc-core（收发线程与 pacer 定时器）、rdc-relay（中继转发）、
//!         rdc-connect（打洞成功后的建链路径）、S3b 端到端工具

// =====** 模块声明 =====**

pub mod clock;
pub mod congestion;
pub mod error;
pub mod fec;
pub mod jitter;
pub mod nack;
pub mod packet;
pub mod pacer;
pub mod params;
pub mod reasm;
pub mod scheduler;
pub mod udp;

// =====** 对外再导出 =====**

pub use clock::ClockEstimator;
pub use congestion::{
    create_controller, CongestionController, CongestionKind, FixedController, GccController,
};
pub use error::TransportError;
pub use fec::{FecDecoder, FecEncoder};
pub use jitter::JitterBuffer;
pub use nack::NackCache;
pub use packet::Header as PacketHeader;
pub use pacer::Pacer;
pub use reasm::{FragmentHeader, Reassembler};
pub use scheduler::{QueuedPacket, StreamQueues};
pub use udp::UdpTransport;
