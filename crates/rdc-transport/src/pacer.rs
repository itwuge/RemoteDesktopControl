//! rdc-transport / 发送节流
//!
//! 层级：L2 能力层
//! 用途：令牌桶式发送节流，把拥塞控制器给出的目标码率换算成「现在能不能发、还要等多久」，
//!       平滑出包避免整秒突刺（docs/04 §2 Pacer 规则）。
//! 调用方：rdc-core 的 `rdc-tx` 任务（每 1ms tick 查询一次）

use std::time::{Duration, Instant};

use crate::error::TransportError;

// =====** Pacer =====**

/// 令牌桶式发送节流器。
///
/// 桶容量 = `burst_bytes`，按 `target_bps` 匀速补充。桶内令牌以「字节」计价，
/// 因此 `on_sent` 传入实际发出的字节数即可（含包头与 tag）。
pub struct Pacer {
    /// 目标码率（bit/s），由拥塞控制器更新。
    target_bps: u32,
    /// 桶容量（字节）。
    capacity: f64,
    /// 当前可用令牌（字节）。
    tokens: f64,
    /// 上次结算时刻，用于按经过时间补充令牌。
    last: Instant,
}

impl Pacer {
    /// 构造节流器。
    ///
    /// # Arguments
    /// * `target_bps` - 初始目标码率（bit/s），必须大于 0
    /// * `burst_bytes` - 允许的瞬时突发字节数，生产配置取约 2ms 的码率
    ///
    /// # Returns
    /// 满桶的节流器实例（允许启动瞬间按突发量发一簇）。
    ///
    /// # Errors
    /// `target_bps` 为 0 时返回 [`TransportError::InvalidRate`]：
    /// 0 码率会让发送永远不被调度，属于调用方配置错误。
    pub fn new(target_bps: u32, burst_bytes: u32) -> Result<Self, TransportError> {
        if target_bps == 0 {
            return Err(TransportError::InvalidRate);
        }
        let capacity = f64::from(burst_bytes.max(1));
        Ok(Self {
            target_bps,
            capacity,
            tokens: capacity,
            last: Instant::now(),
        })
    }

    /// 更新目标码率。
    ///
    /// # Arguments
    /// * `target_bps` - 新目标码率（bit/s），必须大于 0
    ///
    /// # Returns
    /// 更新成功时返回 `()`。
    ///
    /// # Errors
    /// `target_bps` 为 0 时返回 [`TransportError::InvalidRate`]，且保持旧码率不变。
    pub fn set_target_bitrate(&mut self, target_bps: u32) -> Result<(), TransportError> {
        if target_bps == 0 {
            return Err(TransportError::InvalidRate);
        }
        self.refill(Instant::now());
        self.target_bps = target_bps;
        Ok(())
    }

    /// 计算距下一个分片可发送还需等待的时间。
    ///
    /// # Arguments
    /// * `now` - 调用时刻，用单调时钟避免受系统时间调整影响
    ///
    /// # Returns
    /// 需要等待的时长；`ZERO` 表示桶内至少还有 1 字节额度，可立即发送。
    pub fn poll_delay(&self, now: Instant) -> Duration {
        self.poll_delay_for(1, now)
    }

    /// 计算发出 `bytes` 字节还需等待的时间。
    ///
    /// # Arguments
    /// * `bytes` - 待发字节数（包头 + 载荷 + tag）
    /// * `now` - 调用时刻
    ///
    /// # Returns
    /// 需要等待的时长；额度足够时返回 `ZERO`。
    pub fn poll_delay_for(&self, bytes: usize, now: Instant) -> Duration {
        let tokens = self.tokens_at(now);
        let deficit = bytes as f64 - tokens;
        if deficit <= 0.0 {
            return Duration::ZERO;
        }
        let rate = self.bytes_per_second();
        let seconds = deficit / rate;
        // 上界保护：rate 极小时避免转换出 NaN / 溢出。
        Duration::from_secs_f64(seconds.min(1.0))
    }

    /// 记录一次实际发送，扣除令牌。
    ///
    /// # Arguments
    /// * `bytes` - 本次发送的总字节数（包头 + 载荷 + tag）
    /// * `now` - 发送时刻
    ///
    /// # Returns
    /// 无返回值；令牌不足时按负值记账，等价于下次调度自动多等。
    pub fn on_sent(&mut self, bytes: usize, now: Instant) {
        self.refill(now);
        self.tokens -= bytes as f64;
        // 允许短时欠账，但不允许无限透支，否则一次大帧会把后续几十毫秒都吃掉。
        if self.tokens < -self.capacity {
            self.tokens = -self.capacity;
        }
    }

    /// 返回当前目标码率。
    pub fn target_bitrate(&self) -> u32 {
        self.target_bps
    }

    /// 按经过时间补充令牌并推进结算时刻。
    fn refill(&mut self, now: Instant) {
        self.tokens = self.tokens_at(now).min(self.capacity);
        self.last = now;
    }

    /// 计算 `now` 时刻的令牌数（不推进状态），供 `poll_delay` 复用。
    fn tokens_at(&self, now: Instant) -> f64 {
        let elapsed = now.saturating_duration_since(self.last);
        (self.tokens + elapsed.as_secs_f64() * self.bytes_per_second()).min(self.capacity)
    }

    /// 每秒可发字节数。
    fn bytes_per_second(&self) -> f64 {
        f64::from(self.target_bps) / 8.0
    }
}

// =====** 自检 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 新建的桶是满的：启动瞬间允许一簇突发。
    #[test]
    fn initial_burst_is_immediate() {
        let pacer = Pacer::new(8_000_000, 2048).expect("合法码率");
        assert_eq!(pacer.poll_delay(Instant::now()), Duration::ZERO);
    }

    /// 花光突发后必须等待，且等待时间与码率成反比。
    #[test]
    fn empty_bucket_waits_proportionally() {
        let mut pacer = Pacer::new(8_000_000, 2048).expect("合法码率");
        let start = Instant::now();
        // 8 Mbps = 1 MB/s；2048 字节恰好 2ms 的额度。
        pacer.on_sent(2048, start);
        let delay = pacer.poll_delay_for(2048, start);
        assert!(delay >= Duration::from_millis(2), "应至少等 2ms，实际 {delay:?}");
        assert!(delay <= Duration::from_millis(3), "等待不应超出额度太多，实际 {delay:?}");
    }

    /// 码率翻倍后同样欠账的等待时间应减半。
    #[test]
    fn higher_bitrate_halves_delay() {
        let base = Instant::now();
        let slow = {
            let mut p = Pacer::new(8_000_000, 4096).expect("合法码率");
            p.on_sent(4096, base);
            p.poll_delay_for(4096, base)
        };
        let fast = {
            let mut p = Pacer::new(16_000_000, 4096).expect("合法码率");
            p.on_sent(4096, base);
            p.poll_delay_for(4096, base)
        };
        let ratio = slow.as_secs_f64() / fast.as_secs_f64();
        assert!((ratio - 2.0).abs() < 0.1, "比例应接近 2，实际 {ratio}");
    }

    /// 等待足够时间后令牌补回，可再次发送。
    #[test]
    fn tokens_refill_over_time() {
        let mut pacer = Pacer::new(8_000_000, 2048).expect("合法码率");
        let start = Instant::now();
        pacer.on_sent(2048, start);
        let later = start + Duration::from_millis(10);
        assert!(
            pacer.poll_delay_for(2048, later).is_zero(),
            "10ms 应补回约 10KB 额度"
        );
    }

    /// 0 码率必须被拒绝，且失败时保持原码率。
    #[test]
    fn zero_bitrate_is_rejected() {
        let mut pacer = Pacer::new(8_000_000, 2048).expect("合法码率");
        assert!(pacer.set_target_bitrate(0).is_err());
        assert_eq!(pacer.target_bitrate(), 8_000_000);
        assert!(Pacer::new(0, 2048).is_err());
    }

    /// 突发容量为 0 时提升为 1，保证仍能发出单个字节。
    #[test]
    fn zero_burst_becomes_one() {
        let pacer = Pacer::new(1_000_000, 0).expect("合法码率");
        assert_eq!(pacer.poll_delay(Instant::now()), Duration::ZERO);
    }
}
