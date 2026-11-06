//! rdc-core / 连接策略与质量阶梯
//!
//! 层级：L3 编排层
//! 用途：三档连接降级策略（docs/05）与质量降级顺序「码率→色度→分辨率→帧率」
//! 调用方：rdc-core::session、rdc-bridge（设置页读写策略）
//!
//! 关键约束：默认严格 P2P，绝不静默切中继（docs/01 C3）。

use serde::{Deserialize, Serialize};

use crate::stats::StatsSnapshot;

// =====** ConnectionPolicy =====**

/// 三档连接降级策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConnectionPolicy {
    /// 严格 P2P：打洞失败直接报错。
    StrictP2p,
    /// 询问后降级：打洞失败弹窗询问用户是否允许中继。
    AskThenFallback,
    /// 自动降级：仅在用户开启中继时自动走中继。
    AutoFallback,
}

impl Default for ConnectionPolicy {
    /// 默认严格 P2P（docs/01 C3）。
    fn default() -> Self {
        Self::StrictP2p
    }
}

// =====** QualityStep =====**

/// 质量阶梯中的一档，顺序固定：先降码率，帧率最后降（docs/01 C5）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QualityStep {
    /// 降低目标码率。
    Bitrate,
    /// 降低色度采样（4:4:4 → 4:2:0）。
    Chroma,
    /// 降低分辨率。
    Resolution,
    /// 降低帧率（最后手段）。
    FrameRate,
}

// =====** QualityLadder =====**

/// 质量阶梯决策器。
pub struct QualityLadder {
    /// 当前档位索引，0 表示未降级。
    step: usize,
}

impl QualityLadder {
    /// 创建阶梯。
    ///
    /// # Returns
    /// 初始档位 0 的决策器。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new() -> Self {
        Self { step: 0 }
    }

    /// 根据统计快照决定是否需要降级。
    ///
    /// # Arguments
    /// * `snapshot` - 4Hz 聚合后的统计快照
    ///
    /// # Returns
    /// 需要降级时返回下一档 [`QualityStep`]；指标正常返回 `None`（不主动回升，回升需稳定期）。
    ///
    /// # Errors
    /// 不会失败。
    pub fn decide(&mut self, _snapshot: &StatsSnapshot) -> Option<QualityStep> {
        // TODO(stage5): 按丢包率/RTT/队列深度阈值实现，并写入降级原因供审计。
        todo!("S1 空壳：质量阶梯在 stage5 实现")
    }

    /// 当前档位索引。
    ///
    /// # Returns
    /// 已降级的次数。
    ///
    /// # Errors
    /// 不会失败。
    pub fn current_step(&self) -> usize {
        self.step
    }
}

impl Default for QualityLadder {
    /// 等价于 [`QualityLadder::new`]。
    fn default() -> Self {
        Self::new()
    }
}
