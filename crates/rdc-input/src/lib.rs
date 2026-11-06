//! rdc-input · 输入事件模型、坐标映射与平台注入
//!
//! 层级：L3 输入层（位于 rdc-protocol 之上、rdc-core 之前）
//! 用途：定义输入事件类型、视口、坐标变换与输入注入器 trait；按 target_os 选择注入后端
//! 调用方：rdc-core（rdc-in 线程消费输入队列后调用 [`InputInjector::inject`]）、rdc-bridge（Dart 侧构造事件）

// =====** 模块声明 =====**

pub mod error;
pub mod event;
pub mod injector;
pub mod mapper;
pub mod platform;

// =====** 对外再导出 =====**

pub use error::InputError;
pub use event::{
    InputEvent, KeyEvent, PointerButton, PointerEvent, TouchEvent, WheelEvent,
};
pub use injector::{InputInjector, Viewport};
pub use mapper::{InputMapper, TransformMatrix};
pub use platform::create_injector;
