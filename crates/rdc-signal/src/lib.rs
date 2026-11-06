//! rdc-signal · 信令会合服务器库
//!
//! 层级：L1 服务端
//! 用途：导出 QUIC 信令服务端与会话状态机，供二进制入口和 rdc-cli 的
//!       本机回环 Demo（同进程启动服务端）复用。
//! 调用方：crates/rdc-signal/src/main.rs、crates/rdc-cli
//!
//! 模块划分：
//! - `server`：TLS 1.3 over QUIC 监听、五类请求处理、限流与推送。
//! - `state`：服务端会话状态机（docs/03 §3.8）。

pub mod server;
pub mod state;

pub use server::{SignalServer, SignalServerError, TLS_SERVER_NAME};
pub use state::{transition, SessionEvent, SessionState, SessionTransitionError};
