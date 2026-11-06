//! rdc-transport / GCC 拥塞控制
//!
//! 层级：L2 能力层（`congestion` 子模块）
//! 用途：以延迟梯度为主、丢包为辅估计可用带宽，给出目标码率；能降能升且带滞回
//!       （docs/04 §4：丢包 >10% → ×0.7、>5% → ×0.85、每 5s 稳定后 +8%）。
//! 调用方：`congestion::create_controller` 构造，rdc-core 网络任务线程驱动
//!
//! 时间处理：trait 方法用 `Instant::now()`；内部另有带显式时刻的 `*_at` 变体，
//! 供单元测试注入虚拟时间，避免测试依赖真实等待。

use std::time::{Duration, Instant};

use super::CongestionController;

/// 最小目标码率：低于 1 Mbps 不再降，转降分辨率（docs/04 §4.2）。
const MIN_BPS: u32 = 1_000_000;

/// 每次稳定增长的比例（+8%）。
const INCREASE_RATIO: f64 = 1.08;

/// 回升前需要的稳定时长。
const STABLE_WAIT: Duration = Duration::from_secs(5);

/// 稳定期内需要的最少反馈批数（20ms 一批 → 5s 约 250 批，这里取 50 以容忍丢反馈）。
const MIN_ACK_BATCHES: u32 = 50;

/// 重丢包阈值：>10% 时 ×0.7。
const LOSS_HEAVY: f32 = 0.10;

/// 轻丢包阈值：>5% 时 ×0.85。
const LOSS_MILD: f32 = 0.05;

/// 排队延迟超过基线多少毫秒判定为 overuse。
const QUEUE_DELAY_LIMIT_MS: u32 = 15;

/// 两次 overuse 回退之间的冷却时间，防止连续踩踏。
const OVERUSE_COOLDOWN: Duration = Duration::from_millis(20);

// =====** GccController =====**

/// 基于 Google Congestion Control 思路的控制器。
pub struct GccController {
    /// 目标码率（bit/s）。
    target_bps: u32,
    /// 最低码率下限。
    min_bps: u32,
    /// 最高码率上限；实际部署由协商档位决定，这里不做软件上限。
    max_bps: u32,
    /// 平滑后的 RTT（毫秒）。
    smoothed_rtt_ms: u32,
    /// 观测到的最小 RTT，作为排队延迟的基线。
    baseline_rtt_ms: u32,
    /// 最近一次上调时刻；任何回退都会把它重置，形成滞回。
    last_increase: Instant,
    /// 稳定期内累计的 ACK 批数。
    ack_batches: u32,
    /// overuse 回退的冷却截止时刻。
    overuse_until: Instant,
}

impl GccController {
    /// 构造一个估计尚未启动的控制器。
    ///
    /// # Arguments
    /// * `initial_bps` - 初始目标码率（bit/s），低于 [`MIN_BPS`] 会被抬到下限
    ///
    /// # Returns
    /// 目标码率等于 `initial_bps`（已夹到合法区间）的实例。
    pub(crate) fn new(initial_bps: u32) -> Self {
        let now = Instant::now();
        Self {
            target_bps: initial_bps.max(MIN_BPS),
            min_bps: MIN_BPS,
            max_bps: u32::MAX,
            smoothed_rtt_ms: 0,
            baseline_rtt_ms: 0,
            last_increase: now,
            ack_batches: 0,
            overuse_until: now,
        }
    }

    /// 应用一次码率倍率，并重置稳定计数。
    fn apply_ratio(&mut self, ratio: f64, now: Instant) {
        let next = (f64::from(self.target_bps) * ratio).round();
        let clamped = (next as u64).clamp(u64::from(self.min_bps), u64::from(self.max_bps));
        self.target_bps = clamped as u32;
        self.last_increase = now;
        self.ack_batches = 0;
    }

    /// 带显式时刻的丢包处理（测试用）。
    fn on_loss_at(&mut self, loss_rate: f32, _burst_len: u32, now: Instant) {
        let rate = loss_rate.clamp(0.0, 1.0);
        if rate > LOSS_HEAVY {
            self.apply_ratio(0.7, now);
        } else if rate > LOSS_MILD {
            self.apply_ratio(0.85, now);
        }
    }

    /// 带显式时刻的 RTT 处理（测试用）。
    fn on_rtt_at(&mut self, rtt_ms: u32, now: Instant) {
        let sample = rtt_ms;
        if self.smoothed_rtt_ms == 0 {
            self.smoothed_rtt_ms = sample;
            self.baseline_rtt_ms = sample;
            return;
        }
        // 1/8 指数平滑，既能跟上排队增长，也不会被单次尖峰带飞。
        self.smoothed_rtt_ms = self.smoothed_rtt_ms * 7 / 8 + sample / 8;
        if sample < self.baseline_rtt_ms {
            self.baseline_rtt_ms = sample;
        }
        let queue_delay = self.smoothed_rtt_ms.saturating_sub(self.baseline_rtt_ms);
        if queue_delay > QUEUE_DELAY_LIMIT_MS && now >= self.overuse_until {
            self.apply_ratio(0.85, now);
            self.overuse_until = now + OVERUSE_COOLDOWN;
        }
    }

    /// 带显式时刻的 ACK 处理（测试用）。
    fn on_ack_at(&mut self, batch: &[u32], now: Instant) {
        if batch.is_empty() {
            return;
        }
        self.ack_batches = self.ack_batches.saturating_add(1);
        if now.saturating_duration_since(self.last_increase) < STABLE_WAIT {
            return;
        }
        if self.ack_batches < MIN_ACK_BATCHES {
            return;
        }
        self.ack_batches = 0;
        self.last_increase = now;
        let next = (f64::from(self.target_bps) * INCREASE_RATIO).round() as u64;
        self.target_bps = next.min(u64::from(self.max_bps)) as u32;
    }
}

impl CongestionController for GccController {
    /// 记录 ACK 批量，稳定 5s 后按 +8% 上调。
    fn on_ack(&mut self, batch: &[u32]) {
        self.on_ack_at(batch, Instant::now());
    }

    /// 按丢包率回退码率，立即生效。
    fn on_loss(&mut self, loss_rate: f32, burst_len: u32) {
        self.on_loss_at(loss_rate, burst_len, Instant::now());
    }

    /// 用 RTT 估计排队延迟，超阈值时回退。
    fn on_rtt(&mut self, rtt_ms: u32) {
        self.on_rtt_at(rtt_ms, Instant::now());
    }

    /// 返回当前目标码率。
    fn target_bitrate(&self) -> u32 {
        self.target_bps
    }

    /// 应用探测结果：交付码率高于目标时快速上调（单次最多 +20%）。
    fn on_probe_result(&mut self, delivered_bps: u32) {
        if delivered_bps == 0 || delivered_bps <= self.target_bps {
            return;
        }
        let ratio = (f64::from(delivered_bps) / f64::from(self.target_bps)).min(1.2);
        self.apply_ratio(ratio, Instant::now());
        self.last_increase = Instant::now() + STABLE_WAIT;
    }
}

// =====** 自检 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 10% 以上丢包按 ×0.7 立即回退。
    #[test]
    fn heavy_loss_backs_off_immediately() {
        let mut controller = GccController::new(10_000_000);
        let now = Instant::now();
        controller.on_loss_at(0.12, 3, now);
        assert_eq!(controller.target_bitrate(), 7_000_000);
    }

    /// 5%~10% 丢包按 ×0.85 回退。
    #[test]
    fn mild_loss_backs_off_slowly() {
        let mut controller = GccController::new(10_000_000);
        controller.on_loss_at(0.06, 1, Instant::now());
        assert_eq!(controller.target_bitrate(), 8_500_000);
    }

    /// 5% 以下丢包不触发回退（交 FEC/NACK 处理）。
    #[test]
    fn light_loss_keeps_rate() {
        let mut controller = GccController::new(10_000_000);
        controller.on_loss_at(0.02, 1, Instant::now());
        assert_eq!(controller.target_bitrate(), 10_000_000);
    }

    /// 码率不得低于 1 Mbps；任意大的丢包率都按 1.0 截断。
    #[test]
    fn rate_never_below_minimum() {
        let mut controller = GccController::new(1_200_000);
        let now = Instant::now();
        for _ in 0..10 {
            controller.on_loss_at(2.0, 9, now);
        }
        assert_eq!(controller.target_bitrate(), MIN_BPS);
    }

    /// 回升需要连续 5s 稳定 + 至少 50 批 ACK，缺一不可。
    #[test]
    fn increase_requires_stability_hysteresis() {
        let mut controller = GccController::new(10_000_000);
        let base = Instant::now();
        // 回调 60 批，但只过了 4.9s：不应上调。
        for _ in 0..60 {
            controller.on_ack_at(&[1], base + Duration::from_millis(4_900));
        }
        assert_eq!(controller.target_bitrate(), 10_000_000, "未到 5s 不应上调");

        // 过了 5s 再回调 50 批：按 +8% 上调。
        let later = base + Duration::from_secs(5) + Duration::from_millis(10);
        for _ in 0..50 {
            controller.on_ack_at(&[2], later);
        }
        assert_eq!(controller.target_bitrate(), 10_800_000);
    }

    /// 任何一次回退都会重置滞回计时，防止「刚降就升」。
    #[test]
    fn loss_resets_hysteresis() {
        let mut controller = GccController::new(10_000_000);
        let base = Instant::now();
        for _ in 0..60 {
            controller.on_ack_at(&[1], base + Duration::from_secs(6));
        }
        let after_increase = controller.target_bitrate();
        assert!(after_increase >= 10_800_000);

        controller.on_loss_at(0.12, 2, base + Duration::from_secs(7));
        let after_loss = controller.target_bitrate();
        assert!(after_loss < after_increase);

        // 紧接着满足 5s，但不足 50 批：不应继续上升。
        for _ in 0..10 {
            controller.on_ack_at(&[2], base + Duration::from_secs(12));
        }
        assert_eq!(controller.target_bitrate(), after_loss);
    }

    /// 排队延迟超过 15ms 触发 overuse 回退，且有冷却期。
    #[test]
    fn queue_delay_triggers_overuse_once() {
        let mut controller = GccController::new(10_000_000);
        let now = Instant::now();
        controller.on_rtt_at(10, now);
        // 连续样本让平滑 RTT 升到 10 + 15 以上。
        for _ in 0..8 {
            controller.on_rtt_at(80, now);
        }
        assert!(controller.target_bitrate() < 10_000_000, "应已回退");
        let after_first = controller.target_bitrate();

        // 冷却期内再来尖峰不再回退。
        controller.on_rtt_at(200, now + Duration::from_millis(5));
        assert_eq!(controller.target_bitrate(), after_first);
    }

    /// 探测结果只在交付码率更高时上调，且单次不超过 +20%。
    #[test]
    fn probe_result_is_capped() {
        let mut controller = GccController::new(10_000_000);
        controller.on_probe_result(3_000_000);
        assert_eq!(controller.target_bitrate(), 10_000_000, "低于目标不应上调");
        controller.on_probe_result(50_000_000);
        assert_eq!(controller.target_bitrate(), 12_000_000, "单次最多 +20%");
    }
}
