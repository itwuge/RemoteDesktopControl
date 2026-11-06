//! rdc-transport / GCC 拥塞控制
//!
//! 层级：L2 能力层（`congestion` 子模块）
//! 用途：基于延迟梯度与丢包信号估计可用带宽；S1 只保留目标码率字段与接口
//! 调用方：`congestion::create_controller` 构造，rdc-core 网络任务线程驱动

use super::CongestionController;

// =====** GccController =====**

/// 基于 Google Congestion Control 思路的控制器。
pub struct GccController {
    /// 当前目标码率（bit/s），S1 为初始值，S2 由估计器更新。
    target_bps: u32,
}

impl GccController {
    /// 构造一个估计尚未启动的控制器。
    ///
    /// # Arguments
    /// * `initial_bps` - 初始目标码率（bit/s）
    ///
    /// # Returns
    /// 目标码率等于 `initial_bps` 的实例。
    ///
    /// # Errors
    /// 不会失败。
    pub(crate) fn new(initial_bps: u32) -> Self {
        Self {
            target_bps: initial_bps,
        }
    }
}

impl CongestionController for GccController {
    /// 记录 ACK 批量，S2 用于计算交付码率。
    ///
    /// # Arguments
    /// * `_batch` - 本批已确认序号
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    ///
    /// # Panics
    /// S1 空壳：实现落地前调用会 panic。
    fn on_ack(&mut self, _batch: &[u32]) {
        todo!("TODO(stage2)：GCC 的 ACK 处理在 S2 实现")
    }

    /// 记录丢包信号，S2 用于切换过载状态。
    ///
    /// # Arguments
    /// * `_loss_rate` - 丢包率
    /// * `_burst_len` - 最大连续丢包长度
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    ///
    /// # Panics
    /// S1 空壳：实现落地前调用会 panic。
    fn on_loss(&mut self, _loss_rate: f32, _burst_len: u32) {
        todo!("TODO(stage2)：GCC 的丢包响应在 S2 实现")
    }

    /// 记录 RTT 样本，S2 用于计算延迟梯度。
    ///
    /// # Arguments
    /// * `_rtt_ms` - 平滑后的 RTT（毫秒）
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    ///
    /// # Panics
    /// S1 空壳：实现落地前调用会 panic。
    fn on_rtt(&mut self, _rtt_ms: u32) {
        todo!("TODO(stage2)：GCC 的 RTT 处理在 S2 实现")
    }

    /// 返回当前目标码率。
    ///
    /// # Returns
    /// 目标码率（bit/s）。
    ///
    /// # Errors
    /// 不会失败。
    fn target_bitrate(&self) -> u32 {
        self.target_bps
    }

    /// 应用探测结果，S2 用于快速提升/回退。
    ///
    /// # Arguments
    /// * `_delivered_bps` - 探测窗口实测交付码率
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    ///
    /// # Panics
    /// S1 空壳：实现落地前调用会 panic。
    fn on_probe_result(&mut self, _delivered_bps: u32) {
        todo!("TODO(stage2)：GCC 的探测处理在 S2 实现")
    }
}
