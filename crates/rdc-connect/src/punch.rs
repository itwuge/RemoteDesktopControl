//! rdc-connect / 打洞策略与结果
//!
//! 层级：L2 能力层
//! 用途：定义打洞优先级策略、最终选用的传输路径与失败原因；S1 只保留类型与接口
//! 调用方：rdc-core 的会话建立流程（依次尝试候选并选择路径）

use crate::{Candidate, ConnectError};

// =====** HolePunchStrategy =====**

/// 打洞策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HolePunchStrategy {
    /// 优先 IPv6 直连，失败才尝试 IPv4 打洞。
    Ipv6First,
    /// 禁用 UDP，仅走 TCP 打洞。
    DisableUdp,
    /// 强制 TCP 路径（不尝试 UDP 打洞）。
    ForceTcp,
}

// =====** PunchPath =====**

/// 最终建立的传输路径。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PunchPath {
    /// IPv6 直连成功。
    Ipv6Direct,
    /// UDP 打洞成功。
    UdpPunch,
    /// TCP 打洞成功。
    TcpPunch,
    /// 回退到中继转发。
    Relay,
}

// =====** PunchOutcome =====**

/// 打洞结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PunchOutcome {
    /// 实际使用的路径。
    pub path: PunchPath,
    /// 建链后测得的 RTT（毫秒），供选路与日志使用。
    pub rtt_ms: u32,
}

// =====** punch =====**

/// 按策略依次尝试候选地址并返回可用路径。
///
/// # Arguments
/// * `strategy` - 打洞策略，决定候选优先级与是否允许回退中继
/// * `candidates` - 双方交换后的候选地址列表（按优先级排列）
///
/// # Returns
/// 命中路径及该路径的 RTT。
///
/// # Errors
/// 所有候选均不可达时返回 [`ConnectError::P2pFailed`]；策略禁止 UDP/TCP 对应路径时
/// 返回 [`ConnectError::RelayDisabled`] 或 [`ConnectError::InvalidArgument`]。
///
/// # Panics
/// TODO(stage2)：S1 空壳，实现落地前调用会 panic。
pub async fn punch(
    _strategy: HolePunchStrategy,
    _candidates: &[Candidate],
) -> Result<PunchOutcome, ConnectError> {
    todo!("TODO(stage2)：打洞流程在 S2 实现")
}
