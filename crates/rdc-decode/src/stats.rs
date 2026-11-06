//! rdc-decode / 解码与呈现统计
//!
//! 层级：L3 编解码层
//! 用途：记录解码耗时、呈现延迟与队列深度的 p50/p99，供 S3b-2/S3b-4 实测报告使用。
//! 调用方：解码线程（record_decode）、呈现线程（record_present / observe_depth）、rdc-cli

use rdc_runtime::metrics::SampleWindow;

/// 统计窗口容量：60fps 下约 16 秒，足够覆盖一次十秒实测。
pub const WINDOW_CAPACITY: usize = 1024;

// =====** DecodeStats =====**

/// 解码/呈现统计器。
pub struct DecodeStats {
    /// 单帧解码耗时（微秒）。
    decode_us: SampleWindow,
    /// 单帧呈现延迟（微秒）：从解码完成到提交到交换链。
    present_us: SampleWindow,
    /// 观测到的最大队列深度。
    max_depth: usize,
    /// 累计解码帧数。
    decoded_frames: u64,
    /// 累计呈现帧数。
    presented_frames: u64,
}

impl DecodeStats {
    /// 构造统计器。
    pub fn new() -> Self {
        Self {
            decode_us: SampleWindow::new(WINDOW_CAPACITY),
            present_us: SampleWindow::new(WINDOW_CAPACITY),
            max_depth: 0,
            decoded_frames: 0,
            presented_frames: 0,
        }
    }

    /// 记录一帧解码耗时。
    ///
    /// # Arguments
    /// * `cost_us` - 本次解码耗时（微秒）
    pub fn record_decode(&mut self, cost_us: u64) {
        self.decode_us.record(cost_us);
        self.decoded_frames += 1;
    }

    /// 记录一帧呈现延迟。
    ///
    /// # Arguments
    /// * `latency_us` - 从解码完成到提交呈现的延迟（微秒）
    pub fn record_present(&mut self, latency_us: u64) {
        self.present_us.record(latency_us);
        self.presented_frames += 1;
    }

    /// 观测队列深度，只保留最大值。
    ///
    /// # Arguments
    /// * `depth` - 当前队列深度（帧）
    pub fn observe_depth(&mut self, depth: usize) {
        self.max_depth = self.max_depth.max(depth);
    }

    /// 生成快照，供报告直接取用。
    pub fn snapshot(&self) -> DecodeSnapshot {
        DecodeSnapshot {
            decoded_frames: self.decoded_frames,
            presented_frames: self.presented_frames,
            decode_p50_us: self.decode_us.p50(),
            decode_p99_us: self.decode_us.p99(),
            present_p50_us: self.present_us.p50(),
            present_p99_us: self.present_us.p99(),
            max_depth: self.max_depth,
        }
    }
}

impl Default for DecodeStats {
    fn default() -> Self {
        Self::new()
    }
}

// =====** DecodeSnapshot =====**

/// 统计快照（不可变）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodeSnapshot {
    /// 累计解码帧数。
    pub decoded_frames: u64,
    /// 累计呈现帧数。
    pub presented_frames: u64,
    /// 解码耗时 p50（微秒）。
    pub decode_p50_us: u64,
    /// 解码耗时 p99（微秒）。
    pub decode_p99_us: u64,
    /// 呈现延迟 p50（微秒）。
    pub present_p50_us: u64,
    /// 呈现延迟 p99（微秒）。
    pub present_p99_us: u64,
    /// 最大队列深度。
    pub max_depth: usize,
}

// =====** 自检 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 计数与分位数必须能对上单调样本。
    #[test]
    fn snapshot_reports_counts_and_percentiles() {
        let mut stats = DecodeStats::new();
        for value in 1..=100u64 {
            stats.record_decode(value);
            stats.record_present(value * 2);
        }
        let snapshot = stats.snapshot();
        assert_eq!(snapshot.decoded_frames, 100);
        assert_eq!(snapshot.presented_frames, 100);
        // 最近秩法：p50 落在第 50/100，p99 落在第 99/100。
        assert_eq!(snapshot.decode_p50_us, 50);
        assert_eq!(snapshot.decode_p99_us, 99);
        assert_eq!(snapshot.present_p50_us, 100);
        assert_eq!(snapshot.present_p99_us, 198);
    }

    /// 队列深度只记录历史最大值，不回退。
    #[test]
    fn max_depth_is_monotonic() {
        let mut stats = DecodeStats::new();
        stats.observe_depth(1);
        stats.observe_depth(4);
        stats.observe_depth(0);
        assert_eq!(stats.snapshot().max_depth, 4);
    }

    /// 空统计的快照不应 panic，分位数为 0。
    #[test]
    fn empty_snapshot_is_zero() {
        let snapshot = DecodeStats::new().snapshot();
        assert_eq!(snapshot.decoded_frames, 0);
        assert_eq!(snapshot.decode_p99_us, 0);
        assert_eq!(snapshot.max_depth, 0);
    }
}
