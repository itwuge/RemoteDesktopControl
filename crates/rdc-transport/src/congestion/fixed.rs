//! rdc-transport / 固定码率控制
//!
//! 层级：L2 能力层（`congestion` 子模块）
//! 用途：始终返回构造时给定的码率，用于调试、回放与极端受限链路
//! 调用方：`congestion::create_controller` 构造，rdc-core 网络任务线程驱动

use super::CongestionController;

// =====** FixedController =====**

/// 固定码率控制器。
pub struct FixedController {
    /// 恒定目标码率（bit/s）。
    target_bps: u32,
}

impl FixedController {
    /// 构造固定码率控制器。
    ///
    /// # Arguments
    /// * `initial_bps` - 恒定目标码率（bit/s）
    ///
    /// # Returns
    /// 实例。
    ///
    /// # Errors
    /// 不会失败。
    pub(crate) fn new(initial_bps: u32) -> Self {
        Self {
            target_bps: initial_bps,
        }
    }
}

impl CongestionController for FixedController {
    /// 忽略 ACK 反馈。
    ///
    /// # Arguments
    /// * `_batch` - 本批已确认序号，本实现不关心
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    fn on_ack(&mut self, _batch: &[u32]) {}

    /// 忽略丢包反馈。
    ///
    /// # Arguments
    /// * `_loss_rate` - 丢包率，本实现不关心
    /// * `_burst_len` - 最大连续丢包长度，本实现不关心
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    fn on_loss(&mut self, _loss_rate: f32, _burst_len: u32) {}

    /// 忽略 RTT 反馈。
    ///
    /// # Arguments
    /// * `_rtt_ms` - RTT（毫秒），本实现不关心
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    fn on_rtt(&mut self, _rtt_ms: u32) {}

    /// 返回恒定目标码率。
    ///
    /// # Returns
    /// 构造时给定的码率（bit/s）。
    ///
    /// # Errors
    /// 不会失败。
    fn target_bitrate(&self) -> u32 {
        self.target_bps
    }

    /// 忽略探测结果。
    ///
    /// # Arguments
    /// * `_delivered_bps` - 实测交付码率，本实现不关心
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    fn on_probe_result(&mut self, _delivered_bps: u32) {}
}
