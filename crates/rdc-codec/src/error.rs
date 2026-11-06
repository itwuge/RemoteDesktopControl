//! rdc-codec / 编解码错误类型
//!
//! 层级：L3 编解码层
//! 用途：描述硬件能力缺失、配置非法与运行期编解码失败的终态
//! 调用方：rdc-codec 各平台后端、rdc-core（收敛为 RdcError）、rdc-bridge（错误码映射）

use thiserror::Error;

// =====** CodecError =====**

/// 编解码阶段错误。
///
/// 设计要点：能力类错误（`HardwareUnsupported` / `Unsupported`）可触发软编降级；
/// 运行期错误（`EncoderFailure` / `DecoderFailure`）由 rdc-core 决定重建会话或终止。
#[derive(Debug, Error)]
pub enum CodecError {
    /// 平台无可用硬件编码器（或驱动未就绪）。
    #[error("硬件编码器不可用")]
    HardwareUnsupported,
    /// 配置非法（分辨率/帧率/码率/位深/色度超出能力）。
    #[error("编解码配置非法: {0}")]
    InvalidConfig(String),
    /// 编码器运行期失败。
    #[error("编码器失败: {0}")]
    EncoderFailure(String),
    /// 解码器运行期失败。
    #[error("解码器失败: {0}")]
    DecoderFailure(String),
    /// 当前平台不支持编解码能力。
    #[error("编解码能力不受支持")]
    Unsupported,
}
