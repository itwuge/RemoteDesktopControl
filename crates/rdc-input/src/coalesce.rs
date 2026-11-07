//! rdc-input / 高频移动合并与限频
//!
//! 层级：L3 输入层
//! 用途：把指针移动压到可接受速率（默认 200 次/秒），避免淹没 P0 输入队列；
//!       按键、滚轮、按下/抬起一律不合并、不丢、不乱序。
//! 调用方：rdc-core 的 `rdc-in` 线程（每个事件先过本合并器，再编码入队）
//!
//! 语义约定（与 docs/04 §2 pacer 规则一致）：
//! - **只合并移动**：纯移动（无按键）与拖拽移动（同一按键已按下）分别视为一类，
//!   同类连续移动只保留最新一点；换类别视为两次移动，旧点先出。
//! - **“按下”不是移动**：协议里按下与拖动中都是 `button=Some, pressed=true`，靠
//!   [`ButtonState`] 区分；第一次按下必须原样透传，否则对端永远收不到 DOWN。
//! - **按键优先于移动的时序**：挂起的移动在遇到任何非移动事件（键/滚轮/按下/抬起）时
//!   先输出，再输出该事件，保证“移动在前、按键在后”不被合并器颠倒。
//! - **限频只约束移动**：相邻两次移动至少间隔 `1_000_000 / max_moves_per_sec` 微秒；
//!   按键不受该间隔限制（用户按键不能为了帧率等 5ms）。
//! - 挂起的移动由调用方按 1ms tick 调 [`InputCoalescer::flush`] 冲刷，最迟延迟一个间隔。

use crate::event::{ButtonState, InputEvent, PointerButton};

/// 默认移动事件上限（次/秒），与 docs/04 §2「输入包上限 200 包/秒」一致。
pub const DEFAULT_MAX_MOVES_PER_SEC: u32 = 200;

// =====** Emitted =====**

/// 一次 `push`/`flush` 的结果：最多两个事件（挂起移动 + 当前事件）。
///
/// 用定长数组而不是 `Vec`：输入线程不分配，且容量上限在类型上固定为 2。
#[derive(Debug, Default)]
pub struct Emitted {
    /// 输出槽位，按顺序消费。
    events: [Option<InputEvent>; 2],
    /// 已占用的槽位数。
    len: usize,
}

impl Emitted {
    /// 输出的事件数（0~2）。
    pub fn len(&self) -> usize {
        self.len
    }

    /// 是否没有可输出事件。
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// 按输出顺序遍历事件。
    pub fn iter(&self) -> impl Iterator<Item = &InputEvent> {
        self.events.iter().flatten()
    }

    /// 追加一个事件；容量固定为 2，调用方保证不超。
    fn push(&mut self, event: InputEvent) {
        if self.len < self.events.len() {
            self.events[self.len] = Some(event);
            self.len += 1;
        }
    }
}

// =====** InputCoalescer =====**

/// 输入事件合并与限频器（单线程持有，无锁）。
pub struct InputCoalescer {
    /// 相邻移动事件的最小间隔（微秒）。
    min_interval_us: u64,
    /// 上一次移动事件输出的时刻（微秒）。
    last_move_emit_us: Option<u64>,
    /// 尚未输出的最新移动事件。
    pending: Option<InputEvent>,
    /// 指针按键下压状态，用于区分“按下”与“拖动中”。
    buttons: ButtonState,
    /// 被更新覆盖掉的移动数（合并计数）。
    merged: u64,
    /// 因限频被推迟的移动数。
    throttled: u64,
    /// 实际输出的事件总数。
    emitted: u64,
}

impl InputCoalescer {
    /// 按目标速率构造。
    ///
    /// # Arguments
    /// * `max_moves_per_sec` - 每秒允许输出的移动事件数；传 0 会被提升为 1
    ///
    /// # Returns
    /// 无挂起事件的实例。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new(max_moves_per_sec: u32) -> Self {
        let rate = max_moves_per_sec.max(1) as u64;
        Self {
            min_interval_us: 1_000_000 / rate,
            last_move_emit_us: None,
            pending: None,
            buttons: ButtonState::new(),
            merged: 0,
            throttled: 0,
            emitted: 0,
        }
    }

    /// 处理一个输入事件。
    ///
    /// # Arguments
    /// * `event` - 待发送事件（已完成坐标映射）
    /// * `now_us` - 当前单调时钟（微秒），由调用方统一时钟源提供
    ///
    /// # Returns
    /// 本次应输出的 0~2 个事件，按顺序输出即可保持时序。
    ///
    /// # Errors
    /// 不会失败。
    pub fn push(&mut self, event: InputEvent, now_us: u64) -> Emitted {
        let mut out = Emitted::default();
        match self.classify(&event) {
            None => {
                // 非移动事件：先冲刷挂起移动，再原样输出，保证“移动在前、按键在后”。
                if let Some(pending) = self.pending.take() {
                    self.emit(&mut out, pending, now_us, true);
                }
                self.apply_button_state(&event);
                self.emit(&mut out, event, now_us, false);
            }
            Some(class) => self.push_move(&mut out, event, class, now_us),
        }
        out
    }

    /// 按 tick 冲刷挂起的移动（间隔未到则继续挂起）。
    ///
    /// # Arguments
    /// * `now_us` - 当前单调时钟（微秒）
    ///
    /// # Returns
    /// 间隔已到且存在挂起移动时输出 1 个事件，否则为空。
    ///
    /// # Errors
    /// 不会失败。
    pub fn flush(&mut self, now_us: u64) -> Emitted {
        let mut out = Emitted::default();
        if self.interval_elapsed(now_us) {
            if let Some(pending) = self.pending.take() {
                self.emit(&mut out, pending, now_us, true);
            }
        }
        out
    }

    /// 会话结束时强制取出挂起移动，不等待限频间隔。
    ///
    /// # Returns
    /// 有挂起移动时输出 1 个事件，否则为空。
    ///
    /// # Errors
    /// 不会失败。
    pub fn finalize(&mut self) -> Emitted {
        let mut out = Emitted::default();
        if let Some(pending) = self.pending.take() {
            self.emit(&mut out, pending, 0, true);
        }
        out
    }

    /// 是否还有挂起未输出的移动。
    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// 被合并覆盖掉的移动事件数。
    pub fn merged_moves(&self) -> u64 {
        self.merged
    }

    /// 因限频被推迟过的移动事件数。
    pub fn throttled_moves(&self) -> u64 {
        self.throttled
    }

    /// 已输出的事件总数。
    pub fn emitted_events(&self) -> u64 {
        self.emitted
    }

    /// 处理一个已判定为移动的事件。
    fn push_move(
        &mut self,
        out: &mut Emitted,
        event: InputEvent,
        class: Option<PointerButton>,
        now_us: u64,
    ) {
        match self.pending.take() {
            None => {
                if self.interval_elapsed(now_us) {
                    self.emit(out, event, now_us, true);
                } else {
                    self.throttled += 1;
                    self.pending = Some(event);
                }
            }
            Some(pending) => {
                if self.class_of(&pending) == Some(class) {
                    // 同类移动：只保留最新一点，旧的直接丢弃（合并）。
                    self.merged += 1;
                } else {
                    // 换类别（例如拖拽开始/结束）：旧点必须先出，不能被新点覆盖。
                    self.emit(out, pending, now_us, true);
                }
                if self.interval_elapsed(now_us) {
                    self.emit(out, event, now_us, true);
                } else {
                    self.throttled += 1;
                    self.pending = Some(event);
                }
            }
        }
    }

    /// 记录输出并推进移动节拍。
    fn emit(&mut self, out: &mut Emitted, event: InputEvent, now_us: u64, is_move: bool) {
        // 只有移动才推进限频节拍：按键不能被 5ms 间隔挡住。
        if is_move {
            self.last_move_emit_us = Some(now_us);
        }
        self.emitted += 1;
        out.push(event);
    }

    /// 限频间隔是否已过。
    fn interval_elapsed(&self, now_us: u64) -> bool {
        match self.last_move_emit_us {
            None => true,
            Some(last) => now_us.saturating_sub(last) >= self.min_interval_us,
        }
    }

    /// 更新下压状态：只有带按键的指针事件才会改变状态。
    fn apply_button_state(&mut self, event: &InputEvent) {
        if let InputEvent::Pointer(p) = event {
            if let Some(button) = &p.button {
                if p.pressed {
                    self.buttons.press(button);
                } else {
                    self.buttons.release(button);
                }
            }
        }
    }

    /// 判定事件类别：`Some(None)` 纯移动、`Some(Some(键))` 该键拖动中、`None` 非移动。
    fn classify(&self, event: &InputEvent) -> Option<Option<PointerButton>> {
        let InputEvent::Pointer(p) = event else {
            return None;
        };
        match (&p.button, p.pressed) {
            (None, false) => Some(None),
            // 无按键却标记 pressed 是非法组合：原样透传，不做合并。
            (None, true) => None,
            // 已按下的同键再按下 = 拖动中的移动。
            (Some(button), true) if self.buttons.is_down(button) => Some(Some(button.clone())),
            // 首次按下 / 抬起：状态迁移，必须原样透传。
            _ => None,
        }
    }

    /// 取已判定事件的类别（用于与挂起事件比较）。
    fn class_of(&self, event: &InputEvent) -> Option<Option<PointerButton>> {
        self.classify(event)
    }
}

impl Default for InputCoalescer {
    /// 使用 [`DEFAULT_MAX_MOVES_PER_SEC`]。
    fn default() -> Self {
        Self::new(DEFAULT_MAX_MOVES_PER_SEC)
    }
}

// =====** 自检 =====**

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{KeyEvent, PointerEvent, WheelEvent};

    /// 纯移动事件。
    fn mv(x: i32, y: i32) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            x,
            y,
            button: None,
            pressed: false,
        })
    }

    /// 带按键的移动事件：`pressed=true` 时要靠下压状态区分按下与拖动。
    fn btn(x: i32, y: i32, button: PointerButton, pressed: bool) -> InputEvent {
        InputEvent::Pointer(PointerEvent {
            x,
            y,
            button: Some(button),
            pressed,
        })
    }

    /// 一次 push/flush 的输出收集成 Vec，便于断言顺序。
    fn collect(emitted: &Emitted) -> Vec<InputEvent> {
        emitted.iter().cloned().collect()
    }

    /// 间隔内连续移动只保留最新一点；到点 flush 输出最后一点。
    #[test]
    fn consecutive_moves_keep_latest() {
        let mut c = InputCoalescer::default(); // 200/s → 5ms
        assert_eq!(collect(&c.push(mv(1, 1), 1_000)), vec![mv(1, 1)]);
        // 1ms 后连续三次移动：全部被限频挂起，只留最后一点。
        assert!(c.push(mv(2, 2), 2_000).is_empty());
        assert!(c.push(mv(3, 3), 3_000).is_empty());
        assert!(c.push(mv(4, 4), 4_000).is_empty());
        assert_eq!(c.merged_moves(), 2, "只合并同类移动");
        assert!(c.has_pending());
        // 5ms 间隔未到，flush 不输出。
        assert!(c.flush(5_000).is_empty());
        // 6ms 时输出最新一点，而不是中间点。
        assert_eq!(collect(&c.flush(6_000)), vec![mv(4, 4)]);
        assert_eq!(c.throttled_moves(), 3);
    }

    /// 挂起的移动必须先于按键输出，按键一个都不能丢。
    #[test]
    fn keys_never_lose_or_reorder() {
        let mut c = InputCoalescer::default();
        assert_eq!(collect(&c.push(mv(1, 1), 0)), vec![mv(1, 1)]);
        // 2ms 内移动被限频挂起。
        assert!(c.push(mv(2, 2), 2_000).is_empty());
        // 紧接着按键：挂起移动先出，按键随后。
        let key = InputEvent::Key(KeyEvent {
            scan_code: 0x1E,
            pressed: true,
            modifiers: 0,
        });
        let out = c.push(key.clone(), 2_500);
        assert_eq!(collect(&out), vec![mv(2, 2), key]);
        // 连续 8 个按键（按下/抬起交替）全部按序输出，无合并无丢弃。
        let mut got = Vec::new();
        for i in 0..8u16 {
            let event = InputEvent::Key(KeyEvent {
                scan_code: 0x1E,
                pressed: i % 2 == 0,
                modifiers: 0,
            });
            got.extend(collect(&c.push(event.clone(), 3_000 + i as u64)));
            assert_eq!(got.last(), Some(&event));
        }
        assert_eq!(got.len(), 8);
        assert_eq!(c.emitted_events(), 11, "2 次移动 + 9 次按键");
    }

    /// 按下/抬起本身不参与移动合并，也不推进移动节拍。
    #[test]
    fn button_transitions_pass_through() {
        let mut c = InputCoalescer::default();
        let down = btn(10, 10, PointerButton::Left, true);
        let up = btn(10, 10, PointerButton::Left, false);
        assert_eq!(collect(&c.push(down.clone(), 0)), vec![down]);
        assert_eq!(collect(&c.push(up.clone(), 1_000)), vec![up]);
        // 按键不推进移动节拍：紧随其后的移动仍按上一次移动间隔判定。
        assert_eq!(collect(&c.push(mv(5, 5), 1_000)), vec![mv(5, 5)]);
        assert!(c.push(mv(6, 6), 2_000).is_empty());
    }

    /// 按下后同键的移动是拖动：可合并，且不得再当按下透传。
    #[test]
    fn drag_moves_coalesce_after_press() {
        let mut c = InputCoalescer::default();
        assert_eq!(collect(&c.push(mv(1, 1), 0)), vec![mv(1, 1)]);
        assert!(c.push(mv(2, 2), 1_000).is_empty());
        // 首次按下：非移动，先冲刷挂起移动，再输出按下。
        let down = btn(2, 2, PointerButton::Left, true);
        assert_eq!(
            collect(&c.push(down.clone(), 1_500)),
            vec![mv(2, 2), down],
            "按下不能被合并或延迟"
        );
        // 拖动移动：同类合并，只留最新一点。
        assert!(c
            .push(btn(3, 3, PointerButton::Left, true), 2_000)
            .is_empty());
        assert!(c
            .push(btn(4, 4, PointerButton::Left, true), 3_000)
            .is_empty());
        assert_eq!(c.merged_moves(), 1);
        assert_eq!(
            collect(&c.flush(6_500)),
            vec![btn(4, 4, PointerButton::Left, true)]
        );
        // 抬起：先冲刷可能挂起的拖动，再输出抬起（不会漏 UP）。
        assert!(c
            .push(btn(5, 5, PointerButton::Left, true), 7_000)
            .is_empty());
        let up = btn(5, 5, PointerButton::Left, false);
        assert_eq!(
            collect(&c.push(up.clone(), 7_100)),
            vec![btn(5, 5, PointerButton::Left, true), up]
        );
    }

    /// 纯移动与拖动是不同类别，切换时旧点必须先出。
    #[test]
    fn move_class_switch_flushes_old_point() {
        let mut c = InputCoalescer::new(1000); // 1ms 间隔，便于制造挂起
        assert_eq!(collect(&c.push(mv(1, 1), 0)), vec![mv(1, 1)]);
        let down = btn(1, 1, PointerButton::Left, true);
        assert_eq!(
            collect(&c.push(down, 2_000)),
            vec![btn(1, 1, PointerButton::Left, true)]
        );
        // 拖动点被限频挂起。
        assert_eq!(
            collect(&c.push(btn(3, 3, PointerButton::Left, true), 3_000)),
            vec![btn(3, 3, PointerButton::Left, true)]
        );
        assert!(c
            .push(btn(4, 4, PointerButton::Left, true), 3_100)
            .is_empty());
        // 切回纯移动：挂起的拖动点必须先出，纯移动点进入挂起。
        assert_eq!(
            collect(&c.push(mv(5, 5), 3_200)),
            vec![btn(4, 4, PointerButton::Left, true)],
            "类别切换时旧点不能丢"
        );
        assert!(c.has_pending());
        assert_eq!(c.merged_moves(), 0, "不同类别不计入合并");
        // 抬起时把挂起的纯移动和抬起一起冲出。
        let up = btn(5, 5, PointerButton::Left, false);
        assert_eq!(collect(&c.push(up.clone(), 3_300)), vec![mv(5, 5), up]);
    }

    /// 限频速率可配置，0 被提升为 1 次/秒。
    #[test]
    fn rate_is_configurable() {
        let mut c = InputCoalescer::new(1000); // 1ms 间隔
        assert_eq!(collect(&c.push(mv(1, 1), 0)), vec![mv(1, 1)]);
        assert_eq!(collect(&c.push(mv(2, 2), 1_000)), vec![mv(2, 2)]);

        let mut slow = InputCoalescer::new(0);
        assert_eq!(collect(&slow.push(mv(1, 1), 0)), vec![mv(1, 1)]);
        assert!(slow.push(mv(2, 2), 999_999).is_empty());
        assert_eq!(collect(&slow.flush(1_000_000)), vec![mv(2, 2)]);
    }

    /// 滚轮不参与合并，且能冲刷挂起的移动。
    #[test]
    fn wheel_flushes_pending_and_passes_through() {
        let mut c = InputCoalescer::default();
        assert_eq!(collect(&c.push(mv(1, 1), 0)), vec![mv(1, 1)]);
        assert!(c.push(mv(2, 2), 1_000).is_empty());
        let wheel = InputEvent::Wheel(WheelEvent {
            delta_x: 0,
            delta_y: 1,
        });
        assert_eq!(
            collect(&c.push(wheel.clone(), 1_200)),
            vec![mv(2, 2), wheel]
        );
    }

    /// 会话结束强制取出挂起移动，不丢最后一点。
    #[test]
    fn finalize_emits_pending() {
        let mut c = InputCoalescer::default();
        assert_eq!(collect(&c.push(mv(1, 1), 0)), vec![mv(1, 1)]);
        assert!(c.push(mv(2, 2), 1_000).is_empty());
        assert_eq!(collect(&c.finalize()), vec![mv(2, 2)]);
        assert!(!c.has_pending());
    }
}
