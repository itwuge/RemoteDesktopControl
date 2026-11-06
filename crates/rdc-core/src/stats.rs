//! rdc-core / 统计聚合
//!
//! 层级：L3 编排层
//! 用途：把原子量快照聚合成 4Hz 推送对象（docs/01 §3.5 环 G）
//! 调用方：rdc-core 统计任务、rdc-bridge 的事件流

use serde::{Deserialize, Serialize};

// =====** StatsSnapshot =====**

/// 一次统计快照，4Hz 推送给 Dart。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatsSnapshot {
    /// 实际发送码率（bps）。
    pub bitrate_bps: u32,
    /// 实际呈现帧率。
    pub fps: u32,
    /// 丢包率（千分比，0~1000，避免 f32 跨 FFI 精度问题）。
    pub loss_permille: u32,
    /// 往返时延（毫秒）。
    pub rtt_ms: u32,
    /// 端到端 P99 延迟（毫秒，来自画面时间码）。
    pub e2e_p99_ms: u16,
    /// 发送队列积压（毫秒）。
    pub send_queue_ms: u16,
    /// 编码前丢帧累计。
    pub dropped_pre_encode: u64,
    /// 解码覆盖（解码跟不上）累计。
    pub decode_overwrite: u64,
}

// =====** StatsAggregator =====**

/// 统计聚合器。
///
/// 设计要点：只保留最新聚合值，前端不感知丢事件（docs/01 §3.5 环 G）。
#[derive(Debug, Default)]
pub struct StatsAggregator {
    /// 最近一次快照。
    latest: StatsSnapshot,
}

impl StatsAggregator {
    /// 创建空聚合器。
    ///
    /// # Returns
    /// `latest` 全零的聚合器。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new() -> Self {
        Self::default()
    }

    /// 更新快照。
    ///
    /// # Arguments
    /// * `snapshot` - 新的统计快照
    ///
    /// # Errors
    /// 不会失败。
    pub fn update(&mut self, snapshot: StatsSnapshot) {
        // 直接覆盖：4Hz 推送只需要最新值，保留历史会与"不增长缓冲"原则冲突。
        self.latest = snapshot;
    }

    /// 读取最新快照。
    ///
    /// # Returns
    /// 最近一次 [`StatsSnapshot`]；从未更新时返回全零。
    ///
    /// # Errors
    /// 不会失败。
    pub fn latest(&self) -> StatsSnapshot {
        self.latest
    }
}
