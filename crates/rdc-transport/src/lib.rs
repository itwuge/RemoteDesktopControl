//! rdc-transport · 报文头、FEC、NACK、pacer 与拥塞控制
//!
//! 层级：L2 能力层（依赖 rdc-protocol / rdc-security / rdc-runtime，不持有 socket）
//! 用途：定义报文头编解码、FEC 编解码、NACK 缓存、发送节流与拥塞控制抽象；
//!       S1 只提供可编译的类型骨架与常量，算法实现在 S2 落地
//! 调用方：rdc-core（收发线程与 pacer 定时器）、rdc-relay（中继转发）、
//!         rdc-connect（打洞成功后的建链路径）

// =====** 模块声明 =====**

pub mod clock;
pub mod congestion;
pub mod error;
pub mod fec;
pub mod nack;
pub mod packet;
pub mod pacer;
pub mod params;

// =====** 对外再导出 =====**

pub use clock::ClockEstimator;
pub use congestion::{
    create_controller, CongestionController, CongestionKind, FixedController, GccController,
};
pub use error::TransportError;
pub use fec::{FecDecoder, FecEncoder};
pub use nack::NackCache;
pub use packet::PacketHeader;
pub use pacer::Pacer;
