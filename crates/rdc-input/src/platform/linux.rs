//! rdc-input / Linux X11 平台注入器
//!
//! 层级：L3 输入层 · 平台适配
//! 用途：优先用 XTest 扩展注入指针/按键/滚轮（绝对定位 + 按下/抬起成对）；
//!       XTest 不可用时回退到 uinput 虚拟设备（绝对轴 + 闭环校正）。
//! 调用方：rdc-input::platform::create_injector（由 rdc-ctl 在会话建立时调用）
//!
//! 坐标约定：
//! - 进入本层的坐标已是**远端桌面物理像素**（letterbox / 缩放 / 多显示器原点由
//!   [`crate::mapper::ViewTransform`] 完成），平台层只做钳制与绝对坐标归一化，不重复缩放。
//! - X11 的 root 坐标原点固定为 (0,0)，多显示器 CRTC 位置直接落在 root 坐标系里，
//!   因此 XTest 路径直接使用桌面坐标；uinput 路径把视口矩形映射到 0..=32767 绝对轴。
//!
//! 键盘约定：
//! - 协议扫描码仍是 PC/AT Set 1（`0xE0xx` 表示扩展键），本层将其翻译成 X keycode
//!   （evdev 布局，keycode = evdev code + 8）。若被控端改成非 evdev 布局，
//!   按键语义可能偏移（见 research/linux-x11-design.md 风险 12）。
//! - 不根据 `modifiers` 字段自动补发修饰键：修饰键由客户端显式发送按下/抬起，
//!   自动补发会造成卡键。
//!
//! 时延约定：单次注入只做请求入队 + `flush`，不做同步等待（uinput 校正除外，
//! 最多 3 次往返，仅发生在备用通道）。

use std::fs::File;
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::time::Duration;

use tracing::{debug, info, warn};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt as _, Window};
use x11rb::protocol::xtest::ConnectionExt as _;
use x11rb::rust_connection::RustConnection;

use crate::event::{ButtonState, InputEvent, PointerButton};
use crate::injector::{InputInjector, Viewport};
use crate::InputError;

// =====** 常量 =====**

/// XTest 伪造事件类型（X11 协议事件码）。
const MOTION_NOTIFY: u8 = 6;
const BUTTON_PRESS: u8 = 4;
const BUTTON_RELEASE: u8 = 5;
const KEY_PRESS: u8 = 2;
const KEY_RELEASE: u8 = 3;

/// 扩展键前缀（PC/AT Set 1 的 E0，协议用 `0xE0xx` 表达）。
const EXTENDED_SCAN_PREFIX: u16 = 0xE000;

/// 单次 uinput 绝对轴校正的最大往返次数。
const UINPUT_CORRECTION_ATTEMPTS: usize = 3;

/// uinput 绝对轴取值范围（Linux input 惯例）。
const ABS_MAX: i32 = 32_767;

/// 单个滚轮事件一次最多展开的刻度数（防止异常值把注入线程拖慢）。
const WHEEL_MAX_TICKS: i32 = 10;

// =====** uinput 常量与结构 =====**

/// `EV_SYN`：事件分组结束。
const EV_SYN: u16 = 0x00;
/// `EV_KEY`：按键/按钮。
const EV_KEY: u16 = 0x01;
/// `EV_ABS`：绝对轴。
const EV_ABS: u16 = 0x03;
/// `SYN_REPORT`：提交一组事件。
const SYN_REPORT: u16 = 0;
/// 左键。
const BTN_LEFT: u16 = 0x110;
/// 右键。
const BTN_RIGHT: u16 = 0x111;
/// 中键。
const BTN_MIDDLE: u16 = 0x112;
/// 绝对 X 轴。
const ABS_X: u16 = 0x00;
/// 绝对 Y 轴。
const ABS_Y: u16 = 0x01;
/// 创建 / 销毁设备。
const UI_DEV_CREATE: libc::c_ulong = 0x5501;
const UI_DEV_DESTROY: libc::c_ulong = 0x5502;
/// `UI_DEV_SETUP`（`_IOW('U', 3, struct uinput_setup)`，结构 92 字节）。
const UI_DEV_SETUP: libc::c_ulong = 0x405C_5503;
/// `UI_ABS_SETUP`（`_IOW('U', 4, struct uinput_abs_setup)`，结构 28 字节）。
const UI_ABS_SETUP: libc::c_ulong = 0x401C_5504;
/// 允许的事件类型（参数为 int）。
const UI_SET_EVBIT: libc::c_ulong = 0x4004_5564;
/// 允许的按键码（参数为 int）。
const UI_SET_KEYBIT: libc::c_ulong = 0x4004_5565;
/// 允许的绝对轴（参数为 int）。
const UI_SET_ABSBIT: libc::c_ulong = 0x4004_5567;

/// `struct input_id`（来自 `linux/uinput.h`）。
#[repr(C)]
struct InputId {
    bustype: u16,
    vendor: u16,
    product: u16,
    version: u16,
}

/// `struct uinput_setup`（名称沿用内核命名，长度需与内核一致）。
#[repr(C)]
struct UinputSetup {
    id: InputId,
    name: [u8; 80],
    ff_effects_max: u32,
}

/// `struct input_absinfo`。
#[repr(C)]
struct AbsInfo {
    value: i32,
    minimum: i32,
    maximum: i32,
    fuzz: i32,
    flat: i32,
    resolution: i32,
}

/// `struct uinput_abs_setup`（`code` 后隐含 2 字节对齐填充）。
#[repr(C)]
struct UinputAbsSetup {
    code: u16,
    _pad: u16,
    absinfo: AbsInfo,
}

/// `struct input_event`（x86_64 下 `timeval` 为 16 字节，总计 24 字节）。
#[repr(C)]
struct LinuxInputEvent {
    time: libc::timeval,
    type_: u16,
    code: u16,
    value: i32,
}

// =====** 虚拟设备 =====**

/// uinput 虚拟指针/键盘设备（备用注入通道）。
struct UinputDevice {
    /// `/dev/uinput` 句柄；关闭即销毁设备。
    file: File,
}

impl UinputDevice {
    /// 创建并注册虚拟设备（绝对指针 + 键盘 + 三键）。
    ///
    /// # Returns
    /// 已注册设备。
    ///
    /// # Errors
    /// `/dev/uinput` 不可写、ioctl 失败或内核不支持 `UI_ABS_SETUP` 时返回诊断文本。
    fn create() -> Result<Self, String> {
        // 非阻塞打开：设备忙时立即失败，不阻塞会话建立线程。
        let file = std::fs::OpenOptions::new()
            .write(true)
            .custom_flags(libc::O_NONBLOCK)
            .open("/dev/uinput")
            .map_err(|error| format!("打开 /dev/uinput 失败: {error}"))?;
        let fd = file.as_raw_fd();

        // 1) 声明能力：按键 + 绝对轴。
        for event_type in [EV_KEY, EV_ABS] {
            ioctl_uint(fd, UI_SET_EVBIT, u64::from(event_type))?;
        }
        // 键盘码 1..=255 全开，避免逐键维护白名单；鼠标三键额外开启。
        for code in 1..=255u64 {
            ioctl_uint(fd, UI_SET_KEYBIT, code)?;
        }
        for button in [BTN_LEFT, BTN_RIGHT, BTN_MIDDLE] {
            ioctl_uint(fd, UI_SET_KEYBIT, u64::from(button))?;
        }
        ioctl_uint(fd, UI_SET_ABSBIT, u64::from(ABS_X))?;
        ioctl_uint(fd, UI_SET_ABSBIT, u64::from(ABS_Y))?;

        // 2) 绝对轴范围：0..32767。缺省范围是 0..0，必须显式设置，否则绝对定位无效。
        for code in [ABS_X, ABS_Y] {
            let setup = UinputAbsSetup {
                code,
                _pad: 0,
                absinfo: AbsInfo {
                    value: 0,
                    minimum: 0,
                    maximum: ABS_MAX,
                    fuzz: 0,
                    flat: 0,
                    resolution: 0,
                },
            };
            unsafe {
                if libc::ioctl(fd, UI_ABS_SETUP, &setup as *const UinputAbsSetup) < 0 {
                    return Err(format!(
                        "UI_ABS_SETUP({code}) 失败: {}",
                        std::io::Error::last_os_error()
                    ));
                }
            }
        }

        // 3) 设备身份与创建。
        let name = b"rdc-virtual-input";
        let mut setup = UinputSetup {
            id: InputId {
                bustype: 0x03, // BUS_USB
                vendor: 0x1234,
                product: 0x5678,
                version: 1,
            },
            name: [0; 80],
            ff_effects_max: 0,
            };
        setup.name[..name.len()].copy_from_slice(name);
        debug_assert_eq!(std::mem::size_of::<UinputSetup>(), 92);
        debug_assert_eq!(std::mem::size_of::<LinuxInputEvent>(), 24);
        unsafe {
            if libc::ioctl(fd, UI_DEV_SETUP, &setup as *const UinputSetup) < 0 {
                return Err(format!(
                    "UI_DEV_SETUP 失败: {}",
                    std::io::Error::last_os_error()
                ));
            }
            if libc::ioctl(fd, UI_DEV_CREATE) < 0 {
                return Err(format!(
                    "UI_DEV_CREATE 失败: {}",
                    std::io::Error::last_os_error()
                ));
            }
        }
        // 给 udev/X 服务器一点时间识别新设备；这段只在会话建立时发生一次。
        // 实测 30ms 不足以让 Xorg 的 udev 后端完成热插拔并作为指针设备接入。
        std::thread::sleep(Duration::from_millis(800));
        info!("uinput 虚拟设备已创建（备用注入通道）");
        Ok(Self { file })
    }

    /// 写入一条 `input_event`。
    ///
    /// # Arguments
    /// * `type_` - 事件类型。
    /// * `code` - 事件码。
    /// * `value` - 事件值。
    ///
    /// # Returns
    /// 写入成功返回 `Ok(())`。
    ///
    /// # Errors
    /// 写入失败（设备被移除）时返回 [`InputError::InjectionRejected`]。
    fn emit(&mut self, type_: u16, code: u16, value: i32) -> Result<(), InputError> {
        let event = LinuxInputEvent {
            time: libc::timeval {
                tv_sec: 0,
                tv_usec: 0,
            },
            type_,
            code,
            value,
        };
        let bytes = unsafe {
            std::slice::from_raw_parts(
                (&event as *const LinuxInputEvent).cast::<u8>(),
                std::mem::size_of::<LinuxInputEvent>(),
            )
        };
        self.file
            .write_all(bytes)
            .map_err(|error| InputError::InjectionRejectedWithSource {
                source: Box::new(error),
            })
    }

    /// 提交一组事件（`SYN_REPORT`）。
    ///
    /// # Returns
    /// 提交成功返回 `Ok(())`。
    ///
    /// # Errors
    /// 写入失败时返回 [`InputError::InjectionRejected`]。
    fn sync(&mut self) -> Result<(), InputError> {
        self.emit(EV_SYN, SYN_REPORT, 0)
    }

    /// 把桌面坐标写入绝对轴。
    ///
    /// # Arguments
    /// * `x` / `y` - 桌面物理坐标。
    /// * `viewport` - 桌面矩形（用于归一化）。
    ///
    /// # Returns
    /// 写入成功返回 `Ok(())`。
    ///
    /// # Errors
    /// 视口非法或写入失败时返回对应错误。
    fn move_absolute(&mut self, x: i32, y: i32, viewport: Viewport) -> Result<(), InputError> {
        let (ax, ay) = to_abs(x, y, viewport)?;
        self.emit(EV_ABS, ABS_X, ax)?;
        self.emit(EV_ABS, ABS_Y, ay)?;
        self.sync()
    }
}

impl Drop for UinputDevice {
    /// 销毁虚拟设备并关闭文件。
    fn drop(&mut self) {
        unsafe {
            if libc::ioctl(self.file.as_raw_fd(), UI_DEV_DESTROY) < 0 {
                debug!(
                    error = %std::io::Error::last_os_error(),
                    "Drop 阶段销毁 uinput 设备失败，设备节点可能残留"
                );
            }
        }
    }
}

/// 调用整型参数的 uinput ioctl。
///
/// # Arguments
/// * `fd` - 设备文件描述符。
/// * `request` - ioctl 请求码。
/// * `value` - 参数值（内核按 `int` 解释低位）。
///
/// # Returns
/// 成功返回 `Ok(())`。
///
/// # Errors
/// ioctl 返回负值时返回诊断文本。
fn ioctl_uint(fd: i32, request: libc::c_ulong, value: u64) -> Result<(), String> {
    unsafe {
        if libc::ioctl(fd, request, value as libc::c_uint) < 0 {
            return Err(format!(
                "ioctl {request:#x} 失败: {}",
                std::io::Error::last_os_error()
            ));
        }
    }
    Ok(())
}

// =====** 注入通道 =====**

/// 实际可用的注入通道。
enum Channel {
    /// XTest：主路径。
    XTest {
        /// X11 连接。
        conn: RustConnection,
        /// root 窗口。
        root: Window,
    },
    /// uinput：备用路径（可选带上 X 连接做闭环校正）。
    Uinput {
        /// 虚拟设备。
        device: UinputDevice,
        /// 用于位置校正的 X11 连接（可缺省）。
        conn: Option<RustConnection>,
        /// 校正用的 root 窗口。
        root: Option<Window>,
    },
    /// 两条通道均不可用。
    Unavailable,
}

// =====** PlatformInjector =====**

/// Linux 输入注入器。
pub struct PlatformInjector {
    /// 远端桌面矩形；未设置时回退到 root 几何。
    viewport: Option<Viewport>,
    /// 指针按键下压状态（避免拖动中重复按下）。
    buttons: ButtonState,
    /// 注入通道。
    channel: Channel,
}

impl PlatformInjector {
    /// 构造注入器：优先 XTest，失败回退 uinput。
    ///
    /// # Returns
    /// 可用通道的注入器；两条通道都不可用时为 [`Channel::Unavailable`]，
    /// 后续注入返回 [`InputError::Unsupported`]（不 panic）。
    ///
    /// # Errors
    /// 不会失败；能力缺失通过通道状态表达。
    pub fn new() -> Self {
        let viewport = None;
        let buttons = ButtonState::new();
        // 测试开关：允许实测脚本强制走 uinput 备用通道（默认不改变生产选择顺序）。
        let force_uinput = std::env::var("RDC_INPUT_FORCE_UINPUT")
            .map(|value| matches!(value.as_str(), "1" | "true" | "yes"))
            .unwrap_or(false);
        match x11rb::connect(None) {
            Ok((conn, screen)) => {
                let root = conn.setup().roots[screen].root;
                if !force_uinput && xtest_available(&conn) {
                    info!("XTest 注入通道就绪");
                    return Self {
                        viewport,
                        buttons,
                        channel: Channel::XTest { conn, root },
                    };
                }
                warn!("按测试开关或 XTest 不可用，尝试 uinput 备用通道");
                match UinputDevice::create() {
                    Ok(device) => {
                        return Self {
                            viewport,
                            buttons,
                            channel: Channel::Uinput {
                                device,
                                conn: Some(conn),
                                root: Some(root),
                            },
                        };
                    }
                    Err(error) => {
                        warn!(error = %error, "uinput 与 XTest 均不可用，输入注入将返回 Unsupported");
                    }
                }
            }
            Err(error) => {
                warn!(error = %error, "X11 连接失败，尝试 uinput 备用通道");
                if let Ok(device) = UinputDevice::create() {
                    return Self {
                        viewport,
                        buttons,
                        channel: Channel::Uinput {
                            device,
                            conn: None,
                            root: None,
                        },
                    };
                }
            }
        }
        Self {
            viewport,
            buttons,
            channel: Channel::Unavailable,
        }
    }

    /// 本次注入使用视口：优先用户设置，否则用 root 几何（原点 0）。
    ///
    /// # Returns
    /// 可用于钳制/归一化的矩形；两条通道都不可用时返回无原点信息的最小矩形。
    ///
    /// # Errors
    /// 不会失败。
    fn target_viewport(&self) -> Viewport {
        if let Some(viewport) = self.viewport.filter(Viewport::is_valid) {
            return viewport;
        }
        // X11 root 坐标原点恒为 (0,0)；读不到几何时用 1×1 兜底（只影响钳制）。
        let root = match &self.channel {
            Channel::XTest { conn, root } => Some((conn, *root)),
            Channel::Uinput {
                conn: Some(conn),
                root: Some(root),
                ..
            } => Some((conn, *root)),
            _ => None,
        };
        if let Some((conn, root)) = root {
            if let Some(reply) = conn
                .get_geometry(root)
                .ok()
                .and_then(|cookie| cookie.reply().ok())
            {
                return Viewport {
                    x: 0,
                    y: 0,
                    width: u32::from(reply.width),
                    height: u32::from(reply.height),
                    scale: 1.0,
                };
            }
        }
        Viewport {
            x: 0,
            y: 0,
            width: 1,
            height: 1,
            scale: 1.0,
        }
    }
}

impl Default for PlatformInjector {
    /// 等价于 [`PlatformInjector::new`]。
    fn default() -> Self {
        Self::new()
    }
}

// =====** InputInjector 实现 =====**

impl InputInjector for PlatformInjector {
    /// 注入单个事件。
    ///
    /// # Arguments
    /// * `event` - 已完成坐标映射的输入事件。
    ///
    /// # Returns
    /// 注入成功（或事件被有意忽略，例如未知扫描码）返回 `Ok(())`。
    ///
    /// # Errors
    /// 无可用通道返回 [`InputError::Unsupported`]；X 请求/设备写入失败返回
    /// [`InputError::InjectionRejected`]；触摸事件返回 [`InputError::Unsupported`]。
    fn inject(&mut self, event: InputEvent) -> Result<(), InputError> {
        let viewport = self.target_viewport();
        match &mut self.channel {
            Channel::XTest { conn, root } => {
                inject_xtest(conn, *root, viewport, event, &mut self.buttons)
            }
            Channel::Uinput { device, conn, root } => {
                inject_uinput(device, conn.as_ref(), *root, viewport, event, &mut self.buttons)
            }
            Channel::Unavailable => Err(InputError::Unsupported),
        }
    }

    /// 更新远端桌面矩形；非法值被忽略（保留上一次有效值）。
    ///
    /// # Arguments
    /// * `viewport` - 新的桌面矩形。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    fn set_viewport(&mut self, viewport: Viewport) {
        if viewport.is_valid() {
            self.viewport = Some(viewport);
        }
    }
}

// =====** XTest 注入 =====**

/// 判断 XTest 扩展是否可用。
///
/// # Arguments
/// * `conn` - X11 连接。
///
/// # Returns
/// `GetVersion` 成功返回 `true`。
///
/// # Errors
/// 不会失败。
fn xtest_available(conn: &RustConnection) -> bool {
    conn.xtest_get_version(2, 2)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .is_some()
}

/// 通过 XTest 注入事件。
///
/// # Arguments
/// * `conn` - X11 连接。
/// * `root` - root 窗口（绝对坐标的参照）。
/// * `viewport` - 桌面矩形（钳制用）。
/// * `event` - 输入事件。
/// * `buttons` - 下压状态（成功后推进）。
///
/// # Returns
/// 请求入队并 `flush` 成功返回 `Ok(())`。
///
/// # Errors
/// 任一 X 请求失败返回 [`InputError::InjectionRejected`]。
fn inject_xtest(
    conn: &RustConnection,
    root: Window,
    viewport: Viewport,
    event: InputEvent,
    buttons: &mut ButtonState,
) -> Result<(), InputError> {
    match event {
        InputEvent::Pointer(pointer) => {
            let (x, y) = clamp_to_i16(viewport.clamp_point(pointer.x, pointer.y));
            // 先移动，再按键：拖动语义与真实鼠标一致。
            fake(conn, MOTION_NOTIFY, 0, root, x, y)?;
            if let Some(button) = pointer.button.as_ref() {
                let transition = button_transition(buttons, button, pointer.pressed);
                if let Some((button, pressed)) = transition {
                    fake_button(conn, root, &button, pressed)?;
                    if pressed {
                        buttons.press(&button);
                    } else {
                        buttons.release(&button);
                    }
                }
            }
        }
        InputEvent::Key(key) => {
            let Some(keycode) = x_keycode(key.scan_code) else {
                // 未知扫描码直接丢弃：报错会打断整条输入流，得不偿失。
                debug!(scan_code = key.scan_code, "未知扫描码，已忽略");
                return Ok(());
            };
            let event_type = if key.pressed { KEY_PRESS } else { KEY_RELEASE };
            fake(conn, event_type, keycode, root, 0, 0)?;
        }
        InputEvent::Wheel(wheel) => {
            let vertical = wheel.delta_y.clamp(-WHEEL_MAX_TICKS, WHEEL_MAX_TICKS);
            for _ in 0..vertical.abs() {
                let button = if vertical > 0 { 5 } else { 4 };
                fake(conn, BUTTON_PRESS, button, root, 0, 0)?;
                fake(conn, BUTTON_RELEASE, button, root, 0, 0)?;
            }
            let horizontal = wheel.delta_x.clamp(-WHEEL_MAX_TICKS, WHEEL_MAX_TICKS);
            for _ in 0..horizontal.abs() {
                let button = if horizontal > 0 { 7 } else { 6 };
                fake(conn, BUTTON_PRESS, button, root, 0, 0)?;
                fake(conn, BUTTON_RELEASE, button, root, 0, 0)?;
            }
        }
        // 桌面注入器不处理触摸。
        InputEvent::Touch(_) => return Err(InputError::Unsupported),
    }
    conn.flush()
        .map_err(|error| InputError::InjectionRejectedWithSource {
            source: Box::new(error),
        })
}

/// 发送一条 XTest 伪造事件。
///
/// # Arguments
/// * `conn` - X11 连接。
/// * `event_type` - X11 事件码。
/// * `detail` - 按键码/按钮号；移动事件忽略。
/// * `root` - root 窗口。
/// * `x` / `y` - root 坐标（移动事件有效）。
///
/// # Returns
/// 请求入队成功返回 `Ok(())`。
///
/// # Errors
/// X 请求失败返回 [`InputError::InjectionRejected`]。
fn fake(
    conn: &RustConnection,
    event_type: u8,
    detail: u8,
    root: Window,
    x: i16,
    y: i16,
) -> Result<(), InputError> {
    conn.xtest_fake_input(event_type, detail, 0, root, x, y, 0)
        .map(|_| ())
        .map_err(|error| InputError::InjectionRejectedWithSource {
            source: Box::new(error),
        })
}

/// 注入按钮按下/抬起。
///
/// # Arguments
/// * `conn` - X11 连接。
/// * `root` - root 窗口。
/// * `button` - 逻辑按键。
/// * `pressed` - 按下为 `true`。
///
/// # Returns
/// 成功返回 `Ok(())`。
///
/// # Errors
/// X 请求失败返回 [`InputError::InjectionRejected`]。
fn fake_button(
    conn: &RustConnection,
    root: Window,
    button: &PointerButton,
    pressed: bool,
) -> Result<(), InputError> {
    let number = match button {
        PointerButton::Left => 1,
        PointerButton::Middle => 2,
        PointerButton::Right => 3,
    };
    let event_type = if pressed { BUTTON_PRESS } else { BUTTON_RELEASE };
    fake(conn, event_type, number, root, 0, 0)
}

/// 计算按键状态迁移（同 Win32 后端：拖动中不重复按下，抬起总是发送）。
///
/// # Arguments
/// * `buttons` - 当前下压状态（只读）。
/// * `button` - 事件涉及的按键。
/// * `pressed` - 事件是否为按下。
///
/// # Returns
/// 需要注入的状态迁移；无需注入返回 `None`。
///
/// # Errors
/// 不会失败。
fn button_transition(
    buttons: &ButtonState,
    button: &PointerButton,
    pressed: bool,
) -> Option<(PointerButton, bool)> {
    if pressed {
        if buttons.is_down(button) {
            None
        } else {
            Some((button.clone(), true))
        }
    } else {
        Some((button.clone(), false))
    }
}

// =====** uinput 注入 =====**

/// 通过 uinput 虚拟设备注入事件（备用通道）。
///
/// # Arguments
/// * `device` - 虚拟设备。
/// * `conn` - 位置校正用的 X11 连接（可为 `None`）。
/// * `root` - 校正用的 root 窗口。
/// * `viewport` - 桌面矩形。
/// * `event` - 输入事件。
/// * `buttons` - 下压状态。
///
/// # Returns
/// 写入成功返回 `Ok(())`。
///
/// # Errors
/// 设备写入失败返回 [`InputError::InjectionRejected`]；触摸返回 [`InputError::Unsupported`]。
fn inject_uinput(
    device: &mut UinputDevice,
    conn: Option<&RustConnection>,
    root: Option<Window>,
    viewport: Viewport,
    event: InputEvent,
    buttons: &mut ButtonState,
) -> Result<(), InputError> {
    match event {
        InputEvent::Pointer(pointer) => {
            let (x, y) = viewport.clamp_point(pointer.x, pointer.y);
            move_with_correction(device, conn, root, viewport, x, y);
            if let Some(button) = pointer.button.as_ref() {
                if let Some((button, pressed)) = button_transition(buttons, button, pointer.pressed) {
                    let code = match button {
                        PointerButton::Left => BTN_LEFT,
                        PointerButton::Middle => BTN_MIDDLE,
                        PointerButton::Right => BTN_RIGHT,
                    };
                    device.emit(EV_KEY, code, i32::from(pressed))?;
                    device.sync()?;
                    if pressed {
                        buttons.press(&button);
                    } else {
                        buttons.release(&button);
                    }
                }
            }
            Ok(())
        }
        InputEvent::Key(key) => {
            let Some(keycode) = x_keycode(key.scan_code) else {
                debug!(scan_code = key.scan_code, "未知扫描码，已忽略");
                return Ok(());
            };
            // X keycode = evdev keycode + 8（默认 evdev 布局）。
            let code = u16::from(keycode).saturating_sub(8);
            if code == 0 {
                return Ok(());
            }
            device.emit(EV_KEY, code, i32::from(key.pressed))?;
            device.sync()
        }
        InputEvent::Wheel(wheel) => {
            // uinput 备用通道没有滚轮轴能力时，退化为按钮 4/5 点击语义由 X 处理，
            // 但虚拟设备未声明 REL_WHEEL，故此处按「不注入」处理并记日志。
            debug!(
                delta_x = wheel.delta_x,
                delta_y = wheel.delta_y,
                "uinput 备用通道暂不注入滚轮"
            );
            Ok(())
        }
        InputEvent::Touch(_) => Err(InputError::Unsupported),
    }
}

/// 写入绝对坐标并做闭环校正（最多 3 次往返）。
///
/// # Arguments
/// * `device` - 虚拟设备。
/// * `conn` / `root` - 校正通道；缺省时只写一次，不做校正。
/// * `viewport` - 桌面矩形。
/// * `x` / `y` - 目标桌面坐标。
///
/// # Returns
/// 无返回值；校正失败只写调试日志（不影响主流程）。
///
/// # Errors
/// 不会失败。
fn move_with_correction(
    device: &mut UinputDevice,
    conn: Option<&RustConnection>,
    root: Option<Window>,
    viewport: Viewport,
    x: i32,
    y: i32,
) {
    let (Some(conn), Some(root)) = (conn, root) else {
        if let Err(err) = device.move_absolute(x, y, viewport) {
            debug!(x, y, error = %err, "无校正源时绝对移动失败");
        }
        return;
    };
    let mut target_x = x;
    let mut target_y = y;
    for _ in 0..UINPUT_CORRECTION_ATTEMPTS {
        if device.move_absolute(target_x, target_y, viewport).is_err() {
            return;
        }
        let actual = conn
            .query_pointer(root)
            .ok()
            .and_then(|cookie| cookie.reply().ok());
        let Some(actual) = actual else {
            return;
        };
        let dx = x - i32::from(actual.root_x);
        let dy = y - i32::from(actual.root_y);
        if dx.abs() <= 2 && dy.abs() <= 2 {
            return;
        }
        // 用偏差反推下一次目标；映射是线性的，通常一次即可收敛。
        let next = viewport.clamp_point(target_x + dx, target_y + dy);
        target_x = next.0;
        target_y = next.1;
    }
    debug!("uinput 绝对定位校正达到上限仍未收敛到 2px 内");
}

// =====** 坐标与扫描码翻译（纯函数，可单测） =====**

/// 桌面坐标 → uinput 绝对轴值。
///
/// # Arguments
/// * `x` / `y` - 桌面物理坐标。
/// * `viewport` - 桌面矩形。
///
/// # Returns
/// `(abs_x, abs_y)`，范围 `0..=32767`。
///
/// # Errors
/// 视口非法时返回 [`InputError::InvalidViewport`]。
fn to_abs(x: i32, y: i32, viewport: Viewport) -> Result<(i32, i32), InputError> {
    if !viewport.is_valid() {
        return Err(InputError::InvalidViewport);
    }
    let (x, y) = viewport.clamp_point(x, y);
    let span_x = (viewport.width - 1).max(1) as f64;
    let span_y = (viewport.height - 1).max(1) as f64;
    let nx = ((x - viewport.x) as f64 / span_x) * f64::from(ABS_MAX);
    let ny = ((y - viewport.y) as f64 / span_y) * f64::from(ABS_MAX);
    Ok((
        nx.round().clamp(0.0, f64::from(ABS_MAX)) as i32,
        ny.round().clamp(0.0, f64::from(ABS_MAX)) as i32,
    ))
}

/// 把坐标钳制到 XTest 的 `i16` 可表达范围。
///
/// # Arguments
/// * `point` - 桌面坐标。
///
/// # Returns
/// `0..=32767` 内的坐标（X11 root 坐标原点为 0）。
///
/// # Errors
/// 不会失败。
fn clamp_to_i16(point: (i32, i32)) -> (i16, i16) {
    (
        point.0.clamp(0, i32::from(i16::MAX)) as i16,
        point.1.clamp(0, i32::from(i16::MAX)) as i16,
    )
}

/// PC/AT Set 1 扫描码 → X keycode（evdev 布局）。
///
/// # Arguments
/// * `scan` - 协议扫描码；`0xE0xx` 表示扩展键。
///
/// # Returns
/// X keycode；未收录的扫描码返回 `None`。
///
/// # Errors
/// 不会失败。
fn x_keycode(scan: u16) -> Option<u8> {
    let extended = scan & EXTENDED_SCAN_PREFIX == EXTENDED_SCAN_PREFIX;
    let base = (scan & 0x00FF) as u8;
    let keycode = if extended {
        // 扩展键（方向键、导航键、右 Ctrl/Alt、Win 键等）。
        match base {
            0x1C => 104, // KP Enter
            0x1D => 105, // Right Ctrl
            0x35 => 106, // KP /
            0x37 => 107, // PrintScreen
            0x38 => 108, // Right Alt
            0x47 => 110, // Home
            0x48 => 111, // Up
            0x49 => 112, // PageUp
            0x4B => 113, // Left
            0x4D => 114, // Right
            0x4F => 115, // End
            0x50 => 116, // Down
            0x51 => 117, // PageDown
            0x52 => 118, // Insert
            0x53 => 119, // Delete
            0x5B => 133, // Left Meta
            0x5C => 134, // Right Meta
            0x5D => 135, // Menu
            _ => return None,
        }
    } else {
        match base {
            0x01 => 9,   // Esc
            0x02..=0x0B => 10 + (base - 0x02), // 1..0
            0x0C => 20,  // -
            0x0D => 21,  // =
            0x0E => 22,  // Backspace
            0x0F => 23,  // Tab
            0x10..=0x19 => 24 + (base - 0x10), // Q..P
            0x1A => 34,  // [
            0x1B => 35,  // ]
            0x1C => 36,  // Enter
            0x1D => 37,  // Left Ctrl
            0x1E..=0x26 => 38 + (base - 0x1E), // A..L
            0x27 => 47,  // ;
            0x28 => 48,  // '
            0x29 => 49,  // `
            0x2A => 50,  // Left Shift
            0x2B => 51,  // backslash
            0x2C..=0x32 => 52 + (base - 0x2C), // Z..M
            0x33 => 59,  // ,
            0x34 => 60,  // .
            0x35 => 61,  // /
            0x36 => 62,  // Right Shift
            0x37 => 63,  // KP *
            0x38 => 64,  // Left Alt
            0x39 => 65,  // Space
            0x3A => 66,  // CapsLock
            0x3B..=0x44 => 67 + (base - 0x3B), // F1..F10
            0x45 => 77,  // NumLock
            0x46 => 78,  // ScrollLock
            0x47 => 79,  // KP 7
            0x48 => 80,  // KP 8
            0x49 => 81,  // KP 9
            0x4A => 82,  // KP -
            0x4B => 83,  // KP 4
            0x4C => 84,  // KP 5
            0x4D => 85,  // KP 6
            0x4E => 86,  // KP +
            0x4F => 87,  // KP 1
            0x50 => 88,  // KP 2
            0x51 => 89,  // KP 3
            0x52 => 90,  // KP 0
            0x53 => 91,  // KP .
            0x57 => 95,  // F11
            0x58 => 96,  // F12
            _ => return None,
        }
    };
    Some(keycode)
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 桌面矩形。
    fn rect() -> Viewport {
        Viewport {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            scale: 1.0,
        }
    }

    /// 扫描码翻译：常用键位置正确，扩展键区分左右。
    #[test]
    fn scancodes_map_to_evdev_keycodes() {
        assert_eq!(x_keycode(0x1E), Some(38), "A 键");
        assert_eq!(x_keycode(0x39), Some(65), "空格");
        assert_eq!(x_keycode(0x1C), Some(36), "回车");
        assert_eq!(x_keycode(0x02), Some(10), "数字 1");
        assert_eq!(x_keycode(0xE048), Some(111), "上方向键");
        assert_eq!(x_keycode(0xE04D), Some(114), "右方向键");
        assert_eq!(x_keycode(0xE01D), Some(105), "右 Ctrl");
        assert_eq!(x_keycode(0x1D), Some(37), "左 Ctrl");
        assert_eq!(x_keycode(0xFFFF), None, "未知扫描码");
    }

    /// 绝对轴：角落映射到 0/32767，非法视口报错。
    #[test]
    fn absolute_axis_maps_corners() {
        assert_eq!(to_abs(0, 0, rect()).expect("左上"), (0, 0));
        assert_eq!(to_abs(1919, 1079, rect()).expect("右下"), (ABS_MAX, ABS_MAX));
        assert_eq!(to_abs(5000, -5000, rect()).expect("钳制"), (ABS_MAX, 0));
        let invalid = Viewport {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            scale: 1.0,
        };
        assert!(matches!(
            to_abs(0, 0, invalid),
            Err(InputError::InvalidViewport)
        ));
    }

    /// 负原点视口（多屏拼接）映射到绝对轴原点 0。
    #[test]
    fn negative_origin_absolute_axis() {
        let two = Viewport {
            x: -1920,
            y: 0,
            width: 3840,
            height: 1080,
            scale: 1.0,
        };
        assert_eq!(to_abs(-1920, 0, two).expect("左屏左上"), (0, 0));
        assert_eq!(to_abs(1919, 1079, two).expect("右屏右下"), (ABS_MAX, ABS_MAX));
    }

    /// 按钮状态迁移：拖动不重复按下；抬起总是发送。
    #[test]
    fn button_transitions_do_not_repeat_down() {
        let mut buttons = ButtonState::new();
        let left = PointerButton::Left;
        assert_eq!(button_transition(&buttons, &left, true), Some((left.clone(), true)));
        buttons.press(&left);
        assert_eq!(button_transition(&buttons, &left, true), None);
        assert_eq!(button_transition(&buttons, &left, false), Some((left.clone(), false)));
        buttons.release(&left);
        assert!(!buttons.is_down(&left));
    }

    /// XTest 坐标钳制到 i16 范围。
    #[test]
    fn xtest_coordinates_are_clamped() {
        assert_eq!(clamp_to_i16((-5, 40_000)), (0, 32_767));
        assert_eq!(clamp_to_i16((100, 100)), (100, 100));
    }
}
