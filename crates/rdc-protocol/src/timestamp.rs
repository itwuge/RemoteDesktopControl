//! rdc-protocol / 时间码与端到端延迟度量
//!
//! 层级：L2 能力层
//! 用途：定义帧级时间码字段、CSV 列序、抖动缓冲与 deadline 计算规则；
//!       所有延迟统计必须基于这些字段，禁止各模块自造指标（docs/04 §5、docs/10 §7）。
//! 调用方：rdc-capture（写 capture_ts）、rdc-codec（写 encode/decode 耗时）、
//!         rdc-transport（写 send/recv）、rdc-core（统计与 CSV 导出）

use std::collections::VecDeque;

// =====** 抖动缓冲常量 =====**

/// 抖动缓冲下限（帧）。
pub const JITTER_DELAY_MIN_FRAMES: u8 = crate::header::JITTER_BUFFER_MIN_FRAMES;

/// 抖动缓冲上限（帧）。
pub const JITTER_DELAY_MAX_FRAMES: u8 = crate::header::JITTER_BUFFER_MAX_FRAMES;

/// 按 60 fps 估算的每帧时长（微秒），用于把“帧”换算成微秒。
pub const FRAME_DURATION_US_60FPS: u64 = 1_000_000 / 60;

// =====** FrameTiming =====**

/// 单帧时间码。
///
/// 全部时间戳均为**同一主机单调时钟的微秒值**；跨主机比较前必须先减去
/// `offset`（见 [`deadline_us`]），否则墙钟漂移会污染 e2e 统计。
#[derive(Debug, Clone, PartialEq)]
pub struct FrameTiming {
    /// 帧号（u16，按帧递增）。
    pub frame_id: u16,
    /// 采集时刻（微秒）。
    pub capture_ts: u64,
    /// 编码耗时（毫秒，浮点保留小数）。
    pub encode_ms: f32,
    /// 发送时刻（微秒）。
    pub send_ts: u64,
    /// 接收时刻（微秒）。
    pub recv_ts: u64,
    /// 解码耗时（毫秒）。
    pub decode_ms: f32,
    /// 呈现时刻（微秒）。
    pub present_ts: u64,
}

impl FrameTiming {
    /// 端到端延迟（毫秒）：从采集到呈现。
    ///
    /// 用 `saturating_sub` 而不是直接减：时钟回拨或乱序样本不得产生负延迟，
    /// 负值会让 p50/p95 统计失去意义。
    ///
    /// # Returns
    /// 非负毫秒值。
    ///
    /// # Errors
    /// 不会失败。
    pub fn e2e_ms(&self) -> f32 {
        self.present_ts.saturating_sub(self.capture_ts) as f32 / 1000.0
    }

    /// 输出为 CSV 行（列序与 [`TIMING_CSV_HEADER`] 一致）。
    ///
    /// # Returns
    /// 不含换行的 CSV 记录。
    ///
    /// # Errors
    /// 不会失败。
    pub fn to_csv_row(&self) -> String {
        format!(
            "{},{},{:.3},{},{},{:.3},{},{:.3}",
            self.frame_id,
            self.capture_ts,
            self.encode_ms,
            self.send_ts,
            self.recv_ts,
            self.decode_ms,
            self.present_ts,
            self.e2e_ms()
        )
    }
}

/// CSV 表头：列序是外部工具（Excel/脚本）的契约，禁止随意调整。
pub const TIMING_CSV_HEADER: &str =
    "frame_id,capture_ts,encode_ms,send_ts,recv_ts,decode_ms,present_ts,e2e_ms";

// =====** deadline 计算 =====**

/// 计算帧的呈现截止时刻：`deadline = ts_us + offset + jitter_delay`。
///
/// # Arguments
/// * `ts_us` - 采集时间戳（微秒，发送端单调时钟）
/// * `offset_us` - 收发两端单调时钟偏移（带符号，用 `i64` 表达）
/// * `jitter_delay_us` - 抖动缓冲引入的额外延迟（微秒）
///
/// # Returns
/// 接收端时钟下的截止时刻（微秒）；负值饱和为 0，避免 u64 回绕成巨大值。
///
/// # Errors
/// 不会失败。
pub fn deadline_us(ts_us: u64, offset_us: i64, jitter_delay_us: u64) -> u64 {
    // 先转 i128 计算，避免 i64 + u64 溢出；最后饱和回 u64。
    let value = ts_us as i128 + offset_us as i128 + jitter_delay_us as i128;
    value.clamp(0, u64::MAX as i128) as u64
}

/// 把抖动缓冲帧数换算为微秒（按 60 fps 估算）。
///
/// # Arguments
/// * `frames` - 1~3 帧；超出范围会被夹到合法区间
///
/// # Returns
/// 抖动延迟（微秒）。
///
/// # Errors
/// 不会失败。
pub fn jitter_delay_us(frames: u8) -> u64 {
    let clamped = frames.clamp(JITTER_DELAY_MIN_FRAMES, JITTER_DELAY_MAX_FRAMES);
    clamped as u64 * FRAME_DURATION_US_60FPS
}

// =====** 延迟滑动窗口 =====**

/// 固定容量的延迟采样窗口，用于输出 p50/p95/p99。
#[derive(Debug, Clone)]
pub struct LatencyWindow {
    /// 采样容量（超过后丢弃最旧样本）。
    capacity: usize,
    /// 样本队列（毫秒）。
    samples: VecDeque<f32>,
}

impl LatencyWindow {
    /// 创建窗口。
    ///
    /// # Arguments
    /// * `capacity` - 最大样本数；传 0 会被提升为 1，避免除零
    ///
    /// # Returns
    /// 空窗口。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            samples: VecDeque::with_capacity(capacity.max(1)),
        }
    }

    /// 追加一个延迟样本（毫秒）。
    ///
    /// # Arguments
    /// * `ms` - 延迟毫秒值；非有限值（NaN/Inf）会被忽略
    ///
    /// # Returns
    /// 不会失败。
    pub fn push(&mut self, ms: f32) {
        if !ms.is_finite() {
            return;
        }
        if self.samples.len() == self.capacity {
            self.samples.pop_front();
        }
        self.samples.push_back(ms);
    }

    /// 返回分位值。
    ///
    /// # Arguments
    /// * `quantile` - 0.0~1.0，超出范围会被夹住
    ///
    /// # Returns
    /// 空窗口返回 `None`，否则返回最近秩分位值。
    ///
    /// # Errors
    /// 不会失败。
    pub fn quantile(&mut self, quantile: f32) -> Option<f32> {
        if self.samples.is_empty() {
            return None;
        }
        let mut sorted: Vec<f32> = self.samples.iter().copied().collect();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let q = quantile.clamp(0.0, 1.0);
        // 最近秩法：ceil(q*n)-1，保证 q=0 取最小、q=1 取最大。
        let idx = ((q * sorted.len() as f32).ceil() as usize).saturating_sub(1);
        Some(sorted[idx.min(sorted.len() - 1)])
    }

    /// 当前样本数。
    ///
    /// # Returns
    /// 队列长度。
    ///
    /// # Errors
    /// 不会失败。
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// 是否为空。
    ///
    /// # Returns
    /// 无样本返回 `true`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// CSV 表头与行必须列数一致。
    #[test]
    fn csv_columns_match_header() {
        let timing = FrameTiming {
            frame_id: 10,
            capture_ts: 1000,
            encode_ms: 2.5,
            send_ts: 1005,
            recv_ts: 1010,
            decode_ms: 1.0,
            present_ts: 1100,
        };
        let row = timing.to_csv_row();
        assert_eq!(row.split(',').count(), TIMING_CSV_HEADER.split(',').count());
        assert_eq!(timing.e2e_ms(), 0.1);
        assert!(row.ends_with("0.100"));
    }

    /// deadline 必须按 ts+offset+jitter 计算，并饱和到 0。
    #[test]
    fn deadline_is_offset_aware_and_saturating() {
        assert_eq!(deadline_us(1000, 0, 0), 1000);
        assert_eq!(deadline_us(1000, 50, 33_333), 34_383);
        assert_eq!(deadline_us(100, -500, 0), 0);
    }

    /// 抖动帧数必须夹在 1~3 帧。
    #[test]
    fn jitter_frames_are_clamped() {
        assert_eq!(jitter_delay_us(0), FRAME_DURATION_US_60FPS);
        assert_eq!(jitter_delay_us(9), 3 * FRAME_DURATION_US_60FPS);
    }

    /// 分位值必须单调且端点正确。
    #[test]
    fn latency_quantiles_are_monotonic() {
        let mut w = LatencyWindow::new(100);
        for ms in [1.0, 2.0, 3.0, 4.0, 5.0] {
            w.push(ms);
        }
        assert_eq!(w.quantile(0.0), Some(1.0));
        assert_eq!(w.quantile(1.0), Some(5.0));
        let p50 = w.quantile(0.5).expect("p50");
        let p95 = w.quantile(0.95).expect("p95");
        assert!(p50 <= p95);
    }

    /// 容量满后必须丢弃最旧样本。
    #[test]
    fn latency_window_evicts_oldest() {
        let mut w = LatencyWindow::new(2);
        w.push(10.0);
        w.push(20.0);
        w.push(30.0);
        assert_eq!(w.len(), 2);
        assert_eq!(w.quantile(0.0), Some(20.0));
    }
}
