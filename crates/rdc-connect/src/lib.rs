//! rdc-connect · 候选协商、打洞策略与信令客户端抽象
//!
//! 层级：L2 能力层（依赖 rdc-protocol / rdc-security / rdc-transport，不持有 socket）
//! 用途：定义候选地址、打洞策略与结果、信令客户端 trait 及连接错误；
//!       S1 只提供可编译的类型骨架，打洞与信令实现在 S2 落地
//! 调用方：rdc-core（会话建立流程）、rdc-cli / rdc-bridge（连接入口）

// =====** 模块声明 =====**

pub mod candidates;
pub mod error;
pub mod punch;
pub mod signal_client;

// =====** 对外再导出 =====**

pub use candidates::{Candidate, CandidateKind, ObservedAddr};
pub use error::ConnectError;
pub use punch::{HolePunchStrategy, PunchOutcome, PunchPath};
pub use signal_client::SignalingClient;
