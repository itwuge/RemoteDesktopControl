//! rdc-connect / 候选地址
//!
//! 层级：L2 能力层
//! 用途：描述可连接的候选地址及其来源、信令观测到的公网地址；类型可序列化，
//!       以便随信令报文的 candidates 字段传输
//! 调用方：rdc-connect 的 punch / signal_client、rdc-core（会话建立）

use serde::{Deserialize, Serialize};

// =====** CandidateKind =====**

/// 候选地址来源。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CandidateKind {
    /// 本机地址（局域网直连）。
    Host,
    /// 经 STUN 观测到的公网地址。
    Srflx,
    /// 中继服务器地址。
    Relay,
}

// =====** Candidate =====**

/// 单个候选地址。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    /// `ip:port` 形式的地址字符串。
    pub addr: String,
    /// 地址来源，决定打洞优先级与回退顺序。
    pub kind: CandidateKind,
}

// =====** ObservedAddr =====**

/// 信令服务观测到的本端公网地址。
///
/// 为什么单独建模：它是服务端视角的地址，与本端自测的候选不能混用，
/// 否则会误导打洞方连接错误的目标。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ObservedAddr {
    /// `ip:port` 形式，供本端填入候选列表。
    pub addr: String,
}
