//! rdc-runtime / 分位数采样窗口
//!
//! 层级：L2 能力层
//! 用途：记录耗时/帧间隔样本并给出 p50/p99/均值/最大值，供采集与编码统计使用
//! 调用方：rdc-capture 的采集统计、rdc-codec 的编码统计、rdc-cli 的实测报告

// =====** SampleWindow =====**

/// 固定上限的样本窗口（单位由调用方约定，通常是微秒）。
///
/// 为什么设上限：S3a 的 10 秒实测样本量有限，但长时间运行不能无限增长；
/// 超过 `capacity` 后按「隔一丢一」降采样，保留分布形状而不爆内存。
#[derive(Debug, Clone)]
pub struct SampleWindow {
    /// 已记录样本（保持插入顺序）。
    samples: Vec<u64>,
    /// 最大保留样本数。
    capacity: usize,
}

impl SampleWindow {
    /// 创建采样窗口。
    ///
    /// # Arguments
    /// * `capacity` - 最多保留多少个样本；为 0 时按 1 处理。
    ///
    /// # Returns
    /// 空窗口。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new(capacity: usize) -> Self {
        Self {
            samples: Vec::new(),
            capacity: capacity.max(1),
        }
    }

    /// 记录一个样本。
    ///
    /// # Arguments
    /// * `value` - 样本值（微秒或毫秒，调用方约定）。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    pub fn record(&mut self, value: u64) {
        if self.samples.len() >= self.capacity {
            // 降采样：保留偶数位，容量减半后继续接收新样本，避免无限增长。
            let mut keep = Vec::with_capacity(self.samples.len() / 2 + 1);
            for (idx, v) in self.samples.iter().enumerate() {
                if idx % 2 == 0 {
                    keep.push(*v);
                }
            }
            self.samples = keep;
        }
        self.samples.push(value);
    }

    /// 样本数量。
    ///
    /// # Returns
    /// 当前保留的样本数。
    ///
    /// # Errors
    /// 不会失败。
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// 是否没有样本。
    ///
    /// # Returns
    /// 样本为空返回 `true`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// 计算分位数（最近秩法）。
    ///
    /// # Arguments
    /// * `percent` - 百分位，取值 `0.0..=100.0`（如 50.0、99.0）。
    ///
    /// # Returns
    /// 对应分位数；无样本时返回 0。
    ///
    /// # Errors
    /// 不会失败。
    pub fn percentile(&self, percent: f64) -> u64 {
        if self.samples.is_empty() {
            return 0;
        }
        let mut sorted = self.samples.clone();
        sorted.sort_unstable();
        percentile_of_sorted(&sorted, percent)
    }

    /// 中位数（p50）。
    ///
    /// # Returns
    /// p50 分位数（微秒）。
    ///
    /// # Errors
    /// 不会失败。
    pub fn p50(&self) -> u64 {
        self.percentile(50.0)
    }

    /// p99 分位数。
    ///
    /// # Returns
    /// p99 分位数（微秒）。
    ///
    /// # Errors
    /// 不会失败。
    pub fn p99(&self) -> u64 {
        self.percentile(99.0)
    }

    /// 算术平均。
    ///
    /// # Returns
    /// 平均值；无样本时返回 0。
    ///
    /// # Errors
    /// 不会失败。
    pub fn mean(&self) -> u64 {
        if self.samples.is_empty() {
            return 0;
        }
        let sum: u128 = self.samples.iter().map(|v| u128::from(*v)).sum();
        (sum / self.samples.len() as u128) as u64
    }

    /// 最大值。
    ///
    /// # Returns
    /// 样本最大值；无样本时返回 0。
    ///
    /// # Errors
    /// 不会失败。
    pub fn max(&self) -> u64 {
        self.samples.iter().copied().max().unwrap_or(0)
    }

    /// 清空全部样本。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    pub fn reset(&mut self) {
        self.samples.clear();
    }
}

/// 对已排序切片取分位数（最近秩法，p=99 时取第 ceil(0.99*n) 个）。
fn percentile_of_sorted(sorted: &[u64], percent: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let p = percent.clamp(0.0, 100.0) / 100.0;
    // 最近秩：rank = ceil(p * n)，最小为 1，最大为 n。
    let rank = ((p * sorted.len() as f64).ceil() as usize).clamp(1, sorted.len());
    sorted[rank - 1]
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// p50/p99 按最近秩法计算，边界样本必须可预期。
    #[test]
    fn percentile_matches_nearest_rank() {
        let mut w = SampleWindow::new(1000);
        for v in 1..=100u64 {
            w.record(v);
        }
        assert_eq!(w.p50(), 50);
        assert_eq!(w.p99(), 99);
        assert_eq!(w.max(), 100);
        assert_eq!(w.mean(), 50);
    }

    /// 空窗口的统计值一律为 0，不能让调用方出现除以零。
    #[test]
    fn empty_window_reports_zero() {
        let w = SampleWindow::new(10);
        assert_eq!(w.p50(), 0);
        assert_eq!(w.p99(), 0);
        assert_eq!(w.mean(), 0);
        assert_eq!(w.max(), 0);
        assert!(w.is_empty());
    }

    /// 超过容量后降采样，样本数不会无限增长。
    #[test]
    fn capacity_is_bounded() {
        let mut w = SampleWindow::new(8);
        for v in 0..1000u64 {
            w.record(v);
        }
        assert!(w.len() <= 8, "样本数必须被容量约束，实际 {}", w.len());
        assert!(!w.is_empty());
    }
}
