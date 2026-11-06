//! rdc-signal / 服务端会话状态机
//!
//! 层级：L1 服务端
//! 用途：定义服务端视角的会话生命周期与合法迁移，任何非法迁移都必须拒绝，
//!       防止重复 accept、已关闭会话被复活（docs/03 §3.8）。
//! 调用方：rdc-signal::server（每次收到 accept/refuse/connected/bye 时调用）

// =====** SessionState =====**

/// 服务端会话状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// 主控端已呼叫，等待被控端应答。
    Calling,
    /// 被控端已接受，等待打洞/握手。
    Accepted,
    /// 任一端上报 connected（仅用于统计）。
    Connected,
    /// 已正常结束。
    Closed,
    /// 超时或异常结束。
    Failed,
}

/// 会话事件。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionEvent {
    /// 被控端接受。
    Accept,
    /// 被控端拒绝。
    Refuse,
    /// 任一端上报 connected。
    Connected,
    /// 任一端发送 bye。
    Bye,
    /// 呼叫/候选等待超时。
    Timeout,
}

/// 状态迁移错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("会话状态 {from:?} 不接受事件 {event:?}")]
pub struct SessionTransitionError {
    /// 当前状态。
    pub from: SessionState,
    /// 触发事件。
    pub event: SessionEvent,
}

/// 计算迁移后的状态。
///
/// 规则（docs/03 §3.8）：
/// - Calling 只能 Accept/Refuse/Timeout；
/// - Accepted 只能 Connected/Bye/Timeout；
/// - Connected 只能 Bye；
/// - Closed/Failed 为终态，任何事件都拒绝。
///
/// # Arguments
/// * `from` - 当前状态
/// * `event` - 发生的事件
///
/// # Returns
/// 新状态。
///
/// # Errors
/// 非法迁移返回 [`SessionTransitionError`]。
pub fn transition(from: SessionState, event: SessionEvent) -> Result<SessionState, SessionTransitionError> {
    use SessionEvent as E;
    use SessionState as S;
    let next = match (from, event) {
        (S::Calling, E::Accept) => S::Accepted,
        (S::Calling, E::Refuse) => S::Closed,
        (S::Calling, E::Timeout) => S::Failed,
        // Accepted 阶段任一端直接 bye（例如用户取消）视为正常关闭。
        (S::Accepted, E::Connected) => S::Connected,
        (S::Accepted, E::Bye) => S::Closed,
        (S::Accepted, E::Timeout) => S::Failed,
        (S::Connected, E::Bye) => S::Closed,
        // 重复上报 connected 是幂等的：网络抖动可能导致重发。
        (S::Connected, E::Connected) => S::Connected,
        _ => {
            return Err(SessionTransitionError { from, event });
        }
    };
    Ok(next)
}

/// 是否为终态。
///
/// # Arguments
/// * `state` - 待判断状态
///
/// # Returns
/// `Closed`/`Failed` 返回 `true`。
///
/// # Errors
/// 不会失败。
pub fn is_terminal(state: SessionState) -> bool {
    matches!(state, SessionState::Closed | SessionState::Failed)
}

// =====** 单元测试：状态机转换 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 正常路径：Calling → Accepted → Connected → Closed。
    #[test]
    fn happy_path() {
        let s = transition(SessionState::Calling, SessionEvent::Accept).expect("accept");
        assert_eq!(s, SessionState::Accepted);
        let s = transition(s, SessionEvent::Connected).expect("connected");
        assert_eq!(s, SessionState::Connected);
        let s = transition(s, SessionEvent::Bye).expect("bye");
        assert_eq!(s, SessionState::Closed);
        assert!(is_terminal(s));
    }

    /// 拒绝直接关闭；超时进入 Failed。
    #[test]
    fn refuse_and_timeout() {
        assert_eq!(
            transition(SessionState::Calling, SessionEvent::Refuse).expect("refuse"),
            SessionState::Closed
        );
        assert_eq!(
            transition(SessionState::Calling, SessionEvent::Timeout).expect("timeout"),
            SessionState::Failed
        );
    }

    /// 终态不可复活：任何事件都必须拒绝。
    #[test]
    fn terminal_states_are_final() {
        for from in [SessionState::Closed, SessionState::Failed] {
            for event in [
                SessionEvent::Accept,
                SessionEvent::Refuse,
                SessionEvent::Connected,
                SessionEvent::Bye,
                SessionEvent::Timeout,
            ] {
                assert!(transition(from, event).is_err(), "{from:?} + {event:?}");
            }
        }
    }

    /// 未接受就 connected / 未接受就 bye 都属于非法迁移。
    #[test]
    fn premature_events_are_rejected() {
        assert!(transition(SessionState::Calling, SessionEvent::Connected).is_err());
        assert!(transition(SessionState::Calling, SessionEvent::Bye).is_err());
    }
}
