//! rdc-codec / 编码统计
//!
//! 层级：L3 编解码层
//! 用途：累计编码帧数/字节数，记录编码耗时与端到端编码时延，输出 p50/p99 与实测码率
//! 调用方：Windows Media Foundation 后端（编码线程内埋点）、rdc-cli（S3a 实测报告）

use std::sync::Mutex;

use rdc_runtime::SampleWindow;

// =====** 容量常量 =====**

/// 耗时样本窗口容量：10 秒 60fps 约 600 帧，留 4 倍余量。
const WINDOW: usize = 2400;

// =====** EncodeStats =====**

/// 编码统计计数器（编码线程独占写入，报告线程只读快照）。
#[derive(Debug)]
pub struct EncodeStats {
    /// 成功输出的帧数。
    encoded: u64,
    /// 关键帧数。
    keyframes: u64,
    /// 输出总字节数。
    bytes: u64,
    /// 单帧编码 API 耗时（微秒）。
    cost: Mutex<SampleWindow>,
    /// 单帧提交→取回码流的时延（微秒）。
    latency: Mutex<SampleWindow>,
}

impl EncodeStats {
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
            encoded: 0,
            keyframes: 0,
            bytes: 0,
            cost: Mutex::new(SampleWindow::new(WINDOW)),
            latency: Mutex::new(SampleWindow::new(WINDOW)),
        }
    }

    /// 记录一帧编码输出。
    ///
    /// # Arguments
    /// * `cost_us` - 编码 API 调用耗时（微秒）。
    /// * `latency_us` - 提交到取回该帧码流的时延（微秒）。
    /// * `bytes` - 该帧码流字节数。
    /// * `keyframe` - 是否关键帧。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    pub fn record(&mut self, cost_us: u64, latency_us: u64, bytes: usize, keyframe: bool) {
        self.encoded += 1;
        self.bytes += bytes as u64;
        if keyframe {
            self.keyframes += 1;
        }
        if let Ok(mut w) = self.cost.lock() {
            w.record(cost_us);
        }
        if let Ok(mut w) = self.latency.lock() {
            w.record(latency_us);
        }
    }

    /// 生成快照。
    ///
    /// # Arguments
    /// * `elapsed_ms` - 编码持续毫秒数。
    ///
    /// # Returns
    /// 含平均码率与分位数的快照。
    ///
    /// # Errors
    /// 不会失败。
    pub fn snapshot(&self, elapsed_ms: u64) -> EncodeSnapshot {
        let avg_bitrate_bps = if elapsed_ms > 0 {
            (self.bytes as f64 * 8.0 / (elapsed_ms as f64 / 1000.0)) as u64
        } else {
            0
        };
        let fps = if elapsed_ms > 0 {
            self.encoded as f64 / (elapsed_ms as f64 / 1000.0)
        } else {
            0.0
        };
        let (cost_p50, cost_p99, latency_p50, latency_p99) = {
            let cost = self.cost.lock();
            let latency = self.latency.lock();
            match (cost, latency) {
                (Ok(c), Ok(l)) => (c.p50(), c.p99(), l.p50(), l.p99()),
                _ => (0, 0, 0, 0),
            }
        };
        EncodeSnapshot {
            encoded: self.encoded,
            keyframes: self.keyframes,
            bytes: self.bytes,
            elapsed_ms,
            fps,
            avg_bitrate_bps,
            cost_p50_us: cost_p50,
            cost_p99_us: cost_p99,
            latency_p50_us: latency_p50,
            latency_p99_us: latency_p99,
        }
    }
}

impl Default for EncodeStats {
    /// 等价于 [`EncodeStats::new`]。
    fn default() -> Self {
        Self::new()
    }
}

// =====** EncodeSnapshot =====**

/// 编码统计快照。
#[derive(Debug, Clone, Copy, Default)]
pub struct EncodeSnapshot {
    /// 编码帧数。
    pub encoded: u64,
    /// 关键帧数。
    pub keyframes: u64,
    /// 输出总字节数。
    pub bytes: u64,
    /// 持续时长（毫秒）。
    pub elapsed_ms: u64,
    /// 实测编码帧率。
    pub fps: f64,
    /// 实测平均码率（bps）。
    pub avg_bitrate_bps: u64,
    /// 编码 API 耗时 p50（微秒）。
    pub cost_p50_us: u64,
    /// 编码 API 耗时 p99（微秒）。
    pub cost_p99_us: u64,
    /// 提交→输出时延 p50（微秒）。
    pub latency_p50_us: u64,
    /// 提交→输出时延 p99（微秒）。
    pub latency_p99_us: u64,
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 平均码率必须按「总字节数 × 8 / 秒」计算。
    #[test]
    fn average_bitrate_matches_bytes() {
        let mut stats = EncodeStats::new();
        // 1 秒 600 帧 × 1000 字节 = 4.8 Mbps
        for _ in 0..600 {
            stats.record(100, 200, 1000, false);
        }
        let snap = stats.snapshot(1000);
        assert_eq!(snap.encoded, 600);
        assert_eq!(snap.avg_bitrate_bps, 4_800_000);
        assert_eq!(snap.keyframes, 0);
        assert!((snap.fps - 600.0).abs() < 1e-9);
    }

    /// 空统计不得出现 NaN 或除零。
    #[test]
    fn empty_snapshot_is_zero() {
        let snap = EncodeStats::new().snapshot(0);
        assert_eq!(snap.encoded, 0);
        assert_eq!(snap.avg_bitrate_bps, 0);
        assert_eq!(snap.cost_p99_us, 0);
    }

    /// 关键帧计数必须只统计 keyframe=true 的样本。
    #[test]
    fn keyframe_counter() {
        let mut stats = EncodeStats::new();
        stats.record(100, 100, 10, true);
        stats.record(100, 100, 10, false);
        assert_eq!(stats.snapshot(1000).keyframes, 1);
    }
}
