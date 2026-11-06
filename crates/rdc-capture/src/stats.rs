//! rdc-capture / 采集统计
//!
//! 层级：L3 采集层
//! 用途：累计采集帧数/丢帧数/超时数，并记录帧间隔与采集耗时，输出 p50/p99
//! 调用方：rdc-capture 平台后端（采集线程内埋点）、rdc-cli（S3a 实测报告）

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use rdc_runtime::SampleWindow;

// =====** 容量常量 =====**

/// 帧间隔样本窗口容量：10 秒 60fps 约 600 帧，留 4 倍余量。
const INTERVAL_WINDOW: usize = 2400;

/// 采集耗时样本窗口容量。
const COST_WINDOW: usize = 2400;

// =====** CaptureStats =====**

/// 采集统计计数器。
///
/// 线程模型：采集线程写、报告线程读；计数用原子量，分布样本用 `Mutex` 保护
/// （只在采集线程追加，竞争可忽略）。
#[derive(Debug)]
pub struct CaptureStats {
    /// 实际交付给 sink 的帧数。
    captured: AtomicU64,
    /// 因池空或 sink 拒绝而丢弃的帧数。
    dropped: AtomicU64,
    /// `AcquireNextFrame` 超时次数（桌面无变化，不算丢帧）。
    timeouts: AtomicU64,
    /// 池耗尽次数。
    pool_exhausted: AtomicU64,
    /// 帧间隔样本（微秒）。
    intervals: Mutex<SampleWindow>,
    /// 采集耗时样本（微秒）。
    costs: Mutex<SampleWindow>,
    /// 会话起始 QPC 微秒。
    start_qpc_us: AtomicU64,
    /// 最近一帧 QPC 微秒。
    last_qpc_us: AtomicU64,
}

impl CaptureStats {
    /// 创建统计器。
    ///
    /// # Arguments
    /// * 无。
    ///
    /// # Returns
    /// 全零统计器。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new() -> Self {
        Self {
            captured: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            timeouts: AtomicU64::new(0),
            pool_exhausted: AtomicU64::new(0),
            intervals: Mutex::new(SampleWindow::new(INTERVAL_WINDOW)),
            costs: Mutex::new(SampleWindow::new(COST_WINDOW)),
            start_qpc_us: AtomicU64::new(0),
            last_qpc_us: AtomicU64::new(0),
        }
    }

    /// 记录会话开始时间。
    ///
    /// # Arguments
    /// * `qpc_us` - 会话起始 QPC 微秒。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    pub fn mark_start(&self, qpc_us: u64) {
        self.start_qpc_us.store(qpc_us, Ordering::Relaxed);
        self.last_qpc_us.store(qpc_us, Ordering::Relaxed);
    }

    /// 记录一帧成功采集。
    ///
    /// # Arguments
    /// * `qpc_us` - 本帧 QPC 微秒。
    /// * `cost_us` - 采集耗时（微秒）。
    ///
    /// # Returns
    /// 距上一帧的间隔（微秒）；首帧返回 0。
    ///
    /// # Errors
    /// 不会失败。
    pub fn record_frame(&self, qpc_us: u64, cost_us: u64) -> u64 {
        let prev = self.last_qpc_us.swap(qpc_us, Ordering::Relaxed);
        let interval = qpc_us.saturating_sub(prev);
        // 首帧的 prev 等于会话起点，间隔即首帧等待时间；为语义清晰，首帧不记入分布。
        if self.captured.load(Ordering::Relaxed) > 0 && interval > 0 {
            if let Ok(mut w) = self.intervals.lock() {
                w.record(interval);
            }
        }
        if let Ok(mut w) = self.costs.lock() {
            w.record(cost_us);
        }
        self.captured.fetch_add(1, Ordering::Relaxed);
        interval
    }

    /// 累加丢帧。
    ///
    /// # Arguments
    /// * `delta` - 新增丢帧数。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    pub fn add_dropped(&self, delta: u64) {
        self.dropped.fetch_add(delta, Ordering::Relaxed);
    }

    /// 累加超时次数。
    ///
    /// # Arguments
    /// * `无`。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    pub fn add_timeout(&self) {
        self.timeouts.fetch_add(1, Ordering::Relaxed);
    }

    /// 累加池耗尽次数。
    ///
    /// # Arguments
    /// * 无。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    pub fn add_pool_exhausted(&self) {
        self.pool_exhausted.fetch_add(1, Ordering::Relaxed);
    }

    /// 生成统计快照。
    ///
    /// # Arguments
    /// * `elapsed_ms` - 会话已运行毫秒数（由调用方计时）。
    ///
    /// # Returns
    /// 包含丢帧率与 p50/p99 的快照。
    ///
    /// # Errors
    /// 不会失败。
    pub fn snapshot(&self, elapsed_ms: u64) -> CaptureSnapshot {
        let captured = self.captured.load(Ordering::Relaxed);
        let dropped = self.dropped.load(Ordering::Relaxed);
        let timeouts = self.timeouts.load(Ordering::Relaxed);
        let pool_exhausted = self.pool_exhausted.load(Ordering::Relaxed);
        let total = captured + dropped;
        // 丢帧率以「应交付帧」为分母：既包含被丢弃的帧，也包含实际交付的帧。
        let drop_rate = if total > 0 {
            dropped as f64 / total as f64
        } else {
            0.0
        };
        let fps = if elapsed_ms > 0 {
            captured as f64 / (elapsed_ms as f64 / 1000.0)
        } else {
            0.0
        };
        let (interval_p50, interval_p99, cost_p50, cost_p99) = {
            let intervals = self.intervals.lock();
            let costs = self.costs.lock();
            match (intervals, costs) {
                (Ok(i), Ok(c)) => (i.p50(), i.p99(), c.p50(), c.p99()),
                _ => (0, 0, 0, 0),
            }
        };
        CaptureSnapshot {
            captured,
            dropped,
            timeouts,
            pool_exhausted,
            elapsed_ms,
            fps,
            drop_rate,
            interval_p50_us: interval_p50,
            interval_p99_us: interval_p99,
            cost_p50_us: cost_p50,
            cost_p99_us: cost_p99,
        }
    }
}

impl Default for CaptureStats {
    /// 等价于 [`CaptureStats::new`]。
    fn default() -> Self {
        Self::new()
    }
}

// =====** CaptureSnapshot =====**

/// 采集统计快照（供报告与 HUD 使用）。
#[derive(Debug, Clone, Copy, Default)]
pub struct CaptureSnapshot {
    /// 实际交付帧数。
    pub captured: u64,
    /// 丢帧数。
    pub dropped: u64,
    /// 采集超时次数（无变化）。
    pub timeouts: u64,
    /// 池耗尽次数。
    pub pool_exhausted: u64,
    /// 会话时长（毫秒）。
    pub elapsed_ms: u64,
    /// 实测帧率。
    pub fps: f64,
    /// 丢帧率（0.0~1.0）。
    pub drop_rate: f64,
    /// 帧间隔 p50（微秒）。
    pub interval_p50_us: u64,
    /// 帧间隔 p99（微秒）。
    pub interval_p99_us: u64,
    /// 采集耗时 p50（微秒）。
    pub cost_p50_us: u64,
    /// 采集耗时 p99（微秒）。
    pub cost_p99_us: u64,
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 成功帧计数与丢帧率必须按「交付 + 丢弃」计算。
    #[test]
    fn snapshot_counts_frames() {
        let stats = CaptureStats::new();
        stats.mark_start(0);
        stats.record_frame(16_000, 500);
        stats.record_frame(33_000, 600);
        stats.add_dropped(1);
        let snap = stats.snapshot(1000);
        assert_eq!(snap.captured, 2);
        assert_eq!(snap.dropped, 1);
        assert!((snap.drop_rate - 1.0 / 3.0).abs() < 1e-9);
        // 最近秩法：2 个样本时 p50 命中排序后的第 1 个（升序首个），而不是插值。
        assert_eq!(snap.cost_p50_us, 500);
    }

    /// 空统计器不得出现 NaN 或除零。
    #[test]
    fn empty_snapshot_is_zero() {
        let snap = CaptureStats::new().snapshot(0);
        assert_eq!(snap.captured, 0);
        assert_eq!(snap.drop_rate, 0.0);
        assert_eq!(snap.fps, 0.0);
        assert_eq!(snap.interval_p99_us, 0);
    }
}
