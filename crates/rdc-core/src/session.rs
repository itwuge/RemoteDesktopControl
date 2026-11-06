//! rdc-core / 会话状态机
//!
//! 层级：L3 编排层
//! 用途：会话状态的唯一持有者；状态只能在 `rdc-ctl` 线程内变更（docs/01 §3.6）
//! 调用方：rdc-bridge（命令与事件）、rdc-cli、rdc-core::policy

use serde::{Deserialize, Serialize};

use crate::error::RdcError;

// =====** SessionState =====**

/// 会话状态，与 docs/01 §6 状态图一一对应。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionState {
    /// 空闲。
    Idle,
    /// 被控端注册成功。
    Registered,
    /// 主控端发起连接。
    Resolving,
    /// 候选交换/打洞中。
    Negotiating,
    /// 安全握手。
    SecureHandshake,
    /// 策略决策（打洞全失败后）。
    PolicyDecision,
    /// 流媒体中。
    Streaming,
    /// 质量降级中。
    Degrading,
    /// 断线重连。
    Reconnecting,
    /// 关闭中。
    Closing,
    /// 失败终态。
    Failed,
}

// =====** SessionEvent =====**

/// 会话状态变化事件（Rust → Dart 的即时事件，docs/01 §4.1）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionEvent {
    /// 会话 ID。
    pub session_id: String,
    /// 新状态。
    pub state: SessionState,
    /// 附加说明（降级原因、错误提示）。
    pub detail: Option<String>,
}

// =====** Session =====**

/// 一个会话。
///
/// # 线程契约
/// - 状态变更只在 `rdc-ctl` 线程内发生；
/// - 外部只能通过有界 channel 发消息（容量 64），禁止直接改字段。
pub struct Session {
    /// 会话 ID，创建后不变。
    id: String,
    /// 当前状态。
    state: SessionState,
}

impl Session {
    /// 创建空闲会话。
    ///
    /// # Arguments
    /// * `session_id` - 会话 ID（信令分配或本地生成）
    ///
    /// # Returns
    /// `state = Idle` 的会话。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new(session_id: impl Into<String>) -> Self {
        Self {
            id: session_id.into(),
            state: SessionState::Idle,
        }
    }

    /// 返回会话 ID。
    ///
    /// # Returns
    /// 创建时传入的 ID。
    ///
    /// # Errors
    /// 不会失败。
    pub fn id(&self) -> &str {
        &self.id
    }

    /// 返回当前状态。
    ///
    /// # Returns
    /// 当前 [`SessionState`] 快照。
    ///
    /// # Errors
    /// 不会失败。
    pub fn state(&self) -> SessionState {
        self.state
    }

    /// 主控端发起连接。
    ///
    /// # Arguments
    /// * `target` - 目标设备 ID
    /// * `pairing_code` - 配对码；免密登录时为 `None`
    ///
    /// # Returns
    /// 成功时状态推进到 `Resolving`。
    ///
    /// # Errors
    /// 状态不是 `Idle` 时返回 [`RdcError`]（`InvalidState`）。
    pub fn start_controller(&mut self, _target: &str, _pairing_code: Option<&str>) -> Result<(), RdcError> {
        // TODO(stage2): 接入 rdc-connect 的信令与打洞流程。
        todo!("S1 空壳：主控端连接流程在 stage2 实现")
    }

    /// 被控端开始注册。
    ///
    /// # Returns
    /// 成功时状态推进到 `Registered`。
    ///
    /// # Errors
    /// 状态不是 `Idle` 时返回 [`RdcError`]（`InvalidState`）。
    pub fn start_host(&mut self) -> Result<(), RdcError> {
        // TODO(stage2): 接入 rdc-signal 的注册流程。
        todo!("S1 空壳：被控端注册流程在 stage2 实现")
    }

    /// 关闭会话（幂等）。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败；已处于 `Closing`/`Failed` 时直接返回。
    pub fn stop(&mut self) {
        // 幂等：重复调用不应改变状态，避免 Dart 侧 dispose 与错误路径重复触发。
        if matches!(self.state, SessionState::Closing | SessionState::Failed) {
            return;
        }
        self.state = SessionState::Closing;
    }
}
