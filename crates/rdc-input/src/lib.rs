//! rdc-input · 输入事件模型、线格式、合并限频、坐标映射与平台注入
//!
//! 层级：L3 输入层（位于 rdc-protocol 之上、rdc-core 之前）
//! 用途：定义输入事件类型、视口、坐标变换与输入注入器 trait；提供输入通道的
//!       二进制线格式与高频移动合并；按 target_os 选择注入后端
//! 调用方：rdc-core（rdc-in 线程：合并 → 编码 → P0 输入队列；对端解码 → 映射 → [`InputInjector::inject`]）、
//!         rdc-bridge（Dart 侧构造事件）
//!
//! 通道约定：输入走 `StreamKind::Input`（stream=2、priority=0，插队发送、不占令牌桶、
//! 不可丢弃），明文载荷由既有 AEAD 路径加密（包头作 AAD）。本 crate 只产出 ≤14 字节明文，
//! 不持有密钥、不直接收发网络包。

// =====** 模块声明 =====**

pub mod coalesce;
pub mod error;
pub mod event;
pub mod injector;
pub mod mapper;
pub mod platform;
pub mod wire;

// =====** 对外再导出 =====**

pub use coalesce::{Emitted, InputCoalescer, DEFAULT_MAX_MOVES_PER_SEC};
pub use error::InputError;
pub use event::{InputEvent, KeyEvent, PointerButton, PointerEvent, TouchEvent, WheelEvent};
pub use injector::{normalize_absolute, InputInjector, Viewport};
pub use mapper::{DefaultInputMapper, InputMapper, TransformMatrix, ViewTransform};
pub use platform::create_injector;
pub use wire::{decode_event, encode_event, MAX_INPUT_EVENT_PAYLOAD};
