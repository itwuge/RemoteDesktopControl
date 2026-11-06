//! rdc-input / 输入事件类型
//!
//! 层级：L3 输入层
//! 用途：定义可序列化的输入事件模型（指针/键盘/滚轮/触摸），供网络与 FFI 传输
//! 调用方：rdc-core（rdc-in 线程消费）、rdc-bridge（Dart 侧构造）、rdc-input 平台注入器

use serde::{Deserialize, Serialize};

// =====** PointerButton =====**

/// 指针按键。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PointerButton {
    /// 左键。
    Left,
    /// 右键。
    Right,
    /// 中键。
    Middle,
}

// =====** PointerEvent =====**

/// 指针事件（移动、按下、抬起共用同一结构）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PointerEvent {
    /// 远端桌面坐标 X，单位：物理像素（已由 [`crate::mapper::InputMapper`] 映射）。
    pub x: i32,
    /// 远端桌面坐标 Y，单位：物理像素。
    pub y: i32,
    /// 涉及的按键；纯移动事件为 `None`。
    pub button: Option<PointerButton>,
    /// `true` 表示按下或拖动中，`false` 表示抬起或纯移动。
    pub pressed: bool,
}

// =====** KeyEvent =====**

/// 键盘事件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyEvent {
    /// 平台无关扫描码（PC/AT Set 1 语义），确保两端布局差异不影响按键语义。
    pub scan_code: u16,
    /// `true` 表示按下，`false` 表示抬起。
    pub pressed: bool,
    /// 修饰键位掩码：`1=Shift`，`2=Ctrl`，`4=Alt`，`8=Meta`。
    pub modifiers: u8,
}

// =====** WheelEvent =====**

/// 滚轮事件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WheelEvent {
    /// 水平滚动量，单位：刻度，正值为向右。
    pub delta_x: i32,
    /// 垂直滚动量，单位：刻度，正值为向下。
    pub delta_y: i32,
}

// =====** TouchPhase =====**

/// 触摸阶段。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TouchPhase {
    /// 触点按下。
    Down,
    /// 触点移动。
    Move,
    /// 触点抬起。
    Up,
    /// 触摸被系统取消（手势接管、来电等）。
    Cancel,
}

// =====** TouchEvent =====**

/// 触摸事件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TouchEvent {
    /// 触点编号，同一次触摸序列内稳定。
    pub id: u32,
    /// 归一化横坐标 `[0,1]`，与远端画面宽度无关。
    pub x: f32,
    /// 归一化纵坐标 `[0,1]`，与远端画面高度无关。
    pub y: f32,
    /// 触点阶段。
    pub phase: TouchPhase,
}

// =====** InputEvent =====**

/// 单个输入事件。
///
/// 跨线传输与 FFI 均使用此枚举；新增变体只能追加，禁止改变既有语义。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum InputEvent {
    /// 指针事件。
    Pointer(PointerEvent),
    /// 键盘事件。
    Key(KeyEvent),
    /// 滚轮事件。
    Wheel(WheelEvent),
    /// 触摸事件。
    Touch(TouchEvent),
}
