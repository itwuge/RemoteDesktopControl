//! rdc-capture / 采集错误类型
//!
//! 层级：L3 采集层
//! 用途：描述采集后端在权限、显示器生命周期、独占占用与平台能力上的失败原因
//! 调用方：rdc-capture 各平台后端、rdc-core（收敛为 RdcError）、rdc-bridge（错误码映射）

use thiserror::Error;

// =====** CaptureError =====**

/// 采集阶段错误。
///
/// 设计要点：每个变体对应一种可判别终态，供上层决定是否重试或降级；
/// `Internal` 只承载无法归类的后端诊断文本，不用于常规控制流。
#[derive(Debug, Error)]
pub enum CaptureError {
    /// 缺少屏幕录制系统权限（macOS 屏幕录制授权、Linux portal 授权等）。
    #[error("采集权限被拒绝")]
    PermissionDenied,
    /// 系统没有任何活跃显示输出（显示器关闭、未连接或睡眠）。
    ///
    /// 与 RDR-001 的 `no_display_output` 是同一根因：桌面不再产生新画面，
    /// 采集初始化必然拿不到可复制的输出。必须报这一条，不能报笼统「采集失败」。
    #[error("无活跃显示输出（显示器已关闭、未连接或睡眠）")]
    NoActiveDisplay,
    /// 目标显示器已移除（热插拔、休眠唤醒或拓扑切换）。
    #[error("显示器已消失: display_id={display_id}")]
    DisplayDisappeared {
        /// 消失的显示器编号。
        display_id: u32,
    },
    /// 目标显示器被其它会话独占，无法复制其输出。
    #[error("显示器被占用: display_id={display_id}")]
    DisplayBusy {
        /// 被占用的显示器编号。
        display_id: u32,
    },
    /// 当前平台不支持屏幕采集能力。
    #[error("采集能力不受支持")]
    Unsupported,
    /// 后端返回的非预期错误，保留原文用于诊断与日志。
    #[error("采集内部错误: {0}")]
    Internal(String),
}
