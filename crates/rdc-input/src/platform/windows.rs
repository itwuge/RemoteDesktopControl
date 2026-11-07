//! rdc-input / Windows 平台注入器
//!
//! 层级：L3 输入层 · 平台适配
//! 用途：用 `SendInput` 注入指针移动/按下抬起、键盘与滚轮；坐标按虚拟桌面归一化。
//! 调用方：rdc-input::platform::create_injector（由 rdc-ctl 在会话建立时调用）
//!
//! 坐标与 DPI：
//! - 注入坐标是**物理像素**，经 [`normalize_absolute`] 换算为 0..=65535 的绝对坐标，
//!   配合 `MOUSEEVENTF_VIRTUALDESK` 覆盖整个虚拟桌面（含负原点的左侧/上方副屏）。
//! - `GetSystemMetrics` 的虚拟桌面尺寸依赖进程 DPI 感知：被控端进程必须按显示器
//!   DPI 感知（`rdc-capture` 已在采集侧设置 `PER_MONITOR_AWARE_V2`），否则同一进程
//!   拿到的是逻辑尺寸，物理坐标会被系统再缩放一次。
//! - 多屏：本实现用“虚拟桌面原点 + 总宽高”，已覆盖常见的并排/上下拼接；
//!   逐显示器枚举与每屏混合 DPI 留待接入 `rdc-core` 时按接口补齐。

use std::mem::size_of;

use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSEEVENTF_ABSOLUTE,
    MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN,
    MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEINPUT, MOUSE_EVENT_FLAGS, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetSystemMetrics, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

use crate::event::{ButtonState, PointerButton};
use crate::injector::{normalize_absolute, InputInjector, Viewport};
use crate::{InputError, InputEvent};

/// 一个滚轮刻度对应的 `mouseData` 增量（WHEEL_DELTA）。
const WHEEL_DELTA: i32 = 120;

/// 扩展键扫描码前缀（PC/AT Set 1 的 E0 前缀，本协议用 `0xE0xx` 表达）。
const EXTENDED_SCAN_PREFIX: u16 = 0xE000;

// =====** InputBatch =====**

/// 单次 `SendInput` 的输入批次：最多两个（滚轮 X/Y 各一条），定长无分配。
#[derive(Default)]
struct InputBatch {
    /// 输入项。
    items: [INPUT; 2],
    /// 已写入条数。
    count: usize,
}

impl InputBatch {
    /// 追加一条输入；容量固定为 2。
    fn push(&mut self, input: INPUT) {
        if self.count < self.items.len() {
            self.items[self.count] = input;
            self.count += 1;
        }
    }

    /// 是否为空（无需调用 `SendInput`）。
    fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// 已写入的输入切片。
    fn as_slice(&self) -> &[INPUT] {
        &self.items[..self.count]
    }
}

// =====** PlatformInjector =====**

/// Windows 输入注入器。
///
/// 线程约定：只由 `rdc-in` 线程独占使用；`viewport` 由同一线程更新，不加锁。
pub struct PlatformInjector {
    /// 被控端桌面矩形（物理像素）；未设置时回退到系统虚拟桌面。
    viewport: Option<Viewport>,
    /// 指针按键下压状态：区分“首次按下”与“拖动中”，避免重复注入 DOWN。
    buttons: ButtonState,
}

impl PlatformInjector {
    /// 构造注入器。
    ///
    /// # Returns
    /// 尚未设置视口、所有按键抬起的实例；首次注入时回退到 `GetSystemMetrics` 的虚拟桌面。
    ///
    /// # Errors
    /// 不会失败；系统查询推迟到实际注入时。
    pub fn new() -> Self {
        Self {
            viewport: None,
            buttons: ButtonState::new(),
        }
    }

    /// 本次注入使用的目标矩形。
    fn target_rect(&self) -> Result<Viewport, InputError> {
        if let Some(viewport) = self.viewport.filter(Viewport::is_valid) {
            return Ok(viewport);
        }
        let rect = virtual_desktop();
        if rect.is_valid() {
            Ok(rect)
        } else {
            Err(InputError::InvalidViewport)
        }
    }
}

// =====** InputInjector 实现 =====**

impl InputInjector for PlatformInjector {
    /// 注入单个输入事件。
    ///
    /// # Arguments
    /// * `event` - 已完成坐标映射（桌面物理像素）的输入事件
    ///
    /// # Returns
    /// `SendInput` 接受本次批次时返回 `Ok(())`。
    ///
    /// # Errors
    /// 触摸事件在桌面注入器上返回 [`InputError::Unsupported`]；视口非法返回
    /// [`InputError::InvalidViewport`]；`SendInput` 返回 0（UIPI/安全桌面拦截）返回
    /// [`InputError::InjectionRejected`]。失败不重试，由上层丢弃本批事件。
    fn inject(&mut self, event: InputEvent) -> Result<(), InputError> {
        let rect = self.target_rect()?;
        // 指针事件可能改变下压状态：状态迁移只在 SendInput 成功后生效。
        let (batch, transition) = match event {
            InputEvent::Pointer(p) => {
                let (input, transition) =
                    pointer_input(p.x, p.y, rect, p.button.as_ref(), p.pressed, &self.buttons)?;
                let mut batch = InputBatch::default();
                batch.push(input);
                (batch, transition)
            }
            InputEvent::Key(k) => {
                let mut batch = InputBatch::default();
                batch.push(key_input(k.scan_code, k.pressed));
                (batch, None)
            }
            InputEvent::Wheel(w) => (wheel_batch(w.delta_x, w.delta_y), None),
            // 桌面注入器不处理触摸：触摸屏/手势注入需另行实现（留接口）。
            InputEvent::Touch(_) => return Err(InputError::Unsupported),
        };
        if batch.is_empty() {
            return Ok(());
        }
        let sent = unsafe { SendInput(batch.as_slice(), size_of::<INPUT>() as i32) };
        if sent == 0 {
            return Err(InputError::InjectionRejected);
        }
        if let Some((button, pressed)) = transition {
            if pressed {
                self.buttons.press(&button);
            } else {
                self.buttons.release(&button);
            }
        }
        Ok(())
    }

    /// 更新被控端桌面矩形。
    ///
    /// 非法视口（宽高为 0 或缩放非正）被忽略，保留上一次有效值，避免注入坐标整体错位。
    ///
    /// # Arguments
    /// * `viewport` - 新的桌面矩形（物理像素，原点可为负）
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败；非法值由本方法静默丢弃。
    fn set_viewport(&mut self, viewport: Viewport) {
        if viewport.is_valid() {
            self.viewport = Some(viewport);
        }
    }
}

// =====** 输入构造（纯函数，可单测） =====**

/// 构造指针移动（可带按下/抬起）输入。
///
/// # Arguments
/// * `x`/`y` - 桌面物理坐标
/// * `rect` - 目标桌面矩形
/// * `button` - 涉及按键；纯移动为 `None`
/// * `pressed` - 按下为 `true`
/// * `buttons` - 当前下压状态（只读，状态由调用方在注入成功后更新）
///
/// # Returns
/// 带 `MOVE | ABSOLUTE | VIRTUALDESK` 与对应按钮标志的鼠标输入，以及需要提交的状态迁移
/// `(按键, 是否按下)`；纯移动与拖动中的移动返回 `None`。
///
/// # Errors
/// 视口非法时返回 [`InputError::InvalidViewport`]。
fn pointer_input(
    x: i32,
    y: i32,
    rect: Viewport,
    button: Option<&PointerButton>,
    pressed: bool,
    buttons: &ButtonState,
) -> Result<(INPUT, Option<(PointerButton, bool)>), InputError> {
    let (nx, ny) = normalize_absolute(x, y, rect)?;
    let (flags, transition) = pointer_flags(buttons, button, pressed);
    Ok((mouse_input(nx as i32, ny as i32, 0, flags), transition))
}

/// 指针标志与状态迁移。
///
/// 首次按下注入 DOWN；同键已按下时只移动（拖动），不重复 DOWN；抬起总是注入 UP
/// （即使状态显示未按下，也用于纠偏，避免对端按键卡死）。
fn pointer_flags(
    buttons: &ButtonState,
    button: Option<&PointerButton>,
    pressed: bool,
) -> (MOUSE_EVENT_FLAGS, Option<(PointerButton, bool)>) {
    let mut flags = MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK;
    let Some(button) = button else {
        return (flags, None);
    };
    if pressed {
        if buttons.is_down(button) {
            (flags, None)
        } else {
            flags |= button_flag(button, true);
            (flags, Some((button.clone(), true)))
        }
    } else {
        flags |= button_flag(button, false);
        (flags, Some((button.clone(), false)))
    }
}

/// 按钮标志。
fn button_flag(button: &PointerButton, pressed: bool) -> MOUSE_EVENT_FLAGS {
    match (button, pressed) {
        (PointerButton::Left, true) => MOUSEEVENTF_LEFTDOWN,
        (PointerButton::Left, false) => MOUSEEVENTF_LEFTUP,
        (PointerButton::Right, true) => MOUSEEVENTF_RIGHTDOWN,
        (PointerButton::Right, false) => MOUSEEVENTF_RIGHTUP,
        (PointerButton::Middle, true) => MOUSEEVENTF_MIDDLEDOWN,
        (PointerButton::Middle, false) => MOUSEEVENTF_MIDDLEUP,
    }
}

/// 构造键盘输入：按扫描码注入，避免两端布局差异改变按键语义。
///
/// # Arguments
/// * `scan_code` - PC/AT Set 1 扫描码；`0xE0xx` 表示扩展键
/// * `pressed` - 按下为 `true`
///
/// # Returns
/// `KEYEVENTF_SCANCODE` 输入；扩展键额外带 `KEYEVENTF_EXTENDEDKEY`，抬起带 `KEYEVENTF_KEYUP`。
fn key_input(scan_code: u16, pressed: bool) -> INPUT {
    let extended = scan_code & EXTENDED_SCAN_PREFIX == EXTENDED_SCAN_PREFIX;
    let mut flags: KEYBD_EVENT_FLAGS = KEYEVENTF_SCANCODE;
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if !pressed {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                // 扫描码注入不填虚拟键，交由系统按当前布局解释。
                wVk: VIRTUAL_KEY(0),
                wScan: scan_code & 0x00FF,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// 构造滚轮输入批次：X/Y 各一条，顺序为纵向再横向。
fn wheel_batch(delta_x: i32, delta_y: i32) -> InputBatch {
    let mut batch = InputBatch::default();
    if delta_y != 0 {
        batch.push(mouse_input(
            0,
            0,
            (delta_y.saturating_mul(WHEEL_DELTA)) as u32,
            MOUSEEVENTF_WHEEL,
        ));
    }
    if delta_x != 0 {
        batch.push(mouse_input(
            0,
            0,
            (delta_x.saturating_mul(WHEEL_DELTA)) as u32,
            MOUSEEVENTF_HWHEEL,
        ));
    }
    batch
}

/// 鼠标输入项。
fn mouse_input(dx: i32, dy: i32, mouse_data: u32, flags: MOUSE_EVENT_FLAGS) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: mouse_data,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

/// 系统虚拟桌面矩形（物理像素，原点可为负）。
///
/// # Returns
/// `GetSystemMetrics` 报告的虚拟桌面；查询异常时宽高退化为 1。
fn virtual_desktop() -> Viewport {
    let (x, y, width, height) = unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    };
    Viewport {
        x,
        y,
        width: width.max(1) as u32,
        height: height.max(1) as u32,
        scale: 1.0,
    }
}

// =====** 自检：只验证构造出的 INPUT，不做真实注入 =====**

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::PointerButton;

    /// 主屏矩形。
    fn primary() -> Viewport {
        Viewport {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            scale: 1.0,
        }
    }

    /// 读取鼠标输入的标志位（测试专用）。
    fn mouse_flags(input: &INPUT) -> u32 {
        unsafe { input.Anonymous.mi.dwFlags.0 }
    }

    /// 左/右/中键的按下与抬起标志必须成对正确。
    #[test]
    fn pointer_button_flags_are_correct() {
        let cases = [
            (PointerButton::Left, true, MOUSEEVENTF_LEFTDOWN.0),
            (PointerButton::Left, false, MOUSEEVENTF_LEFTUP.0),
            (PointerButton::Right, true, MOUSEEVENTF_RIGHTDOWN.0),
            (PointerButton::Right, false, MOUSEEVENTF_RIGHTUP.0),
            (PointerButton::Middle, true, MOUSEEVENTF_MIDDLEDOWN.0),
            (PointerButton::Middle, false, MOUSEEVENTF_MIDDLEUP.0),
        ];
        for (button, pressed, expected) in cases {
            // 每个用例都从未按下状态开始，确保 DOWN 只由“首次按下”产生。
            let buttons = ButtonState::new();
            let (input, transition) =
                pointer_input(100, 100, primary(), Some(&button), pressed, &buttons).expect("构造");
            let flags = mouse_flags(&input);
            assert_eq!(flags & expected, expected, "{button:?} pressed={pressed}");
            // 必须带绝对坐标 + 虚拟桌面，否则坐标会被当成相对位移。
            assert_ne!(flags & MOUSEEVENTF_ABSOLUTE.0, 0);
            assert_ne!(flags & MOUSEEVENTF_VIRTUALDESK.0, 0);
            assert_ne!(flags & MOUSEEVENTF_MOVE.0, 0);
            assert_eq!(transition, Some((button.clone(), pressed)));
        }
    }

    /// 拖动中不得重复注入 DOWN；抬起永远注入 UP 并清状态。
    #[test]
    fn drag_does_not_repeat_button_down() {
        let mut buttons = ButtonState::new();
        let left = PointerButton::Left;
        let (first, transition) =
            pointer_input(10, 10, primary(), Some(&left), true, &buttons).expect("构造");
        assert_eq!(transition, Some((left.clone(), true)));
        buttons.press(&left);

        let (drag, transition) =
            pointer_input(20, 20, primary(), Some(&left), true, &buttons).expect("构造");
        assert_eq!(transition, None, "拖动中不重复按下");
        assert_eq!(mouse_flags(&drag) & MOUSEEVENTF_LEFTDOWN.0, 0);
        assert_ne!(mouse_flags(&first) & MOUSEEVENTF_LEFTDOWN.0, 0);

        let (up, transition) =
            pointer_input(30, 30, primary(), Some(&left), false, &buttons).expect("构造");
        assert_eq!(transition, Some((left.clone(), false)));
        assert_ne!(mouse_flags(&up) & MOUSEEVENTF_LEFTUP.0, 0);
    }

    /// 纯移动不带任何按钮标志；坐标已归一化。
    #[test]
    fn pure_move_has_no_button_flag() {
        let buttons = ButtonState::new();
        let (input, transition) =
            pointer_input(1919, 1079, primary(), None, false, &buttons).expect("构造");
        assert_eq!(transition, None);
        let flags = mouse_flags(&input);
        for button in [
            MOUSEEVENTF_LEFTDOWN.0,
            MOUSEEVENTF_LEFTUP.0,
            MOUSEEVENTF_RIGHTDOWN.0,
            MOUSEEVENTF_RIGHTUP.0,
            MOUSEEVENTF_MIDDLEDOWN.0,
            MOUSEEVENTF_MIDDLEUP.0,
        ] {
            assert_eq!(flags & button, 0);
        }
        let (dx, dy) = unsafe { (input.Anonymous.mi.dx, input.Anonymous.mi.dy) };
        assert_eq!((dx, dy), (65535, 65535));
    }

    /// 扩展扫描码带 EXTENDEDKEY；普通键不带；抬起带 KEYUP。
    #[test]
    fn key_flags_follow_scancode() {
        let extended = key_input(0xE048, true);
        let flags = unsafe { extended.Anonymous.ki.dwFlags.0 };
        let scan = unsafe { extended.Anonymous.ki.wScan };
        assert_eq!(scan, 0x48);
        assert_ne!(flags & KEYEVENTF_SCANCODE.0, 0);
        assert_ne!(flags & KEYEVENTF_EXTENDEDKEY.0, 0);
        assert_eq!(flags & KEYEVENTF_KEYUP.0, 0);

        let plain_up = key_input(0x1C, false);
        let flags = unsafe { plain_up.Anonymous.ki.dwFlags.0 };
        assert_eq!(unsafe { plain_up.Anonymous.ki.wScan }, 0x1C);
        assert_eq!(flags & KEYEVENTF_EXTENDEDKEY.0, 0);
        assert_ne!(flags & KEYEVENTF_KEYUP.0, 0);
    }

    /// 滚轮：纵向 → WHEEL、横向 → HWHEEL，一个刻度 120。
    #[test]
    fn wheel_direction_and_step() {
        let batch = wheel_batch(-1, 2);
        assert_eq!(batch.count, 2);
        let first = &batch.as_slice()[0];
        let second = &batch.as_slice()[1];
        assert_ne!(mouse_flags(first) & MOUSEEVENTF_WHEEL.0, 0);
        assert_eq!(unsafe { first.Anonymous.mi.mouseData }, 240);
        assert_ne!(mouse_flags(second) & MOUSEEVENTF_HWHEEL.0, 0);
        // 反向滚动以 u32 补码表达，驱动按有符号解释。
        assert_eq!(unsafe { second.Anonymous.mi.mouseData }, (-120i32) as u32);
        // 零增量不产生任何输入。
        assert!(wheel_batch(0, 0).is_empty());
    }

    /// 非法视口被忽略，保留上一次有效值。
    #[test]
    fn invalid_viewport_is_ignored() {
        let mut injector = PlatformInjector::new();
        injector.set_viewport(primary());
        let bad = Viewport {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            scale: 1.0,
        };
        injector.set_viewport(bad);
        assert_eq!(injector.viewport, Some(primary()));
        assert!(injector.target_rect().is_ok());
    }

    /// 解码后的线上载荷可直接注入：编码 → 解码 → 构造输入，坐标保持一致。
    #[test]
    fn decoded_payload_feeds_injector() {
        use crate::wire::decode_event;

        let event = InputEvent::Pointer(crate::event::PointerEvent {
            x: 500,
            y: 300,
            button: None,
            pressed: false,
        });
        let bytes = crate::wire::encode_event(&event);
        let decoded = decode_event(&bytes).expect("解码");
        let rect = primary();
        assert_eq!(decoded, event);
        if let InputEvent::Pointer(p) = decoded {
            let buttons = ButtonState::new();
            let (input, transition) =
                pointer_input(p.x, p.y, rect, p.button.as_ref(), p.pressed, &buttons)
                    .expect("构造");
            assert_eq!(transition, None, "纯移动不改变按键状态");
            let (dx, dy) = unsafe { (input.Anonymous.mi.dx, input.Anonymous.mi.dy) };
            let (ex, ey) = normalize_absolute(p.x, p.y, rect).expect("归一化");
            assert_eq!((dx, dy), (ex as i32, ey as i32));
        } else {
            panic!("应为指针事件");
        }
    }
}
