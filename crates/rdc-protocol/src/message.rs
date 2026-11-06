//! rdc-protocol / 信令信封与消息体
//!
//! 层级：L2 能力层
//! 用途：信令报文（QUIC 控制流）的唯一真源：公共信封、消息清单、错误码、
//!       候选描述与 4 KB 单包上限（docs/03 §2）。
//! 调用方：rdc-connect::signal_client（客户端编解码）、rdc-signal（服务端分派）、
//!         rdc-cli（Demo）
//!
//! 线格式：信封字段 `v/type/id/seq/ts/sig` 与消息体字段摊平在同一层 JSON 对象里
//! （`type` 由 serde 内部标签产生）。`sig` 的签名/校验对象是
//! `{type,id,seq,ts,body}`，其中 `body` 是去掉 `type` 之后的所有业务字段。
//! 该规范化方式记录在 docs/决策待办.md（文档未给出逐字节示例）。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::capabilities::Capabilities;
use crate::error::ProtocolError;
use crate::version::SIGNAL_VERSION;

// =====** 协议常量 =====**

/// 信令单包上限（JSON 字节），超过直接拒绝，避免服务端被大包拖垮（docs/03 §1）。
pub const MAX_SIGNAL_PACKET: usize = 4096;

/// 信令请求超时（毫秒）：1 s 未回则重发，最多 3 次。
pub const SIGNAL_REQUEST_TIMEOUT_MS: u64 = 1000;

/// 信令请求最大重发次数（不含首次）。
pub const SIGNAL_MAX_RETRIES: u8 = 3;

/// 信令 keepalive 周期（毫秒）。
pub const SIGNAL_KEEPALIVE_INTERVAL_MS: u64 = 30_000;

/// 信令离线判定（毫秒）：3 次 keepalive 未收。
pub const SIGNAL_KEEPALIVE_TIMEOUT_MS: u64 = 90_000;

/// 会话心跳周期（毫秒）。
pub const SESSION_HEARTBEAT_INTERVAL_MS: u64 = 3_000;

/// 会话心跳超时（毫秒）。
pub const SESSION_HEARTBEAT_TIMEOUT_MS: u64 = 9_000;

/// 媒体心跳周期（毫秒）。
pub const MEDIA_HEARTBEAT_INTERVAL_MS: u64 = 3_000;

/// 媒体心跳超时（毫秒）。
pub const MEDIA_HEARTBEAT_TIMEOUT_MS: u64 = 9_000;

/// 呼叫等待被接受超时（毫秒）。
pub const CALL_TIMEOUT_MS: u64 = 30_000;

/// 候选交换超时（毫秒）。
pub const CANDIDATE_TIMEOUT_MS: u64 = 10_000;

/// 握手超时（毫秒），失败重试 1 次。
pub const HANDSHAKE_TIMEOUT_MS: u64 = 5_000;

// =====** 辅助枚举 =====**

/// 呼叫模式：一次性配对码或已信任设备免密。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CallMode {
    /// 配对码模式：PAKE 口令来自配对码。
    Code,
    /// 免密模式：双方已配对，用挑战-应答验签。
    Trusted,
}

/// 打洞角色（`punch` 报文）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PunchRole {
    /// 主控端。
    Controller,
    /// 被控端。
    Host,
}

/// 最终路径（`connected` 报文）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalPath {
    /// IPv6 直连。
    Ipv6,
    /// UDP 打洞。
    Udp,
    /// TCP 打洞。
    Tcp,
    /// 中继转发。
    Relay,
}

/// 候选地址的传输协议。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateProto {
    /// IPv6 直连候选。
    Ipv6,
    /// UDP 打洞候选（也是回环 Demo 的 QUIC 直连候选）。
    Udp,
    /// TCP 打洞候选。
    Tcp,
}

/// 单个候选地址描述（随 `candidates` 报文传输）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CandidateDesc {
    /// `ip:port` 形式的地址。
    pub addr: String,
    /// 传输协议，决定尝试顺序。
    pub proto: CandidateProto,
    /// 优先级：数值越小越先尝试（0 最高）。
    pub priority: u8,
}

/// 服务端错误码。
///
/// 线上是字符串（docs/03 §2.3 的下划线命名），新增只能追加。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignalErrorCode {
    /// 目标未注册或已过期。
    PeerNotFound,
    /// 协议版本不一致。
    VersionMismatch,
    /// 报文非法。
    BadRequest,
    /// 触发限流。
    RateLimited,
    /// 会话窗口超时。
    SessionExpired,
    /// 请求中继但服务端未启用中继。
    RelayDisabled,
    /// 中继配额不足。
    RelayQuota,
    /// 签名 / 配对码校验失败。
    AuthFailed,
    /// 未分类服务端错误。
    Internal,
}

// =====** SignalMessage =====**

/// 信令消息体。
///
/// `#[serde(tag = "type")]` 把枚举标签摊平进信封对象；字段名即线上字段名，
/// 重命名等同协议破坏性变更，必须同步 docs/03。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SignalMessage {
    /// C→S：注册/上线。
    Register {
        /// 本端身份公钥（Base64，Ed25519）。
        pub_key: String,
        /// 可读设备名（仅用于 UI 展示，不参与鉴权）。
        device_name: String,
        /// 平台标识（windows/macos/linux/android/ios）。
        platform: String,
        /// 本端能力位图。
        caps: Capabilities,
    },
    /// S→C：注册成功，返回观测地址与会话 token。
    RegisterOk {
        /// 服务端观测到的本端地址 `ip:port`（STUN 结果）。
        observed: String,
        /// 会话 token（HMAC 密钥），只在本连接内存中存在。
        token: String,
        /// token 有效期（秒）。
        ttl: u32,
    },
    /// C→S：刷新在线状态（幂等）。
    Keepalive,
    /// C→S：主控端发起呼叫。
    Call {
        /// 目标设备 ID。
        target: String,
        /// 是否允许回退中继。
        allow_relay: bool,
        /// 请求的权限位（docs/07 §4 的 u16 位图）。
        policy: u16,
        /// HMAC(device_key, session || code_hash)；服务端只透传不校验。
        code_proof: String,
        /// 呼叫模式。
        mode: CallMode,
        /// 免密模式：主控端 32 字节随机挑战（Base64）。
        #[serde(default)]
        chal_c: Option<String>,
        /// 免密模式：主控端毫秒时间戳。
        #[serde(default)]
        ts_c: Option<i64>,
        /// 免密模式：对 `RDC-AUTH‖session‖chal_c‖ts_c` 的签名（Base64）。
        #[serde(default)]
        sig_c: Option<String>,
    },
    /// S→H：通知被控端有人呼叫。
    Incoming {
        /// 主控端 peer ID。
        peer_id: String,
        /// 主控端身份公钥（Base64），用于被控端验签。
        peer_pub: String,
        /// 服务端分配的会话 ID。
        session: String,
        /// 主控端是否允许回退中继。
        allow_relay: bool,
        /// 呼叫模式（透传）。
        mode: CallMode,
        /// 透传的挑战（免密模式）。
        #[serde(default)]
        chal_c: Option<String>,
        /// 透传的时间戳（免密模式）。
        #[serde(default)]
        ts_c: Option<i64>,
        /// 透传的签名（免密模式）。
        #[serde(default)]
        sig_c: Option<String>,
    },
    /// H→S→C：被控端接受会话。
    Accept {
        /// 会话 ID。
        session: String,
        /// 免密模式：被控端 32 字节随机挑战（Base64）。
        #[serde(default)]
        chal_h: Option<String>,
        /// 免密模式：被控端毫秒时间戳。
        #[serde(default)]
        ts_h: Option<i64>,
        /// 免密模式：被控端签名（Base64）。
        #[serde(default)]
        sig_h: Option<String>,
    },
    /// H→S→C：拒绝会话。
    Refuse {
        /// 会话 ID。
        session: String,
        /// 拒绝原因（面向用户的短句，不含敏感信息）。
        reason: String,
    },
    /// 双向经 S：交换候选地址。
    Candidates {
        /// 会话 ID。
        session: String,
        /// 候选列表，按优先级排列。
        list: Vec<CandidateDesc>,
    },
    /// C→S：申请中继票据（仅当用户开启中继）。
    RelayRequest {
        /// 会话 ID。
        session: String,
        /// 配对码证明（服务端透传）。
        code_proof: String,
    },
    /// S→C：中继票据。
    RelayTicket {
        /// 中继地址 `ip:port`。
        relay_addr: String,
        /// 票据（限会话、端口、带宽）。
        ticket: String,
        /// 过期时间（毫秒时间戳）。
        expires: i64,
    },
    /// S→C/H：通知双方立刻向对端发包（同时打洞）。
    Punch {
        /// 对端地址 `ip:port`。
        peer_addr: String,
        /// 会话 ID。
        session: String,
        /// 本端在这次打洞中的角色。
        role: PunchRole,
    },
    /// C/H→S：上报最终路径，服务端仅记录统计。
    Connected {
        /// 会话 ID。
        session: String,
        /// 最终路径。
        path: SignalPath,
    },
    /// 双向：结束会话（幂等）。
    Bye {
        /// 会话 ID。
        session: String,
        /// 结束原因。
        reason: String,
    },
    /// S→C：服务端错误。
    Error {
        /// 错误码。
        code: SignalErrorCode,
        /// 面向用户的说明。
        message: String,
    },
    /// C/H：媒体握手完成后的验签结果（仅用于 UI 与审计）。
    AuthResult {
        /// 会话 ID。
        session: String,
        /// 是否通过。
        ok: bool,
        /// 失败原因（通过时为空）。
        #[serde(default)]
        reason: Option<String>,
    },
}

impl SignalMessage {
    /// 返回线上 `type` 字符串。
    ///
    /// # Returns
    /// 与 serde 标签一致的下划线字符串。
    ///
    /// # Errors
    /// 不会失败。
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Register { .. } => "register",
            Self::RegisterOk { .. } => "register_ok",
            Self::Keepalive => "keepalive",
            Self::Call { .. } => "call",
            Self::Incoming { .. } => "incoming",
            Self::Accept { .. } => "accept",
            Self::Refuse { .. } => "refuse",
            Self::Candidates { .. } => "candidates",
            Self::RelayRequest { .. } => "relay_request",
            Self::RelayTicket { .. } => "relay_ticket",
            Self::Punch { .. } => "punch",
            Self::Connected { .. } => "connected",
            Self::Bye { .. } => "bye",
            Self::Error { .. } => "error",
            Self::AuthResult { .. } => "auth_result",
        }
    }

    /// 取出会话 ID（无会话的消息返回 `None`）。
    ///
    /// # Returns
    /// 会话类消息返回 `Some(&str)`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn session(&self) -> Option<&str> {
        match self {
            Self::Incoming { session, .. }
            | Self::Accept { session, .. }
            | Self::Refuse { session, .. }
            | Self::Candidates { session, .. }
            | Self::RelayRequest { session, .. }
            | Self::Punch { session, .. }
            | Self::Connected { session, .. }
            | Self::Bye { session, .. }
            | Self::AuthResult { session, .. } => Some(session),
            _ => None,
        }
    }
}

// =====** SignalEnvelope =====**

/// 信令公共信封。
//[字段名就是线上字段名] `v` 用 u8：docs/03 §2.1 的示例为 `1`，不是 `65536`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignalEnvelope {
    /// 协议版本（当前为 [`SIGNAL_VERSION`]）。
    pub v: u8,
    /// 发送方 peer ID。
    pub id: String,
    /// 单调递增序号（服务端按 peer 记录窗口，防重放）。
    pub seq: u64,
    /// 客户端毫秒时间戳（仅观测，不参与鉴权）。
    pub ts: i64,
    /// 签名 / HMAC（Base64）；注册报文用身份私钥签名，其余用会话 token 做 HMAC。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sig: Option<String>,
    /// 消息体（摊平到同一层 JSON）。
    #[serde(flatten)]
    pub body: SignalMessage,
}

impl SignalEnvelope {
    /// 构造未签名的信封。
    ///
    /// # Arguments
    /// * `id` - 发送方 peer ID
    /// * `seq` - 序号
    /// * `ts` - 毫秒时间戳
    /// * `body` - 消息体
    ///
    /// # Returns
    /// `v` 为当前协议版本、`sig` 为空的信封。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new(id: impl Into<String>, seq: u64, ts: i64, body: SignalMessage) -> Self {
        Self {
            v: SIGNAL_VERSION,
            id: id.into(),
            seq,
            ts,
            sig: None,
            body,
        }
    }

    /// 附加签名/HMAC 后返回自身（链式构造）。
    ///
    /// # Arguments
    /// * `sig` - Base64 签名或 HMAC
    ///
    /// # Returns
    /// 带 `sig` 的信封。
    ///
    /// # Errors
    /// 不会失败。
    pub fn signed(mut self, sig: impl Into<String>) -> Self {
        self.sig = Some(sig.into());
        self
    }

    /// 生成签名/HMAC 的规范化字节。
    ///
    /// 内容为 `{"type":...,"id":...,"seq":...,"ts":...,"body":{...}}`；
    /// `v` 与 `sig` 不参与，`body` 去掉重复的 `type` 字段。
    /// JSON 对象键由 serde_json（BTreeMap）排序，保证两端逐字节一致。
    ///
    /// # Returns
    /// 用于 Ed25519 签名或 HMAC-SHA256 的字节序列。
    ///
    /// # Errors
    /// 消息体无法序列化时返回 [`ProtocolError::Serialize`]。
    pub fn canonical_auth_bytes(&self) -> Result<Vec<u8>, ProtocolError> {
        // 先序列化整个信封：`sig` 为 None 时被 skip，剩下 v/type/id/seq/ts/业务字段。
        let envelope = serde_json::to_value(self)?;
        let mut obj = envelope
            .as_object()
            .cloned()
            .ok_or(ProtocolError::InvalidField("envelope 必须是对象"))?;
        obj.remove("v");
        obj.remove("sig");
        let type_name = obj
            .remove("type")
            .and_then(|v| v.as_str().map(str::to_owned))
            .ok_or(ProtocolError::InvalidField("缺少 type"))?;
        let id = obj
            .remove("id")
            .ok_or(ProtocolError::InvalidField("缺少 id"))?;
        let seq = obj
            .remove("seq")
            .ok_or(ProtocolError::InvalidField("缺少 seq"))?;
        let ts = obj
            .remove("ts")
            .ok_or(ProtocolError::InvalidField("缺少 ts"))?;

        // 根对象保持固定字段顺序：type,id,seq,ts,body。serde_json 的 Map 是 BTreeMap，
        // 序列化时会按键名排序，因此结果与插入顺序无关，天然确定。
        let mut root = Map::new();
        root.insert("type".to_owned(), Value::String(type_name));
        root.insert("id".to_owned(), id);
        root.insert("seq".to_owned(), seq);
        root.insert("ts".to_owned(), ts);
        root.insert("body".to_owned(), Value::Object(obj));
        Ok(serde_json::to_vec(&Value::Object(root))?)
    }

    /// 序列化为线上字节，并校验 4 KB 上限。
    ///
    /// # Returns
    /// UTF-8 JSON 字节序列。
    ///
    /// # Errors
    /// 序列化失败返回 [`ProtocolError::Serialize`]；超过 [`MAX_SIGNAL_PACKET`]
    /// 返回 [`ProtocolError::SignalPacketTooLarge`]。
    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        let bytes = serde_json::to_vec(self)?;
        if bytes.len() > MAX_SIGNAL_PACKET {
            return Err(ProtocolError::SignalPacketTooLarge {
                actual: bytes.len(),
                max: MAX_SIGNAL_PACKET,
            });
        }
        Ok(bytes)
    }

    /// 从线上字节解析信封。
    ///
    /// # Arguments
    /// * `bytes` - QUIC 流里读到的一包 JSON
    ///
    /// # Returns
    /// 解析后的信封（`sig` 是否存在由调用方按消息类型判断）。
    ///
    /// # Errors
    /// 超长返回 [`ProtocolError::SignalPacketTooLarge`]；JSON 非法返回
    /// [`ProtocolError::Serialize`]；版本不兼容返回
    /// [`ProtocolError::SignalVersionUnsupported`]。
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.len() > MAX_SIGNAL_PACKET {
            return Err(ProtocolError::SignalPacketTooLarge {
                actual: bytes.len(),
                max: MAX_SIGNAL_PACKET,
            });
        }
        // `from_slice` 对尾随垃圾字节也会报错，避免把两个包拼在一起的情况放过。
        let env: Self = serde_json::from_slice(bytes)?;
        if env.v != SIGNAL_VERSION {
            return Err(ProtocolError::SignalVersionUnsupported {
                peer: env.v,
                local: SIGNAL_VERSION,
            });
        }
        Ok(env)
    }
}

// =====** 单元测试：冻结线格式与认证规范化 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一个注册信封用于多处断言。
    fn register_env() -> SignalEnvelope {
        SignalEnvelope::new(
            "host-01",
            7,
            1_700_000_000_000,
            SignalMessage::Register {
                pub_key: "AAAA".to_owned(),
                device_name: "demo".to_owned(),
                platform: "windows".to_owned(),
                caps: Capabilities::DEFAULT,
            },
        )
    }

    /// 信封字段名必须与 docs/03 §2.1 完全一致，且消息体摊平。
    #[test]
    fn envelope_wire_shape_is_frozen() {
        let mut env = register_env();
        env.sig = Some("sig".to_owned());
        let bytes = env.encode().expect("编码");
        let value: Value = serde_json::from_slice(&bytes).expect("JSON");
        let obj = value.as_object().expect("对象");
        for key in ["v", "type", "id", "seq", "ts", "sig", "pub_key", "device_name", "platform", "caps"] {
            assert!(obj.contains_key(key), "缺少字段 {key}");
        }
        assert_eq!(obj["v"], SIGNAL_VERSION);
        assert_eq!(obj["type"], "register");
        assert_eq!(obj["id"], "host-01");
    }

    /// 规范化字节只覆盖 {type,id,seq,ts,body}，改 sig/v 不影响认证内容。
    #[test]
    fn canonical_bytes_exclude_sig_and_version() {
        let base = register_env();
        let mut signed = base.clone();
        signed.sig = Some("abc".to_owned());
        signed.v = SIGNAL_VERSION;
        assert_eq!(
            base.canonical_auth_bytes().expect("规范化"),
            signed.canonical_auth_bytes().expect("规范化")
        );
        let text = String::from_utf8(base.canonical_auth_bytes().expect("规范化")).expect("UTF-8");
        assert!(text.contains("\"body\""));
        assert!(text.contains("\"type\":\"register\""));
        assert!(!text.contains("\"sig\""));
        assert!(!text.contains("\"v\":"));
    }

    /// 任何业务字段变化都必须改变规范化字节（否则可被篡改）。
    #[test]
    fn canonical_bytes_change_with_body() {
        let a = register_env();
        let mut b = register_env();
        if let SignalMessage::Register { device_name, .. } = &mut b.body {
            device_name.push('x');
        }
        assert_ne!(
            a.canonical_auth_bytes().expect("规范化"),
            b.canonical_auth_bytes().expect("规范化")
        );
    }

    /// 超过 4 KB 的信令包必须在编码期拒绝。
    #[test]
    fn oversized_signal_packet_is_rejected() {
        let big = "x".repeat(MAX_SIGNAL_PACKET);
        let env = SignalEnvelope::new(
            "id",
            1,
            1,
            SignalMessage::Register {
                pub_key: big,
                device_name: "d".to_owned(),
                platform: "p".to_owned(),
                caps: Capabilities::NONE,
            },
        );
        let err = env.encode().expect_err("必须拒绝");
        assert!(matches!(err, ProtocolError::SignalPacketTooLarge { .. }));
    }

    /// 版本不兼容必须显式报错，不能静默继续。
    #[test]
    fn version_mismatch_is_rejected() {
        let mut bytes = register_env().encode().expect("编码");
        // 直接把 JSON 里的 "v":1 改成 2；长度不变，避免重新缩进影响断言。
        let text = String::from_utf8(bytes.clone()).expect("UTF-8");
        bytes = text.replace("\"v\":1", "\"v\":2").into_bytes();
        let err = SignalEnvelope::decode(&bytes).expect_err("版本不兼容");
        assert!(matches!(err, ProtocolError::SignalVersionUnsupported { .. }));
    }

    /// 候选描述与错误码的下划线命名是线上协议。
    #[test]
    fn error_code_and_candidates_wire_shape() {
        let env = SignalEnvelope::new(
            "srv",
            1,
            1,
            SignalMessage::Error {
                code: SignalErrorCode::PeerNotFound,
                message: "offline".to_owned(),
            },
        );
        let bytes = env.encode().expect("编码");
        let text = String::from_utf8(bytes).expect("UTF-8");
        assert!(text.contains("\"code\":\"peer_not_found\""));
        assert!(text.contains("\"type\":\"error\""));

        let cand = CandidateDesc {
            addr: "127.0.0.1:9000".to_owned(),
            proto: CandidateProto::Udp,
            priority: 0,
        };
        let json = serde_json::to_string(&cand).expect("序列化");
        assert!(json.contains("\"proto\":\"udp\""));
    }
}
