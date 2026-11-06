//! rdc-decode / 解码队列
//!
//! 层级：L3 编解码层
//! 用途：解码输出按时间码排序，给呈现线程按序取帧；乱序等待有上限、队列有界
//!       （docs/05 §2：解码队列 4 帧，陈旧帧丢弃不追帧）。
//! 调用方：rdc-core 的 `rdc-decode` 任务（解码后入队）、呈现线程（按节拍取帧）

use std::collections::BTreeMap;

/// 默认队列深度（帧）。
pub const DEFAULT_DEPTH: usize = 4;

/// 默认乱序等待上限（微秒）：超过即整帧丢弃，避免延迟累积。
pub const DEFAULT_WAIT_LIMIT_US: u64 = 33_334;

// =====** DecodeQueue =====**

/// 有界、按时间码排序的解码队列。
pub struct DecodeQueue {
    /// 最大帧数；超出时丢弃最旧的一帧。
    capacity: usize,
    /// 乱序等待上限（微秒）。
    wait_limit_us: u64,
    /// 时间码 → 帧载荷；`BTreeMap` 保证出队严格按时间码升序。
    frames: BTreeMap<u64, Vec<u8>>,
    /// 已交付的最大时间码，用于识别迟到帧（时间码回退）。
    last_delivered: Option<u64>,
    /// 因队列满被挤出的帧数。
    dropped_overflow: u64,
    /// 因时间码过旧被丢弃的帧数。
    dropped_late: u64,
}

impl DecodeQueue {
    /// 构造队列。
    ///
    /// # Arguments
    /// * `capacity` - 最大帧数，传 0 会被提升为 1
    /// * `wait_limit_us` - 乱序等待上限（微秒）
    ///
    /// # Returns
    /// 空队列。
    pub fn new(capacity: usize, wait_limit_us: u64) -> Self {
        Self {
            capacity: capacity.max(1),
            wait_limit_us,
            frames: BTreeMap::new(),
            last_delivered: None,
            dropped_overflow: 0,
            dropped_late: 0,
        }
    }

    /// 入队一帧。
    ///
    /// # Arguments
    /// * `ts_us` - 帧时间码（微秒，已换算到本端时钟）
    /// * `payload` - 解码后的帧数据
    ///
    /// # Returns
    /// 入队成功返回 `true`；时间码早于已交付帧（迟到）或与在队帧重复时返回 `false`。
    /// 队列满时丢弃最旧的一帧后仍接受新帧。
    pub fn push(&mut self, ts_us: u64, payload: Vec<u8>) -> bool {
        if let Some(last) = self.last_delivered {
            if ts_us < last {
                self.dropped_late += 1;
                return false;
            }
        }
        if self.frames.contains_key(&ts_us) {
            self.dropped_late += 1;
            return false;
        }
        while self.frames.len() >= self.capacity {
            if let Some(oldest) = self.frames.keys().next().copied() {
                self.frames.remove(&oldest);
                self.dropped_overflow += 1;
            } else {
                break;
            }
        }
        self.frames.insert(ts_us, payload);
        true
    }

    /// 取出到期的下一帧。
    ///
    /// # Arguments
    /// * `now_us` - 当前本端时钟（微秒）
    /// * `delay_us` - 期望的额外呈现延迟（微秒），用于吸收到达抖动
    ///
    /// # Returns
    /// 时间码 + 延迟 ≤ 当前时刻时返回 `(时间码, 载荷)`，否则 `None`。
    pub fn pop_ready(&mut self, now_us: u64, delay_us: u64) -> Option<(u64, Vec<u8>)> {
        let (ts, _) = self.frames.iter().next()?;
        let ts = *ts;
        if ts.saturating_add(delay_us) > now_us {
            return None;
        }
        let payload = self.frames.remove(&ts)?;
        self.last_delivered = Some(ts);
        Some((ts, payload))
    }

    /// 丢弃超过等待上限仍无法出队的陈旧帧。
    ///
    /// # Arguments
    /// * `now_us` - 当前本端时钟（微秒）
    ///
    /// # Returns
    /// 本次丢弃的帧数。
    pub fn flush_stale(&mut self, now_us: u64) -> usize {
        let mut removed = 0usize;
        while let Some((ts, _)) = self.frames.iter().next() {
            let ts = *ts;
            if ts.saturating_add(self.wait_limit_us) > now_us {
                break;
            }
            self.frames.remove(&ts);
            self.dropped_late += 1;
            removed += 1;
        }
        removed
    }

    /// 当前队列深度（帧）。
    pub fn depth(&self) -> usize {
        self.frames.len()
    }

    /// 因队列满被挤出的帧数。
    pub fn dropped_overflow(&self) -> u64 {
        self.dropped_overflow
    }

    /// 因过期/迟到被丢弃的帧数。
    pub fn dropped_late(&self) -> u64 {
        self.dropped_late
    }
}

// =====** 自检 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 乱序入队后必须严格按时间码出队。
    #[test]
    fn pop_order_follows_timestamp() {
        let mut queue = DecodeQueue::new(4, DEFAULT_WAIT_LIMIT_US);
        assert!(queue.push(300, vec![3]));
        assert!(queue.push(100, vec![1]));
        assert!(queue.push(200, vec![2]));
        assert_eq!(queue.pop_ready(1_000, 0), Some((100, vec![1])));
        assert_eq!(queue.pop_ready(1_000, 0), Some((200, vec![2])));
        assert_eq!(queue.pop_ready(1_000, 0), Some((300, vec![3])));
        assert_eq!(queue.pop_ready(1_000, 0), None);
    }

    /// 未到呈现时刻的帧不得提前出队。
    #[test]
    fn pop_respects_delay() {
        let mut queue = DecodeQueue::new(4, DEFAULT_WAIT_LIMIT_US);
        assert!(queue.push(10_000, vec![1]));
        assert_eq!(queue.pop_ready(15_000, 8_000), None, "10ms + 8ms 延迟 > 15ms");
        assert_eq!(queue.pop_ready(18_000, 8_000), Some((10_000, vec![1])));
    }

    /// 队列满时挤掉最旧的一帧，保证深度有界。
    #[test]
    fn overflow_drops_oldest() {
        let mut queue = DecodeQueue::new(2, DEFAULT_WAIT_LIMIT_US);
        assert!(queue.push(10, vec![1]));
        assert!(queue.push(20, vec![2]));
        assert!(queue.push(30, vec![3]));
        assert_eq!(queue.depth(), 2);
        assert_eq!(queue.dropped_overflow(), 1);
        assert_eq!(queue.pop_ready(1_000, 0), Some((20, vec![2])), "时间码 10 应被挤出");
    }

    /// 时间码早于已交付帧的迟到帧必须丢弃并计数。
    #[test]
    fn late_frame_is_rejected() {
        let mut queue = DecodeQueue::new(4, DEFAULT_WAIT_LIMIT_US);
        assert!(queue.push(10, vec![1]));
        assert_eq!(queue.pop_ready(1_000, 0), Some((10, vec![1])));
        assert!(!queue.push(5, vec![0]), "迟到帧不应入队");
        assert_eq!(queue.dropped_late(), 1);
    }

    /// 超过乱序等待上限的陈旧帧应被 flush 掉。
    #[test]
    fn stale_frames_are_flushed() {
        let mut queue = DecodeQueue::new(4, 10_000);
        assert!(queue.push(100, vec![1]));
        assert!(queue.push(500, vec![2]));
        assert_eq!(queue.flush_stale(20_000), 2);
        assert_eq!(queue.depth(), 0);
        assert_eq!(queue.dropped_late(), 2);
    }

    /// 重复时间码只保留第一份。
    #[test]
    fn duplicate_timestamp_is_rejected() {
        let mut queue = DecodeQueue::new(4, DEFAULT_WAIT_LIMIT_US);
        assert!(queue.push(100, vec![1]));
        assert!(!queue.push(100, vec![9]));
        assert_eq!(queue.depth(), 1);
    }
}
