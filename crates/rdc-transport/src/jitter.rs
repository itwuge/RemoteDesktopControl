//! rdc-transport / 抖动缓冲
//!
//! 层级：L2 能力层
//! 用途：按时间码排序并平滑出队，吸收网络到达抖动；超过 deadline 的帧直接丢弃，
//!       不做「追帧」（docs/04 §6：宁可丢帧，也不让延迟累积）。
//! 调用方：rdc-core 的 `rdc-rx` 任务（重组完成后入缓冲）、呈现线程（按刷新节拍取帧）

// =====** JitterBuffer =====**

/// 缓冲中的一帧。
#[derive(Debug, Clone)]
struct BufferedFrame {
    /// 呈现时间戳（微秒，已换算到本端时钟）。
    deadline_us: u64,
    /// 帧载荷。
    payload: Vec<u8>,
}

/// 按时间码出队的抖动缓冲。
pub struct JitterBuffer {
    /// 按 `deadline_us` 升序排列的帧。
    frames: Vec<BufferedFrame>,
    /// 额外交付延迟（微秒），由 95 分位抖动自适应调整。
    delay_us: u64,
    /// 超过 deadline 多久后判定为迟到并丢弃（微秒）。
    max_late_us: u64,
    /// 迟到丢弃计数。
    late_frames: u64,
    /// 观测到的到达抖动（微秒，指数滑动平均）。
    jitter_us: u64,
    /// 上一帧的时间戳与到达时刻，用于计算抖动样本。
    previous: Option<(u64, u64)>,
}

impl JitterBuffer {
    /// 构造缓冲。
    ///
    /// # Arguments
    /// * `delay_us` - 初始交付延迟（微秒），生产初始值取 1 帧时长
    ///
    /// # Returns
    /// 空缓冲实例，`max_late_us` 默认为 1 帧（16.6ms）。
    pub fn new(delay_us: u64) -> Self {
        Self {
            frames: Vec::new(),
            delay_us,
            max_late_us: 16_667,
            late_frames: 0,
            jitter_us: 0,
            previous: None,
        }
    }

    /// 调整交付延迟（微秒）。
    pub fn set_delay_us(&mut self, delay_us: u64) {
        self.delay_us = delay_us;
    }

    /// 当前交付延迟（微秒）。
    pub fn delay_us(&self) -> u64 {
        self.delay_us
    }

    /// 入缓冲。
    ///
    /// # Arguments
    /// * `ts_us` - 帧的原始时间戳（微秒，采集端单调时钟，调用方需先换算到本端）
    /// * `arrival_us` - 本端收到该帧的时刻（微秒）
    /// * `payload` - 完整帧载荷
    ///
    /// # Returns
    /// 无返回值；重复时间戳的帧会被直接丢弃（同一时刻只呈现一帧）。
    pub fn push(&mut self, ts_us: u64, arrival_us: u64, payload: Vec<u8>) {
        if let Some((previous_ts, previous_arrival)) = self.previous {
            if ts_us > previous_ts && arrival_us > previous_arrival {
                let ts_delta = ts_us - previous_ts;
                let arrival_delta = arrival_us - previous_arrival;
                let deviation = ts_delta.abs_diff(arrival_delta);
                // 1/8 的指数滑动平均，既能跟上升高的抖动，也不会被单帧尖峰带飞。
                self.jitter_us = (self.jitter_us * 7 + deviation) / 8;
            }
        }
        self.previous = Some((ts_us, arrival_us));

        let deadline_us = ts_us.saturating_add(self.delay_us);
        if self.frames.iter().any(|f| f.deadline_us == deadline_us) {
            return;
        }
        let position = self
            .frames
            .iter()
            .position(|f| f.deadline_us > deadline_us)
            .unwrap_or(self.frames.len());
        self.frames.insert(
            position,
            BufferedFrame {
                deadline_us,
                payload,
            },
        );
    }

    /// 取出到期的下一帧。
    ///
    /// # Arguments
    /// * `now_us` - 当前本端时钟（微秒）
    ///
    /// # Returns
    /// 到期的帧载荷；尚未到期或缓冲为空时返回 `None`。
    ///
    /// 超过 `deadline + max_late` 的帧会被丢弃并计入 [`Self::late_frames`]，
    /// 避免旧帧把播放时刻拖后。
    pub fn pop_ready(&mut self, now_us: u64) -> Option<Vec<u8>> {
        let threshold = now_us.saturating_sub(self.max_late_us);
        while let Some(frame) = self.frames.first() {
            if frame.deadline_us > threshold {
                break;
            }
            self.frames.remove(0);
            self.late_frames += 1;
        }
        if let Some(frame) = self.frames.first() {
            if frame.deadline_us <= now_us {
                let frame = self.frames.remove(0);
                return Some(frame.payload);
            }
        }
        None
    }

    /// 当前缓冲深度（帧）。
    pub fn depth(&self) -> usize {
        self.frames.len()
    }

    /// 累计迟到丢弃帧数。
    pub fn late_frames(&self) -> u64 {
        self.late_frames
    }

    /// 当前抖动估计（微秒）。
    pub fn jitter_us(&self) -> u64 {
        self.jitter_us
    }
}

// =====** 自检 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 乱序到达时必须按时间码出队。
    #[test]
    fn dequeue_follows_timestamp_order() {
        let mut buffer = JitterBuffer::new(0);
        buffer.push(300, 10, vec![3]);
        buffer.push(100, 20, vec![1]);
        buffer.push(200, 30, vec![2]);
        assert_eq!(buffer.pop_ready(350), Some(vec![1]));
        assert_eq!(buffer.pop_ready(350), Some(vec![2]));
        assert_eq!(buffer.pop_ready(350), Some(vec![3]));
        assert_eq!(buffer.pop_ready(350), None);
    }

    /// 未到期的帧不得提前出队。
    #[test]
    fn frame_waits_for_delay() {
        let mut buffer = JitterBuffer::new(16_000);
        buffer.push(100_000, 100_000, vec![1]);
        assert_eq!(buffer.pop_ready(110_000), None, "deadline 116ms 之前不应出队");
        assert_eq!(buffer.pop_ready(117_000), Some(vec![1]));
    }

    /// 超过 deadline 太久的帧必须丢弃，不允许拖慢播放。
    #[test]
    fn late_frame_is_dropped() {
        let mut buffer = JitterBuffer::new(0);
        buffer.push(100, 100, vec![1]);
        buffer.push(200, 200, vec![2]);
        // now=400，max_late=16.6ms → deadline 100/200 都过期。
        assert_eq!(buffer.pop_ready(400_000), None);
        assert_eq!(buffer.late_frames(), 2);
        assert_eq!(buffer.depth(), 0);
    }

    /// 深度与抖动估计要随入队更新。
    #[test]
    fn depth_and_jitter_are_tracked() {
        let mut buffer = JitterBuffer::new(0);
        buffer.push(0, 0, vec![0]);
        buffer.push(33_000, 40_000, vec![1]);
        assert_eq!(buffer.depth(), 2);
        // 时间戳差 33ms、到达差 40ms → 抖动样本 7ms，EWMA 后约 0.875ms。
        assert!(buffer.jitter_us() > 0 && buffer.jitter_us() <= 7_000);
    }

    /// 同一时间戳重复入队只保留一份。
    #[test]
    fn duplicate_timestamp_is_ignored() {
        let mut buffer = JitterBuffer::new(0);
        buffer.push(100, 100, vec![1]);
        buffer.push(100, 120, vec![2]);
        assert_eq!(buffer.depth(), 1);
        assert_eq!(buffer.pop_ready(200), Some(vec![1]));
    }
}
