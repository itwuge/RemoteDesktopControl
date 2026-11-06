//! rdc-core / 授权与角色
//!
//! 层级：L3 编排层
//! 用途：多账号授权的判定点（唯一判定点在 Rust，docs/02 §3.7）
//! 调用方：rdc-transport 的输入闸门（角色判定）、rdc-bridge（只读展示）

use serde::{Deserialize, Serialize};

// =====** Permission =====**

/// 单项权限位。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Permission {
    /// 键盘输入。
    Keyboard,
    /// 鼠标/触摸输入。
    Pointer,
    /// 剪贴板文本同步。
    ClipboardText,
    /// 文件传输。
    FileTransfer,
    /// 音频。
    Audio,
}

// =====** Role =====**

/// 会话内角色（docs/02 §3.7 会话内角色与接管）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    /// 被控端所有者。
    Owner,
    /// 当前控制者。
    Controller,
    /// 只读观看者。
    Viewer,
}

// =====** AuthzDecision =====**

/// 授权判定结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthzDecision {
    /// 是否允许该操作。
    pub allowed: bool,
    /// 判定依据（写入审计日志，不含任何内容数据）。
    pub reason: String,
}

impl AuthzDecision {
    /// 允许。
    ///
    /// # Arguments
    /// * `reason` - 判定依据
    ///
    /// # Returns
    /// `allowed = true` 的判定。
    ///
    /// # Errors
    /// 不会失败。
    pub fn allow(reason: impl Into<String>) -> Self {
        Self { allowed: true, reason: reason.into() }
    }

    /// 拒绝。
    ///
    /// # Arguments
    /// * `reason` - 拒绝原因（如 `perm_exceeded`）
    ///
    /// # Returns
    /// `allowed = false` 的判定。
    ///
    /// # Errors
    /// 不会失败。
    pub fn deny(reason: impl Into<String>) -> Self {
        Self { allowed: false, reason: reason.into() }
    }
}

/// 判断某角色是否允许某项权限。
///
/// # Arguments
/// * `role` - 会话角色
/// * `permission` - 待判定权限
///
/// # Returns
/// Owner 与 Controller 允许全部；Viewer 一律拒绝（输入闸门在传输层执行）。
///
/// # Errors
/// 不会失败。
pub fn check(role: Role, _permission: Permission) -> AuthzDecision {
    // S1 只实现角色级最小规则；令牌签名/有效期/撤销世代在 S2（docs/02 §3.7）。
    match role {
        Role::Owner | Role::Controller => AuthzDecision::allow("role_granted"),
        Role::Viewer => AuthzDecision::deny("viewer_readonly"),
    }
}
