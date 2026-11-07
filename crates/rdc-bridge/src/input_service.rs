// ============================================================
// 文件用途：控制页输入注入服务（S5-4 块 1）
//
// 层级：L4 桥接层（本机回环输入管线：Dart 事件 → S4 线格式 → 合并限频 → SendInput）
// 被谁调用：crates/rdc-bridge/src/api.rs 的 input_* 函数
// 边界：不建网络会话、不暴露裸指针；只做本进程内的输入注入与统计。
//       与 S4 输入通道共用 `rdc-input` 的事件模型、线格式与合并器；网络跳在
//       真实会话接入后由 rdc-core 的 rdc-in 线程承担（本机回环阶段先验证注入链路）。
// ============================================================

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rdc_input::{
    create_injector, decode_event, encode_event, InputCoalescer, InputEvent, InputInjector,
    KeyEvent, PointerButton, PointerEvent, TextEvent, Viewport, WheelEvent,
    DEFAULT_MAX_MOVES_PER_SEC, MAX_TEXT_EVENT_BYTES,
};

/// 合并器 tick 间隔：挂起移动最迟 1 ms 内被冲刷（与 docs/04 §2 的 1 ms tick 一致）。
const TICK_INTERVAL: Duration = Duration::from_millis(1);

// =====** FFI 类型 =====**

/// 一个输入事件（Dart → Rust）。
///
/// 坐标 `x/y` 已是**被控端桌面物理像素**，由 Dart 侧 `ViewportController` 的矩阵
/// （letterbox + DPI + 缩放/平移）换算完成；Rust 侧只做线格式与钳制。
///
/// `kind = "text"` 时只读取 `delete_units` 与 `text`，其余字段忽略
/// （输入法事件没有桌面坐标，也不属于任何按键）。
#[derive(Debug, Clone)]
pub struct InputEventDto {
    /// 事件类型：`move` / `button` / `wheel` / `key` / `text`。
    pub kind: String,
    /// 桌面横坐标（物理像素）。
    pub x: i32,
    /// 桌面纵坐标（物理像素）。
    pub y: i32,
    /// 指针按键：`0` 无、`1` 左、`2` 右、`3` 中（与 S4 线格式编码一致）。
    pub button: u8,
    /// `true` 表示按下（指针）或按下（键盘）。
    pub pressed: bool,
    /// 滚轮水平增量（刻度，正值为右）。
    pub delta_x: i32,
    /// 滚轮垂直增量（刻度，正值为下）。
    pub delta_y: i32,
    /// PC/AT Set 1 扫描码；`0xE0xx` 表示扩展键。
    pub scan_code: u16,
    /// 修饰键位掩码：`1=Shift`、`2=Ctrl`、`4=Alt`、`8=Meta`。
    pub modifiers: u8,
    /// 文本事件：插入前先删除的 UTF-16 码元数（退格次数）。
    pub delete_units: u16,
    /// 文本事件：要插入的 UTF-8 文本；空串表示纯删除。
    pub text: String,
}

/// 输入注入统计快照。
#[derive(Debug, Clone, Default)]
pub struct InputStatsDto {
    /// 输入通道是否运行。
    pub running: bool,
    /// 收到的事件数。
    pub received: u64,
    /// 合并器输出的事件数。
    pub emitted: u64,
    /// 注入成功的事件数。
    pub injected: u64,
    /// 注入被系统拒绝的事件数。
    pub rejected: u64,
    /// 被合并覆盖掉的移动数。
    pub merged: u64,
    /// 被限频推迟过的移动数。
    pub throttled: u64,
    /// 指针事件数。
    pub pointer_events: u64,
    /// 键盘事件数。
    pub key_events: u64,
    /// 滚轮事件数。
    pub wheel_events: u64,
    /// 文本/输入法事件数。
    pub text_events: u64,
    /// 当前按下的键数（会话结束时必须为 0）。
    pub keys_down: u64,
    /// 最近一次指针事件的桌面横坐标。
    pub last_x: i32,
    /// 最近一次指针事件的桌面纵坐标。
    pub last_y: i32,
    /// 注入成功率（`injected / emitted`）。
    pub success_rate: f64,
    /// 最近错误。
    pub last_error: Option<String>,
}

// =====** 计数与管线 =====**

/// 跨线程统计（注入线程与 FFI 线程共享）。
#[derive(Default)]
struct Counters {
    received: AtomicU64,
    emitted: AtomicU64,
    injected: AtomicU64,
    rejected: AtomicU64,
    merged: AtomicU64,
    throttled: AtomicU64,
    pointer: AtomicU64,
    key: AtomicU64,
    wheel: AtomicU64,
    text: AtomicU64,
    keys_down: AtomicU64,
    last_x: AtomicI64,
    last_y: AtomicI64,
    last_error: Mutex<Option<String>>,
}

impl Counters {
    /// 记录一条错误（只保留第一条）。
    fn set_error(&self, message: impl Into<String>) {
        if let Ok(mut slot) = self.last_error.lock() {
            if slot.is_none() {
                *slot = Some(message.into());
            }
        }
    }

    /// 生成快照。
    fn snapshot(&self, running: bool) -> InputStatsDto {
        let emitted = self.emitted.load(Ordering::Relaxed);
        let injected = self.injected.load(Ordering::Relaxed);
        let success_rate = if emitted == 0 {
            0.0
        } else {
            injected as f64 / emitted as f64
        };
        InputStatsDto {
            running,
            received: self.received.load(Ordering::Relaxed),
            emitted,
            injected,
            rejected: self.rejected.load(Ordering::Relaxed),
            merged: self.merged.load(Ordering::Relaxed),
            throttled: self.throttled.load(Ordering::Relaxed),
            pointer_events: self.pointer.load(Ordering::Relaxed),
            key_events: self.key.load(Ordering::Relaxed),
            wheel_events: self.wheel.load(Ordering::Relaxed),
            text_events: self.text.load(Ordering::Relaxed),
            keys_down: self.keys_down.load(Ordering::Relaxed),
            last_x: self.last_x.load(Ordering::Relaxed) as i32,
            last_y: self.last_y.load(Ordering::Relaxed) as i32,
            success_rate,
            last_error: self.last_error.lock().ok().and_then(|slot| slot.clone()),
        }
    }
}

/// 一次运行实例：注入器 + 合并器 + 清理状态。
struct Pipeline {
    injector: Box<dyn InputInjector>,
    coalescer: InputCoalescer,
    origin: Instant,
    counters: Arc<Counters>,
    /// 当前按下的扫描码，会话结束时逐个抬起，避免对端按键卡死。
    pressed_keys: BTreeSet<u16>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl Pipeline {
    /// 单调时钟（微秒），供合并器限频。
    fn now_us(&self) -> u64 {
        self.origin.elapsed().as_micros() as u64
    }

    /// 输出合并器给出的事件（0~2 条）并注入。
    fn flush_out(&mut self, emitted: rdc_input::Emitted) {
        let events: Vec<InputEvent> = emitted.iter().cloned().collect();
        for event in events {
            self.inject_one(event);
        }
    }

    /// 注入单条事件并更新计数。
    fn inject_one(&mut self, event: InputEvent) {
        self.counters.emitted.fetch_add(1, Ordering::Relaxed);
        match &event {
            InputEvent::Pointer(pointer) => {
                self.counters
                    .last_x
                    .store(pointer.x as i64, Ordering::Relaxed);
                self.counters
                    .last_y
                    .store(pointer.y as i64, Ordering::Relaxed);
            }
            InputEvent::Key(key) => {
                if key.pressed {
                    self.pressed_keys.insert(key.scan_code);
                } else {
                    self.pressed_keys.remove(&key.scan_code);
                }
                self.counters
                    .keys_down
                    .store(self.pressed_keys.len() as u64, Ordering::Relaxed);
            }
            _ => {}
        }
        match self.injector.inject(event) {
            Ok(()) => {
                self.counters.injected.fetch_add(1, Ordering::Relaxed);
            }
            Err(error) => {
                self.counters.rejected.fetch_add(1, Ordering::Relaxed);
                self.counters.set_error(error.to_string());
            }
        }
    }

    /// 把合并器的内部计数同步到快照计数器。
    fn sync_counters(&self) {
        self.counters
            .merged
            .store(self.coalescer.merged_moves(), Ordering::Relaxed);
        self.counters
            .throttled
            .store(self.coalescer.throttled_moves(), Ordering::Relaxed);
    }
}

// =====** 全局单实例 =====**

/// 当前运行实例槽位。
fn state() -> &'static Mutex<Option<Pipeline>> {
    static STATE: OnceLock<Mutex<Option<Pipeline>>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(None))
}

/// 最近一次会话的计数器（结束后仍可读，便于收尾核对）。
fn last_counters() -> &'static Mutex<Arc<Counters>> {
    static LAST: OnceLock<Mutex<Arc<Counters>>> = OnceLock::new();
    LAST.get_or_init(|| Mutex::new(Arc::new(Counters::default())))
}

/// 1 ms tick：冲刷挂起的移动，保证最后一个点在抬手后也能送达。
fn spawn_ticker(stop: Arc<AtomicBool>) -> JoinHandle<()> {
    thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            thread::sleep(TICK_INTERVAL);
            let Ok(mut guard) = state().lock() else {
                return;
            };
            let Some(pipeline) = guard.as_mut() else {
                return;
            };
            let now = pipeline.now_us();
            let out = pipeline.coalescer.flush(now);
            pipeline.flush_out(out);
            pipeline.sync_counters();
        }
    })
}

// =====** 对外入口 =====**

/// 启动输入通道（幂等：已在运行会先清理旧实例）。
///
/// # Arguments
/// * `desktop_width` - 被控端桌面物理宽
/// * `desktop_height` - 被控端桌面物理高
///
/// # Returns
/// 启动后的统计快照。
///
/// # Errors
/// 平台注入器创建失败时返回错误文本。
pub fn start(desktop_width: u32, desktop_height: u32) -> Result<InputStatsDto, String> {
    let width = desktop_width.max(1);
    let height = desktop_height.max(1);
    let viewport = Viewport {
        x: 0,
        y: 0,
        width,
        height,
        scale: 1.0,
    };
    // 先创建新注入器，失败则保留旧实例继续工作。
    let mut injector = create_injector().map_err(|error| format!("注入器创建失败: {error}"))?;
    injector.set_viewport(viewport);

    stop_internal();

    let counters = Arc::new(Counters::default());
    if let Ok(mut slot) = last_counters().lock() {
        *slot = counters.clone();
    }
    let stop = Arc::new(AtomicBool::new(false));
    let mut pipeline = Pipeline {
        injector,
        coalescer: InputCoalescer::new(DEFAULT_MAX_MOVES_PER_SEC),
        origin: Instant::now(),
        counters: counters.clone(),
        pressed_keys: BTreeSet::new(),
        stop: stop.clone(),
        join: None,
    };
    pipeline.join = Some(spawn_ticker(stop));
    *state()
        .lock()
        .map_err(|_| "输入状态锁异常".to_owned())? = Some(pipeline);
    Ok(counters.snapshot(true))
}

/// 推送一批输入事件（顺序即注入顺序）。
///
/// # Arguments
/// * `events` - Dart 侧构造的事件（坐标已完成映射）
///
/// # Returns
/// 处理后的统计快照。
///
/// # Errors
/// 通道未启动、事件类型未知或线格式往返失败时返回错误文本。
pub fn push(events: Vec<InputEventDto>) -> Result<InputStatsDto, String> {
    let mut guard = state().lock().map_err(|_| "输入状态锁异常".to_owned())?;
    let Some(pipeline) = guard.as_mut() else {
        return Err("输入通道未启动".to_owned());
    };
    for dto in events {
        let event = to_event(&dto)?;
        pipeline.counters.received.fetch_add(1, Ordering::Relaxed);
        match &event {
            InputEvent::Pointer(_) => {
                pipeline.counters.pointer.fetch_add(1, Ordering::Relaxed);
            }
            InputEvent::Key(_) => {
                pipeline.counters.key.fetch_add(1, Ordering::Relaxed);
            }
            InputEvent::Wheel(_) => {
                pipeline.counters.wheel.fetch_add(1, Ordering::Relaxed);
            }
            InputEvent::Text(_) => {
                pipeline.counters.text.fetch_add(1, Ordering::Relaxed);
            }
            InputEvent::Touch(_) => {}
        }
        let now = pipeline.now_us();
        let out = pipeline.coalescer.push(event, now);
        pipeline.flush_out(out);
    }
    pipeline.sync_counters();
    Ok(pipeline.counters.snapshot(true))
}

/// 冲刷挂起的移动（指针抬起、滚轮前后由 Dart 调用）。
///
/// # Returns
/// 处理后的统计快照。
///
/// # Errors
/// 通道未启动时返回错误文本。
pub fn flush() -> Result<InputStatsDto, String> {
    let mut guard = state().lock().map_err(|_| "输入状态锁异常".to_owned())?;
    let Some(pipeline) = guard.as_mut() else {
        return Err("输入通道未启动".to_owned());
    };
    let now = pipeline.now_us();
    let out = pipeline.coalescer.flush(now);
    pipeline.flush_out(out);
    pipeline.sync_counters();
    Ok(pipeline.counters.snapshot(true))
}

/// 停止输入通道：停 tick 线程、抬起所有键/钮、释放注入器。
///
/// # Returns
/// 收尾后的统计快照（`running=false`，`keys_down` 应为 0）。
///
/// # Errors
/// 不会失败；状态锁异常时返回空快照。
pub fn stop() -> Result<InputStatsDto, String> {
    stop_internal();
    Ok(last_snapshot())
}

/// 读取当前统计。
///
/// # Returns
/// 运行中返回实时快照；已停止返回最近一次会话的收尾快照。
///
/// # Errors
/// 状态锁异常时返回空快照。
pub fn status() -> Result<InputStatsDto, String> {
    if let Ok(guard) = state().lock() {
        if let Some(pipeline) = guard.as_ref() {
            return Ok(pipeline.counters.snapshot(true));
        }
    }
    Ok(last_snapshot())
}

// =====** 内部实现 =====**

/// 停止并清理当前实例（幂等）。
fn stop_internal() {
    let running = match state().lock() {
        Ok(mut guard) => guard.take(),
        Err(_) => None,
    };
    let Some(mut pipeline) = running else {
        return;
    };
    pipeline.stop.store(true, Ordering::Relaxed);
    if let Some(join) = pipeline.join.take() {
        let _ = join.join();
    }
    // 收尾：把仍按下的键与鼠标键全部抬起（即使对端以为还按着，也必须复位）。
    let x = pipeline.counters.last_x.load(Ordering::Relaxed) as i32;
    let y = pipeline.counters.last_y.load(Ordering::Relaxed) as i32;
    let pressed: Vec<u16> = pipeline.pressed_keys.iter().copied().collect();
    for scan_code in pressed {
        let _ = pipeline.injector.inject(InputEvent::Key(KeyEvent {
            scan_code,
            pressed: false,
            modifiers: 0,
        }));
    }
    for button in [
        PointerButton::Left,
        PointerButton::Right,
        PointerButton::Middle,
    ] {
        let _ = pipeline.injector.inject(InputEvent::Pointer(PointerEvent {
            x,
            y,
            button: Some(button),
            pressed: false,
        }));
    }
    pipeline
        .counters
        .keys_down
        .store(0, Ordering::Relaxed);
    // 注入器随 Pipeline 一起 drop，下压状态不再保留。
}

/// 最近一次会话的收尾快照。
fn last_snapshot() -> InputStatsDto {
    match last_counters().lock() {
        Ok(counters) => counters.snapshot(false),
        Err(_) => InputStatsDto::default(),
    }
}

/// DTO → 事件，并做一次 **S4 线格式往返**（encode → decode）。
///
/// 与网络通道使用同一编码器，保证本机回环验证的载荷就是 S4 真实发送的字节。
fn to_event(dto: &InputEventDto) -> Result<InputEvent, String> {
    let event = match dto.kind.as_str() {
        "move" => InputEvent::Pointer(PointerEvent {
            x: dto.x,
            y: dto.y,
            button: None,
            pressed: false,
        }),
        "button" => {
            let button = match dto.button {
                1 => PointerButton::Left,
                2 => PointerButton::Right,
                3 => PointerButton::Middle,
                other => return Err(format!("未知指针按键: {other}")),
            };
            InputEvent::Pointer(PointerEvent {
                x: dto.x,
                y: dto.y,
                button: Some(button),
                pressed: dto.pressed,
            })
        }
        "wheel" => InputEvent::Wheel(WheelEvent {
            delta_x: dto.delta_x,
            delta_y: dto.delta_y,
        }),
        "key" => InputEvent::Key(KeyEvent {
            scan_code: dto.scan_code,
            pressed: dto.pressed,
            modifiers: dto.modifiers,
        }),
        "text" => {
            // 上限在 DTO 边界检查：encode_event 不返回错误，超长只能在解码时暴露，
            // 这里提前拒绝可以给出可读错误，而不是“线格式解码失败”。
            if dto.text.len() > MAX_TEXT_EVENT_BYTES {
                return Err(format!(
                    "文本事件超长: {} 字节 > {}",
                    dto.text.len(),
                    MAX_TEXT_EVENT_BYTES
                ));
            }
            let event = TextEvent {
                delete_units: dto.delete_units,
                text: dto.text.clone(),
            };
            // 空编辑既不会改变远端文本，也会白白消耗一次 SendInput 调用。
            if event.is_noop() {
                return Err("文本事件为空（既不删除也不插入）".to_owned());
            }
            InputEvent::Text(event)
        }
        other => return Err(format!("未知输入事件类型: {other}")),
    };
    let payload = encode_event(&event);
    decode_event(&payload).map_err(|error| format!("线格式解码失败: {error}"))
}

// =====** 自检：只验证 DTO→事件与线格式往返，不做真实注入 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 指针移动：坐标与类型必须原样穿过线格式。
    #[test]
    fn move_roundtrip() {
        let dto = InputEventDto {
            kind: "move".to_owned(),
            x: 1234,
            y: 567,
            button: 0,
            pressed: false,
            delta_x: 0,
            delta_y: 0,
            scan_code: 0,
            modifiers: 0,
            delete_units: 0,
            text: String::new(),
        };
        match to_event(&dto).expect("往返") {
            InputEvent::Pointer(pointer) => {
                assert_eq!((pointer.x, pointer.y), (1234, 567));
                assert_eq!(pointer.button, None);
                assert!(!pointer.pressed);
            }
            other => panic!("应为指针事件: {other:?}"),
        }
    }

    /// 按下/抬起的三键编码必须与 S4 线格式一致（1/2/3）。
    #[test]
    fn button_codes_match_wire_format() {
        for (code, expected) in [
            (1u8, PointerButton::Left),
            (2, PointerButton::Right),
            (3, PointerButton::Middle),
        ] {
            let dto = InputEventDto {
                kind: "button".to_owned(),
                x: 100,
                y: 200,
                button: code,
                pressed: true,
                delta_x: 0,
                delta_y: 0,
                scan_code: 0,
                modifiers: 0,
                delete_units: 0,
                text: String::new(),
            };
            match to_event(&dto).expect("往返") {
                InputEvent::Pointer(pointer) => {
                    assert_eq!(pointer.button, Some(expected));
                    assert!(pointer.pressed);
                }
                other => panic!("应为指针事件: {other:?}"),
            }
        }
        let bad = InputEventDto {
            kind: "button".to_owned(),
            x: 0,
            y: 0,
            button: 9,
            pressed: true,
            delta_x: 0,
            delta_y: 0,
            scan_code: 0,
            modifiers: 0,
            delete_units: 0,
            text: String::new(),
        };
        assert!(to_event(&bad).is_err());
    }

    /// 键盘事件：扫描码、按下状态与修饰位都不得被线格式改写。
    #[test]
    fn key_roundtrip_keeps_scan_and_modifiers() {
        let dto = InputEventDto {
            kind: "key".to_owned(),
            x: 0,
            y: 0,
            button: 0,
            pressed: true,
            delta_x: 0,
            delta_y: 0,
            scan_code: 0xE048,
            modifiers: 0b0011,
            delete_units: 0,
            text: String::new(),
        };
        match to_event(&dto).expect("往返") {
            InputEvent::Key(key) => {
                assert_eq!(key.scan_code, 0xE048);
                assert!(key.pressed);
                assert_eq!(key.modifiers, 0b0011);
            }
            other => panic!("应为键盘事件: {other:?}"),
        }
    }

    /// 滚轮两个方向都必须保留符号。
    #[test]
    fn wheel_roundtrip_keeps_sign() {
        let dto = InputEventDto {
            kind: "wheel".to_owned(),
            x: 0,
            y: 0,
            button: 0,
            pressed: false,
            delta_x: -2,
            delta_y: 3,
            scan_code: 0,
            modifiers: 0,
            delete_units: 0,
            text: String::new(),
        };
        match to_event(&dto).expect("往返") {
            InputEvent::Wheel(wheel) => {
                assert_eq!(wheel.delta_x, -2);
                assert_eq!(wheel.delta_y, 3);
            }
            other => panic!("应为滚轮事件: {other:?}"),
        }
    }

    /// 文本事件：删除码元数与 UTF-8 正文原样穿过线格式；空编辑与超长正文拒绝。
    #[test]
    fn text_roundtrip_and_limits() {
        let dto = InputEventDto {
            kind: "text".to_owned(),
            x: 0,
            y: 0,
            button: 0,
            pressed: false,
            delta_x: 0,
            delta_y: 0,
            scan_code: 0,
            modifiers: 0,
            delete_units: 3,
            text: "输入法".to_owned(),
        };
        match to_event(&dto).expect("往返") {
            InputEvent::Text(text) => {
                assert_eq!(text.delete_units, 3);
                assert_eq!(text.text, "输入法");
            }
            other => panic!("应为文本事件: {other:?}"),
        }

        let mut oversize = dto.clone();
        oversize.text = "x".repeat(MAX_TEXT_EVENT_BYTES + 1);
        assert!(to_event(&oversize).is_err(), "超长正文必须拒绝");

        let mut noop = dto;
        noop.delete_units = 0;
        noop.text = String::new();
        assert!(to_event(&noop).is_err(), "空编辑必须拒绝");
    }

    /// 未知类型必须报错，不能被静默丢弃。
    #[test]
    fn unknown_kind_is_rejected() {
        let dto = InputEventDto {
            kind: "touch".to_owned(),
            x: 0,
            y: 0,
            button: 0,
            pressed: false,
            delta_x: 0,
            delta_y: 0,
            scan_code: 0,
            modifiers: 0,
            delete_units: 0,
            text: String::new(),
        };
        assert!(to_event(&dto).is_err());
    }
}
