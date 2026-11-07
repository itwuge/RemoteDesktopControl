//! rdc-filetransfer · 分块文件传输状态机
//!
//! 层级：L3 交互层（与音视频热路径隔离）
//! 用途：提供与网络无关的发送/接收状态机——分块计划（默认 4 MiB）、逐块 SHA-256、
//!       整文件 SHA-256、进度回调、已收位图（断点续传）。
//! 调用方：rdc-core 的文件传输任务；本 crate 不碰 transport/capture/codec/decode。
//!
//! 典型流程（发送端）：
//! ```
//! # use rdc_filetransfer::FileSender;
//! # fn demo(src: std::path::PathBuf) {
//! let mut sender = FileSender::open(src, 4 << 20).expect("打开");
//! // 循环：sender.next_chunk()? → 网络发送 → 对端确认后 sender.confirm(index, true)?
//! # }
//! ```
//!
//! 典型流程（接收端）：
//! ```
//! # use rdc_filetransfer::{FileReceiver, AcceptOutcome};
//! # fn demo(dst: std::path::PathBuf, chunk: rdc_filetransfer::Chunk) {
//! let mut receiver = FileReceiver::create(dst, 1024, 4 << 20).expect("创建");
//! let _ = receiver.accept(&chunk).expect("接收");
//! # }
//! ```
//!
//! 断点续传：接收端 [`encode_state`] 生成位图 → 重连后发送端 [`FileSender::resume_with`]、
//! 接收端 [`FileReceiver::resume`] 恢复，已收块不再重发/重写。
//!
//! 约束：本 crate 只产出/消费内存中的分块，明文与 AEAD、通道优先级由既有传输层负责。

#![deny(missing_docs)]

mod digest;
mod error;
mod plan;
mod receiver;
mod sender;
mod state;

#[cfg(test)]
mod tests_support;

pub use digest::{hash_bytes, hash_file};
pub use error::FileTransferError;
pub use plan::{Chunk, ChunkPlan, Progress, DEFAULT_CHUNK_SIZE};
pub use receiver::{AcceptOutcome, FileReceiver};
pub use sender::{FileSender, ProgressHook};
pub use state::{decode_state, encode_state};
