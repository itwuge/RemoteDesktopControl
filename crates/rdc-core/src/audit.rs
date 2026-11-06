//! rdc-core / 审计日志
//!
//! 层级：L3 编排层
//! 用途：仅记录元数据的本地环形日志（docs/02 §3.7 审计日志）
//! 调用方：rdc-core 各决策点、rdc-bridge 只读查询
//!
//! 红线：审计字段由 Rust 定义，禁止 Dart 追加自由文本以防注入；不得含媒体内容。

use serde::{Deserialize, Serialize};

// =====** AuditEvent =====**

/// 审计事件（仅元数据）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEvent {
    /// Unix 毫秒时间戳。
    pub ts_ms: u64,
    /// 事件类别，如 `connect` / `takeover` / `kick` / `clipboard`。
    pub kind: String,
    /// 会话 ID（可为空）。
    pub session_id: Option<String>,
    /// 结果，如 `allowed` / `denied`。
    pub outcome: String,
}
