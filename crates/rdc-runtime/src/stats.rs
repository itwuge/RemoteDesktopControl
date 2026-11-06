//! rdc-runtime / 原子统计计数器
//!
//! 层级：L2 能力层
//! 用途：码率/丢帧/队列深度等指标的原子累加与快照（docs/01 §3.6）
//! 调用方：rdc-core 的统计聚合任务（4Hz 快照）、各线程的埋点

use std::sync::atomic::{AtomicU64, Ordering};

// =====** AtomicCounters =====**

/// 一组无锁统计计数器。
///
/// 锁注释：本结构**不使用任何锁**；每个字段独立原子量，读取快照允许轻微不一致
/// （docs/01 §3.6 统计指标行）。
#[derive(Debug, Default)]
pub struct AtomicCounters {
    /// 编码前丢帧数。
    dropped_pre_encode_count: AtomicU64,
    /// 发送队列积压毫秒数。
    send_queue_ms: AtomicU64,
    /// 迟到帧数。
    late_frames: AtomicU64,
    /// 输入队列溢出数。
    input_overflow_count: AtomicU64,
}

impl AtomicCounters {
    /// 累加编码前丢帧。
    ///
    /// # Arguments
    /// * `delta` - 本次新增丢帧数
    ///
    /// # Returns
    /// 累加后的总值。
    ///
    /// # Errors
    /// 不会失败。
    pub fn add_dropped_pre_encode(&self, delta: u64) -> u64 {
        // Relaxed 足够：统计只用于展示与 QoS 趋势，不需要跨变量顺序保证。
        self.dropped_pre_encode_count.fetch_add(delta, Ordering::Relaxed) + delta
    }

    /// 写入当前发送队列积压毫秒数。
    ///
    /// # Arguments
    /// * `value` - 当前积压毫秒
    ///
    /// # Errors
    /// 不会失败。
    pub fn set_send_queue_ms(&self, value: u64) {
        self.send_queue_ms.store(value, Ordering::Relaxed);
    }

    /// 累加输入队列溢出。
    ///
    /// # Arguments
    /// * `delta` - 本次新增溢出事件数
    ///
    /// # Errors
    /// 不会失败。
    pub fn add_input_overflow(&self, delta: u64) {
        self.input_overflow_count.fetch_add(delta, Ordering::Relaxed);
    }

    /// 生成统计快照。
    ///
    /// # Returns
    /// 带四个字段的普通结构体元组，供 4Hz 事件推送使用。
    ///
    /// # Errors
    /// 不会失败。
    pub fn snapshot(&self) -> (u64, u64, u64, u64) {
        (
            self.dropped_pre_encode_count.load(Ordering::Relaxed),
            self.send_queue_ms.load(Ordering::Relaxed),
            self.late_frames.load(Ordering::Relaxed),
            self.input_overflow_count.load(Ordering::Relaxed),
        )
    }
}
