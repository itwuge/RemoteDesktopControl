//! rdc-security / 握手状态机
//!
//! 层级：L2 能力层
//! 用途：定义握手角色、握手状态与单步推进接口；S1 只实现状态置位与查询
//! 调用方：rdc-connect（P2P 建链后的安全握手）、rdc-core（会话建立编排）

use crate::error::SecurityError;

// =====** HandshakeRole =====**

/// 握手角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeRole {
    /// 被控端（Host），负责先发 challenge。
    Host,
    /// 控制端（Controller），负责校验 challenge 并回应。
    Controller,
}

// =====** HandshakeState =====**

/// 握手状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeState {
    /// 尚未交换任何报文。
    Idle,
    /// Host 已发出 challenge，等待对端回应。
    ChallengeSent,
    /// Controller 已收到 challenge，尚未回应。
    ChallengeReceived,
    /// 双方已确认会话密钥，可以开始传输。
    Established,
    /// 握手失败，会话必须关闭且不可重试。
    Failed,
}

// =====** Handshake =====**

/// 单次会话的握手状态机。
#[derive(Debug, Clone)]
pub struct Handshake {
    /// 本端角色，决定 challenge 的发起方向。
    role: HandshakeRole,
    /// 当前状态，由 `step` 推进。
    state: HandshakeState,
}

impl Handshake {
    /// 创建处于 `Idle` 的握手状态机。
    ///
    /// # Arguments
    /// * `role` - 本端角色
    ///
    /// # Returns
    /// 初始状态为 [`HandshakeState::Idle`] 的实例。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new(role: HandshakeRole) -> Self {
        Self {
            role,
            state: HandshakeState::Idle,
        }
    }

    /// 返回当前握手状态。
    ///
    /// # Returns
    /// 当前状态的拷贝。
    ///
    /// # Errors
    /// 不会失败。
    pub fn state(&self) -> HandshakeState {
        self.state
    }

    /// 推进一次握手。
    ///
    /// # Arguments
    /// * `peer_message` - 对端上一步的握手报文（角色为 Host 的首步传空切片）
    ///
    /// # Returns
    /// 需要回给对端的报文；已建立且无后续报文时返回空 `Vec`。
    ///
    /// # Errors
    /// 当前状态不接受该报文时返回 [`SecurityError::HandshakeStateInvalid`]；
    /// 验签失败返回 [`SecurityError::SignatureInvalid`]；报文无法解析时返回
    /// [`SecurityError::Protocol`]。
    ///
    /// # Panics
    /// S1 空壳：实现落地前调用会 panic。
    pub fn step(&mut self, _peer_message: &[u8]) -> Result<Vec<u8>, SecurityError> {
        todo!("S1 空壳：握手报文处理在 S2 实现")
    }

    /// 是否已完成握手。
    ///
    /// # Returns
    /// 状态为 [`HandshakeState::Established`] 时返回 `true`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn is_established(&self) -> bool {
        matches!(self.state, HandshakeState::Established)
    }
}
