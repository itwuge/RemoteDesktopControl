//! rdc-input / 输入错误类型
//!
//! 层级：L3 输入层
//! 用途：描述输入注入在平台能力、系统拒绝与视口参数上的失败原因
//! 调用方：rdc-input 各平台注入器、rdc-core（收敛为 RdcError）、rdc-bridge（错误码映射）

use thiserror::Error;

// =====** InputError =====**

/// 输入注入阶段错误。
///
/// 设计要点：`InjectionRejected` 通常不可重试（如安全桌面/UAC 提示），
/// rdc-core 据此丢弃本批事件而不是重放；`InvalidViewport` 属编程错误，由上层立即修正。
#[derive(Debug, Error)]
pub enum InputError {
    /// 当前平台不支持输入注入。
    #[error("输入注入能力不受支持")]
    Unsupported,
    /// 系统拒绝注入（权限不足、目标不可交互或处于安全桌面）。
    #[error("输入注入被系统拒绝")]
    InjectionRejected,
    /// 视口参数非法（宽高为 0 或缩放比非正）。
    #[error("视口参数非法")]
    InvalidViewport,
    /// 输入事件载荷非法（长度不符、未知 tag 或非法枚举值）。
    #[error("输入事件载荷非法")]
    MalformedPayload,
}
