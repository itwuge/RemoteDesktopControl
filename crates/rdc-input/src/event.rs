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

// =====** TextEvent =====**

/// 文本输入事件（S9-Android 新增，兼容旧客户端的逐扫描码路径）。
///
/// 为什么需要独立事件：扫描码只能表达“按下哪个物理键”，无法表达输入法（中文/日文）
/// 的组合串与候选词。本事件用“先删除 N 个 UTF-16 码元 + 再插入一段文本”描述一次编辑：
///
/// - 逐字上屏：`delete_units = 0`，`text = "你"`；
/// - 组合串更新：`delete_units = 上一段组合串长度`，`text = 新组合串`；
/// - 取消组合：`delete_units = 组合串长度`，`text = ""`；
/// - 退格：`delete_units = 1`，`text = ""`。
///
/// 为什么不按“字符”计：Windows `SendInput` 的 `KEYEVENTF_UNICODE` 以 UTF-16 码元
/// 为单位，BMP 外字符（emoji）占两个码元，按码元计数才能让删除与注入严格对齐。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextEvent {
    /// 插入文本前先发送的退格次数（UTF-16 码元数）；`0` 表示纯插入。
    pub delete_units: u16,
    /// 随后插入的文本（UTF-8 原文）；空串表示只删除（退格/取消组合）。
    pub text: String,
}

impl TextEvent {
    /// 是否为空操作（既不删除也不插入）。
    ///
    /// # Returns
    /// `delete_units == 0` 且 `text` 为空时返回 `true`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn is_noop(&self) -> bool {
        self.delete_units == 0 && self.text.is_empty()
    }
}

// =====** ButtonState =====**

/// 指针按键下压状态。
///
/// 协议里“按下”与“拖动中”都是 `button=Some, pressed=true`（见 [`PointerEvent`]），
/// 因此注入端与合并端都必须维护一份下压状态：第一次看到某键为按下 → 注入 DOWN，
/// 之后同键的按下事件都是拖动中的移动，不得重复注入 DOWN。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ButtonState {
    /// 位图，见 [`ButtonState::bit`]。
    bits: u8,
}

impl ButtonState {
    /// 空状态（所有键都抬起）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 按键对应的位。
    fn bit(button: &PointerButton) -> u8 {
        match button {
            PointerButton::Left => 1,
            PointerButton::Right => 2,
            PointerButton::Middle => 4,
        }
    }

    /// 该键当前是否处于按下状态。
    pub fn is_down(&self, button: &PointerButton) -> bool {
        self.bits & Self::bit(button) != 0
    }

    /// 标记该键为按下。
    pub fn press(&mut self, button: &PointerButton) {
        self.bits |= Self::bit(button);
    }

    /// 标记该键为抬起。
    pub fn release(&mut self, button: &PointerButton) {
        self.bits &= !Self::bit(button);
    }

    /// 会话结束或重连后清空（避免对端以为键还按着）。
    pub fn clear(&mut self) {
        self.bits = 0;
    }
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
    /// 文本事件（输入法提交/组合/删除）；新增变体只追加，旧客户端按未知 tag 拒绝。
    Text(TextEvent),
}
