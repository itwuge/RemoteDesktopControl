//! rdc-transport / 发送节流
//!
//! 层级：L2 能力层
//! 用途：把拥塞控制器给出的目标码率换算成每个分片的发送延迟，平滑出包避免整秒突刺
//! 调用方：rdc-core 的发送任务（每 `PACER_TICK_MS` 查询一次延迟）

use std::time::{Duration, Instant};

use crate::error::TransportError;

// =====** Pacer =====**

/// 令牌桶式发送节流器。
pub struct Pacer {
    /// 目标码率（bit/s），由拥塞控制器更新。
    target_bps: u32,
    /// 允许的瞬时突发字节数，吸收编码器的帧级抖动。
    burst_bytes: u32,
}

impl Pacer {
    /// 构造节流器。
    ///
    /// # Arguments
    /// * `target_bps` - 初始目标码率（bit/s），必须大于 0
    /// * `burst_bytes` - 允许的瞬时突发字节数
    ///
    /// # Returns
    /// 节流器实例。
    ///
    /// # Errors
    /// `target_bps` 为 0 时返回 [`TransportError::InvalidRate`]：
    /// 0 码率会让发送永远不被调度，属于调用方配置错误。
    pub fn new(target_bps: u32, burst_bytes: u32) -> Result<Self, TransportError> {
        if target_bps == 0 {
            return Err(TransportError::InvalidRate);
        }
        Ok(Self {
            target_bps,
            burst_bytes,
        })
    }

    /// 更新目标码率。
    ///
    /// # Arguments
    /// * `target_bps` - 新目标码率（bit/s），必须大于 0
    ///
    /// # Returns
    /// 更新成功时返回 `()`。
    ///
    /// # Errors
    /// `target_bps` 为 0 时返回 [`TransportError::InvalidRate`]，且保持旧码率不变。
    pub fn set_target_bitrate(&mut self, target_bps: u32) -> Result<(), TransportError> {
        if target_bps == 0 {
            return Err(TransportError::InvalidRate);
        }
        self.target_bps = target_bps;
        Ok(())
    }

    /// 计算距下一个分片可发送还需等待的时间。
    ///
    /// # Arguments
    /// * `now` - 调用时刻，用单调时钟避免受系统时间调整影响
    ///
    /// # Returns
    /// 需要等待的时长；`ZERO` 表示可立即发送。
    ///
    /// # Errors
    /// 不会失败。
    ///
    /// # Panics
    /// S1 空壳：实现落地前调用会 panic。
    pub fn poll_delay(&self, _now: Instant) -> Duration {
        todo!("S1 空壳：pacer 延迟计算在 S2 实现")
    }

    /// 返回当前目标码率。
    ///
    /// # Returns
    /// 目标码率（bit/s）。
    ///
    /// # Errors
    /// 不会失败。
    pub fn target_bitrate(&self) -> u32 {
        self.target_bps
    }
}
