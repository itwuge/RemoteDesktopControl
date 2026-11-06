//! rdc-protocol / 信令报文与能力位
//!
//! 层级：L2 能力层
//! 用途：信令信封、消息体、版本协商能力位图的唯一真源（docs/03 信令协议）
//! 调用方：rdc-connect::signal_client、rdc-signal 服务端、rdc-bridge（少量状态上报）

use serde::{Deserialize, Serialize};

use crate::version::PROTOCOL_VERSION;

// =====** Capabilities 能力位图 =====**

/// 版本协商能力位图。
///
/// 用 `u32` 位图而不是布尔字段列表：新增能力只占用一个未用位，老客户端读到
/// 未定义的位直接忽略，天然向后兼容（docs/03 §协议冻结）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities(pub u32);

impl Capabilities {
    /// HEVC 硬编能力。
    pub const HEVC: u32 = 1 << 0;
    /// H.264 硬编能力。
    pub const H264: u32 = 1 << 1;
    /// 4:4:4 色度支持。
    pub const CHROMA_444: u32 = 1 << 2;
    /// 10-bit 位深支持。
    pub const BIT_DEPTH_10: u32 = 1 << 3;
    /// 中继可用（仅在用户显式开启时置位）。
    pub const RELAY: u32 = 1 << 4;
    /// 免密挑战-应答登录能力（S2 起使用，S1 仅占位）。
    pub const PASSWORDLESS: u32 = 1 << 5;

    /// 判断某能力位是否置位。
    ///
    /// # Arguments
    /// * `mask` - 上述常量之一或其组合
    ///
    /// # Returns
    /// 置位返回 `true`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn contains(self, mask: u32) -> bool {
        self.0 & mask != 0
    }
}

// =====** SignalEnvelope 信封 =====**

/// 信令报文的统一信封。
///
/// 所有消息都带版本号与消息体；服务端先校验主版本，再分派消息体。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignalEnvelope {
    /// 协议版本（`major << 16 | minor`），见 [`PROTOCOL_VERSION`]。
    pub version: u32,
    /// 能力位图，双方在注册/呼叫阶段交换。
    #[serde(default)]
    pub capabilities: Capabilities,
    /// 消息体。
    pub message: SignalMessage,
}

impl SignalEnvelope {
    /// 用当前协议版本构造信封。
    ///
    /// # Arguments
    /// * `message` - 消息体
    /// * `capabilities` - 本端能力位图
    ///
    /// # Returns
    /// 可直接序列化发送的信令信封。
    ///
    /// # Errors
    /// 不会失败（纯构造）。
    pub fn new(message: SignalMessage, capabilities: Capabilities) -> Self {
        Self {
            version: PROTOCOL_VERSION,
            capabilities,
            message,
        }
    }
}

// =====** SignalMessage 消息体 =====**

/// 信令消息体。
///
/// 字段名即线上字段名；重命名等同协议破坏性变更，必须同步 docs/03。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SignalMessage {
    /// 被控端注册到会合服务器。
    Register {
        /// 设备 ID（公钥指纹派生）。
        peer_id: String,
        /// 公钥（base64）。
        public_key: String,
    },
    /// 注册成功，返回服务器观测到的地址（STUN 结果）。
    RegisterOk {
        /// 服务器观测地址，形如 `ip:port`。
        observed_addr: String,
    },
    /// 主控端发起连接请求。
    Call {
        /// 目标设备 ID。
        target: String,
        /// 配对码（免密登录时为空）。
        pairing_code: Option<String>,
        /// 是否允许中继；由设置页决定，默认 `false`（docs/01 C2）。
        allow_relay: bool,
    },
    /// 被控端收到来电。
    Incoming {
        /// 会话 ID。
        session_id: String,
        /// 主控端设备 ID。
        from: String,
    },
    /// 被控端接受会话。
    Accept {
        /// 会话 ID。
        session_id: String,
    },
    /// 被控端拒绝会话。
    Refuse {
        /// 会话 ID。
        session_id: String,
        /// 拒绝原因（面向用户的短句）。
        reason: String,
    },
    /// 候选地址交换。
    Candidates {
        /// 会话 ID。
        session_id: String,
        /// 候选地址列表，形如 `ip:port`。
        candidates: Vec<String>,
    },
    /// 申请中继票据（仅在 `allow_relay=true` 时可申请）。
    RelayTicket {
        /// 会话 ID。
        session_id: String,
    },
    /// 结束会话。
    Bye {
        /// 会话 ID。
        session_id: String,
    },
}
