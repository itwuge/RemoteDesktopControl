//! rdc-connect · 候选协商、直连会话与信令客户端
//!
//! 层级：L2 能力层（依赖 rdc-protocol / rdc-security / rdc-transport，S2 起在本 crate
//!       内直接持有 QUIC 端点以完成本机回环验证；该分层偏差已记入 docs/决策待办.md）
//! 用途：候选地址与打洞策略类型、QUIC 信令客户端、直连会话（安全握手 + 控制通道 AEAD）、
//!       连接错误。
//! 调用方：rdc-core（会话建立流程）、rdc-cli（连接入口与 Demo）

// =====** 模块声明 =====**

pub mod candidates;
pub mod error;
pub mod punch;
pub mod quic;
pub mod session;
pub mod signal_client;

// =====** 对外再导出 =====**

pub use candidates::{Candidate, CandidateKind, ObservedAddr};
pub use error::ConnectError;
pub use punch::{HolePunchStrategy, PunchOutcome, PunchPath};
pub use quic::{client_endpoint, read_frame, transport_error, write_frame, MAX_FRAME_LEN};
pub use session::{
    dial, establish, listen, open_control, seal_control, DirectConnection, DirectListener,
    EstablishedSession,
};
pub use signal_client::QuicSignalClient;
