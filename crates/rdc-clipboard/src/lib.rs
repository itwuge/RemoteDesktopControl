//! rdc-clipboard · 剪贴板文本双向同步
//!
//! 层级：L3 交互层（与 rdc-input 同级，独立于音视频热路径）
//! 用途：文本载荷线格式（SHA-256 自校验）、换行归一化、去重与环路防护、
//!       按平台的剪贴板读写
//! 调用方：rdc-core（剪贴板轮询线程：`poll_text` → [`ClipboardSync::on_local_text`] → P1 队列；
//!         接收循环：`on_remote_payload` → `write_text`）
//!
//! 通道约定（docs/04 §2.1）：剪贴板走 `StreamKind::Clipboard`（stream=5，priority=1），
//! 文本单条 ≤1 MiB，按内容哈希去重；载荷由既有 AEAD 路径加密（独立子密钥 k_clipboard）。
//! 图片/文件剪贴板本轮只留 kind 与错误路径，不实现采集与落盘。

// =====** 模块声明 =====**

pub mod error;
pub mod platform;
pub mod sync;
pub mod text;
pub mod wire;

// =====** 对外再导出 =====**

pub use error::ClipboardError;
pub use platform::{create_clipboard, Clipboard};
pub use sync::{ClipboardSync, SyncStats};
pub use text::{to_network, to_platform, Newlines, MAX_TEXT_BYTES};
pub use wire::{decode_text, digest, encode_text, HASH_LEN, HEADER_LEN, MAX_PAYLOAD_BYTES};
