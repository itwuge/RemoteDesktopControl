//! rdc-security / 4 报文握手状态机（HELLO / HELLO_ACK / AUTH / AUTH_OK）
//!
//! 层级：L2 能力层
//! 用途：实现 docs/07 §2 的会话握手：X25519 + SPAKE2(配对码) → HKDF 分通道密钥，
//!       再以 confirm 密钥做 HMAC 确认；同时提供免密挑战-应答的签名/验签。
//! 调用方：rdc-connect（P2P 直连后的安全握手）、rdc-cli（Demo）
//!
//! 安全前提：任何一步校验失败都进入 `Failed` 且不可重试；失败原因对外统一为
//! “认证失败”，不区分码错/过期，避免侧信道。

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use hmac::{Hmac, Mac};
use rand_core::{OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::Sha256;

use rdc_protocol::challenge::{auth_signing_payload, validate_auth_time, CHALLENGE_LEN};

use crate::error::SecurityError;
use crate::keys::{
    derive_channel_keys, derive_sas, from_b64_exact, verify_signature_b64, ChannelKeys,
    EphemeralKey, IdentityKeyPair,
};
use crate::pake::{PakeRole, PakeSession, PairingCode};

// =====** 类型 =====**

/// HMAC-SHA256 类型别名。
type HmacSha256 = Hmac<Sha256>;

/// 握手消息（JSON 承载，随控制通道传输）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum HandshakeMessage {
    /// 主控端发起：临时公钥、身份公钥、签名、PAKE 消息。
    Hello {
        /// X25519 临时公钥（Base64）。
        eph: String,
        /// 发送方 peer ID。
        id: String,
        /// 发送方 Ed25519 身份公钥（Base64）。
        identity: String,
        /// 毫秒时间戳（端点校验 ±30 s）。
        ts: i64,
        /// 对 `RDC-AUTH‖session‖eph‖ts` 的签名（Base64）。
        sig: String,
        /// SPAKE2 消息（Base64）。
        pake: String,
        /// 本端能力位图（阶段 5 追加；旧端缺省为 `None`）。
        ///
        /// 不进入签名范围：握手运行在 TLS 1.3 + SPAKE2 之上，通道已被认证；
        /// 对端谎报自己的能力只会让双方少启用功能，不能放大权限。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caps: Option<u64>,
    },
    /// 被控端回应：结构同 HELLO。
    HelloAck {
        /// X25519 临时公钥（Base64）。
        eph: String,
        /// 发送方 peer ID。
        id: String,
        /// 发送方 Ed25519 身份公钥（Base64）。
        identity: String,
        /// 毫秒时间戳。
        ts: i64,
        /// 签名（Base64）。
        sig: String,
        /// SPAKE2 消息（Base64）。
        pake: String,
        /// 本端能力位图（阶段 5 追加；旧端缺省为 `None`）。
        #[serde(default, skip_serializing_if = "Option::is_none")]
        caps: Option<u64>,
    },
    /// 主控端确认：`hmac(confirm_key, "C"‖session‖n_C)`。
    Auth {
        /// 16 字节随机数（Base64），AUTH_OK 必须原样回显。
        nonce: String,
        /// HMAC（Base64）。
        mac: String,
    },
    /// 被控端确认：`hmac(confirm_key, "H"‖session‖n_C)`。
    AuthOk {
        /// 回显的 nonce（Base64）。
        nonce: String,
        /// HMAC（Base64）。
        mac: String,
    },
}

impl HandshakeMessage {
    /// 序列化为线上字节并校验 4 KB 上限。
    ///
    /// # Returns
    /// JSON 字节。
    ///
    /// # Errors
    /// 序列化失败或超长返回 [`SecurityError::Protocol`]。
    pub fn encode(&self) -> Result<Vec<u8>, SecurityError> {
        let bytes = serde_json::to_vec(self).map_err(rdc_protocol::ProtocolError::from)?;
        if bytes.len() > rdc_protocol::message::MAX_SIGNAL_PACKET {
            return Err(SecurityError::HandshakeStateInvalid {
                state: "握手报文超过 4 KB".to_owned(),
            });
        }
        Ok(bytes)
    }

    /// 从线上字节解析。
    ///
    /// # Arguments
    /// * `bytes` - JSON 字节
    ///
    /// # Returns
    /// 握手消息。
    ///
    /// # Errors
    /// 超长或 JSON 非法返回 [`SecurityError::Protocol`]。
    pub fn decode(bytes: &[u8]) -> Result<Self, SecurityError> {
        if bytes.len() > rdc_protocol::message::MAX_SIGNAL_PACKET {
            return Err(SecurityError::HandshakeStateInvalid {
                state: "握手报文超过 4 KB".to_owned(),
            });
        }
        Ok(serde_json::from_slice(bytes).map_err(rdc_protocol::ProtocolError::from)?)
    }
}

/// 握手状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeState {
    /// 尚未开始。
    Idle,
    /// 主控端已发送 HELLO，等待 HELLO_ACK。
    HelloSent,
    /// 被控端已回 HELLO_ACK，等待 AUTH。
    HelloAckSent,
    /// 主控端已发送 AUTH，等待 AUTH_OK。
    AuthSent,
    /// 双方确认完成，可以使用通道密钥。
    Established,
    /// 任一步失败；会话必须关闭且不可重试。
    Failed,
}

/// 握手角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandshakeRole {
    /// 主控端（发起方）。
    Controller,
    /// 被控端（响应方）。
    Host,
}

/// 单次会话握手状态机。
pub struct SessionHandshake {
    /// 本端角色。
    role: HandshakeRole,
    /// 会话 ID（绑定所有签名与 HMAC，防跨会话重放）。
    session_id: String,
    /// 本端身份密钥（只用于签名，不参与密钥交换）。
    identity: IdentityKeyPair,
    /// 配对码；免密模式为 `None`（此时不启动 PAKE，需要外部注入挑战-应答）。
    code: Option<PairingCode>,
    /// 期望的对端身份公钥（被控端来自 `incoming`；主控端可为 `None`）。
    expected_peer_pub: Option<String>,
    /// X25519 临时密钥。
    eph: EphemeralKey,
    /// PAKE 会话。
    pake: Option<PakeSession>,
    /// 当前状态。
    state: HandshakeState,
    /// 派生后的通道密钥。
    keys: Option<ChannelKeys>,
    /// 4 位 SAS（来自 k_pake）。
    sas: Option<String>,
    /// 主控端生成的 AUTH nonce（被控端回显）。
    nonce_c: Option<[u8; 16]>,
    /// 本端能力位图（随 HELLO/HELLO_ACK 发送）。
    local_caps: Option<u64>,
    /// 对端能力位图（从 HELLO/HELLO_ACK 读取；旧端缺省为 `None`）。
    peer_caps: Option<u64>,
}

impl std::fmt::Debug for SessionHandshake {
    /// 脱敏 Debug：不输出身份私钥、临时私钥、通道密钥。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionHandshake")
            .field("role", &self.role)
            .field("session_id", &self.session_id)
            .field("state", &self.state)
            .field("established", &self.is_established())
            .finish()
    }
}

impl SessionHandshake {
    /// 创建主控端握手。
    ///
    /// # Arguments
    /// * `session_id` - 会话 ID
    /// * `identity` - 本端身份密钥
    /// * `code` - 配对码（一次性码模式）
    ///
    /// # Returns
    /// 处于 [`HandshakeState::Idle`] 的状态机。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new_controller(
        session_id: impl Into<String>,
        identity: IdentityKeyPair,
        code: PairingCode,
    ) -> Self {
        Self::new_internal(
            HandshakeRole::Controller,
            session_id.into(),
            identity,
            Some(code),
            None,
        )
    }

    /// 创建被控端握手。
    ///
    /// # Arguments
    /// * `session_id` - 会话 ID
    /// * `identity` - 本端身份密钥
    /// * `code` - 配对码
    /// * `expected_peer_pub` - 信令里收到的对端身份公钥（必填，用于验签绑定）
    ///
    /// # Returns
    /// 处于 [`HandshakeState::Idle`] 的状态机。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new_host(
        session_id: impl Into<String>,
        identity: IdentityKeyPair,
        code: PairingCode,
        expected_peer_pub: impl Into<String>,
    ) -> Self {
        Self::new_internal(
            HandshakeRole::Host,
            session_id.into(),
            identity,
            Some(code),
            Some(expected_peer_pub.into()),
        )
    }

    /// 内部构造。
    fn new_internal(
        role: HandshakeRole,
        session_id: String,
        identity: IdentityKeyPair,
        code: Option<PairingCode>,
        expected_peer_pub: Option<String>,
    ) -> Self {
        Self {
            role,
            session_id,
            identity,
            code,
            expected_peer_pub,
            eph: EphemeralKey::generate(),
            pake: None,
            state: HandshakeState::Idle,
            keys: None,
            sas: None,
            nonce_c: None,
            local_caps: None,
            peer_caps: None,
        }
    }

    /// 返回角色。
    ///
    /// # Returns
    /// 本端角色。
    ///
    /// # Errors
    /// 不会失败。
    pub fn role(&self) -> HandshakeRole {
        self.role
    }

    /// 返回当前状态。
    ///
    /// # Returns
    /// 状态拷贝。
    ///
    /// # Errors
    /// 不会失败。
    pub fn state(&self) -> HandshakeState {
        self.state
    }

    /// 是否已建立会话。
    ///
    /// # Returns
    /// 状态为 `Established` 时返回 `true`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn is_established(&self) -> bool {
        matches!(self.state, HandshakeState::Established)
    }

    /// 返回通道密钥（未建立时为 `None`）。
    ///
    /// # Returns
    /// 密钥引用。
    ///
    /// # Errors
    /// 不会失败。
    pub fn keys(&self) -> Option<&ChannelKeys> {
        self.keys.as_ref()
    }

    /// 返回 4 位 SAS（未建立时为 `None`）。
    ///
    /// # Returns
    /// 4 位数字串。
    ///
    /// # Errors
    /// 不会失败。
    pub fn sas(&self) -> Option<&str> {
        self.sas.as_deref()
    }

    /// 启动握手：主控端返回 HELLO，被控端返回 `None`（等待对方先发）。
    ///
    /// # Arguments
    /// * `now_ms` - 当前毫秒时间戳
    ///
    /// # Returns
    /// 需要发送的握手消息。
    ///
    /// # Errors
    /// 状态非法或未配置配对码时返回 [`SecurityError::HandshakeStateInvalid`]。
    pub fn start(&mut self, now_ms: i64) -> Result<Option<HandshakeMessage>, SecurityError> {
        if self.state != HandshakeState::Idle {
            return Err(self.invalid_state("start"));
        }
        if self.role != HandshakeRole::Controller {
            // 被控端必须等待 HELLO，不能主动发起。
            return Ok(None);
        }
        let code = self
            .code
            .as_ref()
            .ok_or(SecurityError::HandshakeStateInvalid {
                state: "主控端未配置配对码".to_owned(),
            })?;
        let (pake, pake_msg) = PakeSession::start(PakeRole::Controller, code);
        self.pake = Some(pake);
        let payload = auth_signing_payload(&self.session_id, &self.eph.public_key_bytes(), now_ms);
        let msg = HandshakeMessage::Hello {
            eph: self.eph.public_key_b64(),
            id: self.identity.public_key_b64(),
            identity: self.identity.public_key_b64(),
            ts: now_ms,
            sig: self.identity.sign_b64(&payload),
            pake: STANDARD.encode(&pake_msg),
            caps: self.local_caps,
        };
        self.state = HandshakeState::HelloSent;
        Ok(Some(msg))
    }

    /// 设置本端能力位图（必须在发送 HELLO/HELLO_ACK 之前调用）。
    ///
    /// # Arguments
    /// * `caps` - 本端能力位图（`rdc_protocol::capabilities::Capabilities::raw`）
    ///
    /// # Errors
    /// 不会失败。
    pub fn set_local_capabilities(&mut self, caps: u64) {
        self.local_caps = Some(caps);
    }

    /// 返回从对端 HELLO/HELLO_ACK 读取的能力位图。
    ///
    /// # Returns
    /// * `Some(bits)` - 对端携带了 caps；
    /// * `None` - 旧端未携带（调用方应按“旧端默认能力”回退）。
    ///
    /// # Errors
    /// 不会失败。
    pub fn peer_capabilities(&self) -> Option<u64> {
        self.peer_caps
    }

    /// 推进握手一步。
    ///
    /// # Arguments
    /// * `msg` - 对端发来的握手消息
    /// * `now_ms` - 当前毫秒时间戳
    ///
    /// # Returns
    /// 需要回给对端的消息；已建立时返回 `None`。
    ///
    /// # Errors
    /// 状态不接受该消息、验签失败、时间窗超限或 HMAC 校验失败时返回
    /// [`SecurityError::HandshakeStateInvalid`] / [`SecurityError::SignatureInvalid`] /
    /// [`SecurityError::ConfirmationFailed`]。
    pub fn step(
        &mut self,
        msg: HandshakeMessage,
        now_ms: i64,
    ) -> Result<Option<HandshakeMessage>, SecurityError> {
        let result = match (self.state, self.role, msg) {
            (HandshakeState::Idle, HandshakeRole::Host, HandshakeMessage::Hello {
                eph,
                id,
                identity,
                ts,
                sig,
                pake,
                caps,
            }) => self.host_on_hello(eph, id, identity, ts, sig, pake, caps, now_ms),
            (HandshakeState::HelloSent, HandshakeRole::Controller, HandshakeMessage::HelloAck {
                eph,
                id,
                identity,
                ts,
                sig,
                pake,
                caps,
            }) => self.controller_on_hello_ack(eph, id, identity, ts, sig, pake, caps, now_ms),
            (HandshakeState::HelloAckSent, HandshakeRole::Host, HandshakeMessage::Auth { nonce, mac }) => {
                self.host_on_auth(nonce, mac)
            }
            (HandshakeState::AuthSent, HandshakeRole::Controller, HandshakeMessage::AuthOk { nonce, mac }) => {
                self.controller_on_auth_ok(nonce, mac)
            }
            _ => Err(self.invalid_state("step")),
        };
        // 任一失败都进入 Failed：握手不可重试，必须重新建立会话。
        if result.is_err() {
            self.state = HandshakeState::Failed;
        }
        result
    }

    /// 被控端处理 HELLO 并生成 HELLO_ACK。
    #[allow(clippy::too_many_arguments)]
    fn host_on_hello(
        &mut self,
        eph: String,
        id: String,
        identity: String,
        ts: i64,
        sig: String,
        pake_msg: String,
        caps: Option<u64>,
        now_ms: i64,
    ) -> Result<Option<HandshakeMessage>, SecurityError> {
        self.verify_peer_hello(&eph, &id, &identity, ts, &sig, now_ms)?;
        // 旧端不带 caps：保留 None，由上层按“旧端默认能力”回退。
        self.peer_caps = caps;
        let pake_msg = STANDARD
            .decode(&pake_msg)
            .map_err(|_| SecurityError::PakeFailure("PAKE 消息不是合法 Base64"))?;
        // 被控端在收到 HELLO 后才启动 PAKE（B 侧消息随 HELLO_ACK 返回）。
        let code = self.code.as_ref().ok_or(SecurityError::PairingCodeInvalid)?;
        let (mut pake, my_msg) = PakeSession::start(PakeRole::Host, code);
        let k_pake = pake.finish(&pake_msg)?;
        let shared = self.eph.exchange_secret(&eph)?;
        self.install_keys(&shared, &k_pake);
        self.pake = Some(pake);
        let payload = auth_signing_payload(&self.session_id, &self.eph.public_key_bytes(), now_ms);
        self.state = HandshakeState::HelloAckSent;
        Ok(Some(HandshakeMessage::HelloAck {
            eph: self.eph.public_key_b64(),
            id: self.identity.public_key_b64(),
            identity: self.identity.public_key_b64(),
            ts: now_ms,
            sig: self.identity.sign_b64(&payload),
            pake: STANDARD.encode(&my_msg),
            caps: self.local_caps,
        }))
    }

    /// 主控端处理 HELLO_ACK 并生成 AUTH。
    #[allow(clippy::too_many_arguments)]
    fn controller_on_hello_ack(
        &mut self,
        eph: String,
        id: String,
        identity: String,
        ts: i64,
        sig: String,
        pake_msg: String,
        caps: Option<u64>,
        now_ms: i64,
    ) -> Result<Option<HandshakeMessage>, SecurityError> {
        self.verify_peer_hello(&eph, &id, &identity, ts, &sig, now_ms)?;
        // 旧端不带 caps：保留 None，由上层按“旧端默认能力”回退。
        self.peer_caps = caps;
        let pake_msg = STANDARD
            .decode(&pake_msg)
            .map_err(|_| SecurityError::PakeFailure("PAKE 消息不是合法 Base64"))?;
        let pake = self
            .pake
            .as_mut()
            .ok_or(SecurityError::PakeFailure("PAKE 会话缺失"))?;
        let k_pake = pake.finish(&pake_msg)?;
        let shared = self.eph.exchange_secret(&eph)?;
        self.install_keys(&shared, &k_pake);

        let mut nonce = [0u8; 16];
        OsRng.fill_bytes(&mut nonce);
        self.nonce_c = Some(nonce);
        let mac = hmac_b64(
            &self.required_keys()?.confirm,
            &confirm_payload(b"C", &self.session_id, &nonce),
        );
        self.state = HandshakeState::AuthSent;
        Ok(Some(HandshakeMessage::Auth {
            nonce: STANDARD.encode(nonce),
            mac,
        }))
    }

    /// 被控端校验 AUTH 并生成 AUTH_OK。
    fn host_on_auth(&mut self, nonce: String, mac: String) -> Result<Option<HandshakeMessage>, SecurityError> {
        let nonce: [u8; 16] = from_b64_exact(&nonce, 16)?;
        let keys = self.required_keys()?;
        verify_hmac_b64(&keys.confirm, &confirm_payload(b"C", &self.session_id, &nonce), &mac)?;
        let ok_mac = hmac_b64(
            &keys.confirm,
            &confirm_payload(b"H", &self.session_id, &nonce),
        );
        self.state = HandshakeState::Established;
        Ok(Some(HandshakeMessage::AuthOk {
            nonce: STANDARD.encode(nonce),
            mac: ok_mac,
        }))
    }

    /// 主控端校验 AUTH_OK。
    fn controller_on_auth_ok(&mut self, nonce: String, mac: String) -> Result<Option<HandshakeMessage>, SecurityError> {
        let nonce: [u8; 16] = from_b64_exact(&nonce, 16)?;
        let expected = self.nonce_c.ok_or(SecurityError::ConfirmationFailed)?;
        // nonce 必须原样回显：否则对端可能在重放旧 AUTH_OK。
        if !constant_time_eq(&nonce, &expected) {
            return Err(SecurityError::ConfirmationFailed);
        }
        let keys = self.required_keys()?;
        verify_hmac_b64(&keys.confirm, &confirm_payload(b"H", &self.session_id, &nonce), &mac)?;
        self.state = HandshakeState::Established;
        Ok(None)
    }

    /// 校验对端 HELLO/HELLO_ACK 的身份签名与时间窗。
    fn verify_peer_hello(
        &self,
        eph: &str,
        _id: &str,
        identity: &str,
        ts: i64,
        sig: &str,
        now_ms: i64,
    ) -> Result<(), SecurityError> {
        // 时间窗先校验：过期挑战不进入任何密码学运算，避免浪费 CPU。
        validate_auth_time(now_ms, ts)?;
        let eph_bytes: [u8; 32] = from_b64_exact(eph, 32)?;
        let payload = auth_signing_payload(&self.session_id, &eph_bytes, ts);
        verify_signature_b64(identity, &payload, sig)?;
        // 被控端必须把签名公钥与信令里收到的 peer_pub 绑定，防止中间人替换。
        if let Some(expected) = &self.expected_peer_pub {
            if expected != identity {
                return Err(SecurityError::SignatureInvalid);
            }
        }
        Ok(())
    }

    /// 安装通道密钥与 SAS。
    fn install_keys(&mut self, shared: &[u8; 32], k_pake: &[u8]) {
        self.keys = Some(derive_channel_keys(shared, k_pake));
        self.sas = Some(derive_sas(k_pake));
    }

    /// 返回已派生密钥的引用。
    fn required_keys(&self) -> Result<&ChannelKeys, SecurityError> {
        self.keys.as_ref().ok_or(SecurityError::HandshakeStateInvalid {
            state: "通道密钥尚未派生".to_owned(),
        })
    }

    /// 构造带当前状态的状态错误，并进入 `Failed`。
    fn invalid_state(&mut self, action: &str) -> SecurityError {
        let state = format!("{action} 在 {:?}/{:?} 不被接受", self.role, self.state);
        self.state = HandshakeState::Failed;
        SecurityError::HandshakeStateInvalid { state }
    }
}

// =====** 免密挑战-应答（信任设备） =====**

/// 签名一个免密登录挑战。
///
/// # Arguments
/// * `identity` - 本端身份密钥
/// * `session_id` - 会话 ID
/// * `challenge` - 32 字节随机挑战
/// * `ts` - 毫秒时间戳
///
/// # Returns
/// Base64 签名，签名内容为 `RDC-AUTH‖session‖challenge‖ts`。
///
/// # Errors
/// 不会失败。
pub fn sign_auth_challenge(
    identity: &IdentityKeyPair,
    session_id: &str,
    challenge: &[u8; CHALLENGE_LEN],
    ts: i64,
) -> String {
    identity.sign_b64(&auth_signing_payload(session_id, challenge, ts))
}

/// 校验免密登录挑战。
///
/// # Arguments
/// * `peer_public_b64` - 对端身份公钥
/// * `session_id` - 会话 ID
/// * `challenge` - 挑战
/// * `ts` - 对端时间戳
/// * `sig_b64` - 对端签名
/// * `now_ms` - 本端当前时间
///
/// # Returns
/// 验签与时间窗均通过返回 `Ok(())`。
///
/// # Errors
/// 超时返回 [`SecurityError::Protocol`]；签名不符返回 [`SecurityError::SignatureInvalid`]。
pub fn verify_auth_challenge(
    peer_public_b64: &str,
    session_id: &str,
    challenge: &[u8; CHALLENGE_LEN],
    ts: i64,
    sig_b64: &str,
    now_ms: i64,
) -> Result<(), SecurityError> {
    validate_auth_time(now_ms, ts)?;
    verify_signature_b64(
        peer_public_b64,
        &auth_signing_payload(session_id, challenge, ts),
        sig_b64,
    )
}

// =====** HMAC 辅助 =====**

/// 构造确认 HMAC 的输入：`前缀 ‖ session ‖ nonce`。
///
/// 前缀 `"C"`/`"H"` 区分方向，防止把主控端的 AUTH 直接当 AUTH_OK 回放。
fn confirm_payload(prefix: &[u8], session_id: &str, nonce: &[u8; 16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(prefix.len() + session_id.len() + 16);
    out.extend_from_slice(prefix);
    out.extend_from_slice(session_id.as_bytes());
    out.extend_from_slice(nonce);
    out
}

/// 计算 HMAC-SHA256 并返回 Base64。
fn hmac_b64(key: &[u8; 32], payload: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC 支持任意长度密钥");
    mac.update(payload);
    STANDARD.encode(mac.finalize().into_bytes())
}

/// 常量时间校验 HMAC。
fn verify_hmac_b64(key: &[u8; 32], payload: &[u8], mac_b64: &str) -> Result<(), SecurityError> {
    let provided = STANDARD
        .decode(mac_b64)
        .map_err(|_| SecurityError::ConfirmationFailed)?;
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC 支持任意长度密钥");
    mac.update(payload);
    // `verify_slice` 内部是常量时间比较，避免时序侧信道。
    mac.verify_slice(&provided)
        .map_err(|_| SecurityError::ConfirmationFailed)
}

/// 常量时间比较两个 16 字节数组。
fn constant_time_eq(a: &[u8; 16], b: &[u8; 16]) -> bool {
    use subtle::ConstantTimeEq;
    a.ct_eq(b).into()
}

/// 便于测试：从种子构造确定性身份。
#[cfg(test)]
fn identity_from_seed(seed: u8) -> IdentityKeyPair {
    IdentityKeyPair::from_seed(&[seed; 32])
}

// =====** 单元测试：完整握手与失败路径 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 完整握手必须两端建立、SAS 一致、派生密钥一致。
    #[test]
    fn full_handshake_establishes() {
        let code = PairingCode::parse("ABCD-EFGH-JK").expect("合法");
        let controller_id = identity_from_seed(1);
        let host_id = identity_from_seed(2);
        let controller_pub = controller_id.public_key_b64();

        let mut c = SessionHandshake::new_controller("s-1", controller_id, code.clone());
        let mut h = SessionHandshake::new_host("s-1", host_id, code, controller_pub);

        let hello = c.start(1000).expect("start").expect("HELLO");
        let ack = h.step(hello, 1000).expect("host hello").expect("ACK");
        let auth = c.step(ack, 1000).expect("controller ack").expect("AUTH");
        let ok = h.step(auth, 1000).expect("host auth").expect("AUTH_OK");
        c.step(ok, 1000).expect("controller ok");

        assert!(c.is_established());
        assert!(h.is_established());
        assert_eq!(c.sas().expect("SAS"), h.sas().expect("SAS"));
        // 对称性：通道密钥必须两端一致（用 confirm 做代表比较）。
        assert_eq!(
            c.keys().expect("keys").confirm,
            h.keys().expect("keys").confirm
        );
    }

    /// 握手报文必须携带本端能力位，且两端都能读到对方位图（bit13/bit14 显式 opt-in）。
    #[test]
    fn handshake_carries_capabilities_both_ways() {
        let code = PairingCode::parse("ABCD-EFGH-JK").expect("合法");
        let controller_id = identity_from_seed(8);
        let host_id = identity_from_seed(9);
        let controller_pub = controller_id.public_key_b64();
        let controller_caps = (1u64 << 13) | (1u64 << 14);
        let host_caps = 1u64 << 14;

        let mut c = SessionHandshake::new_controller("s-caps", controller_id, code.clone());
        let mut h = SessionHandshake::new_host("s-caps", host_id, code, controller_pub);
        c.set_local_capabilities(controller_caps);
        h.set_local_capabilities(host_caps);

        let hello = c.start(1000).expect("start").expect("HELLO");
        let ack = h.step(hello, 1000).expect("host hello").expect("ACK");
        // 被控端此时已读到主控端位图，主控端还没读到。
        assert_eq!(h.peer_capabilities(), Some(controller_caps));
        assert_eq!(c.peer_capabilities(), None);
        let auth = c.step(ack, 1000).expect("controller ack").expect("AUTH");
        assert_eq!(c.peer_capabilities(), Some(host_caps));
        let ok = h.step(auth, 1000).expect("host auth").expect("AUTH_OK");
        c.step(ok, 1000).expect("controller ok");

        // 双方按位与：bit13 单端支持不成立，bit14 双方支持。
        let negotiated = c.peer_capabilities().unwrap() & controller_caps;
        assert_eq!(negotiated, host_caps);
    }

    /// 旧端（不带 caps）必须解析为 `None`，由上层回退默认能力。
    #[test]
    fn hello_without_caps_decodes_as_none() {
        let json = br#"{"t":"hello","eph":"AA","id":"x","identity":"y","ts":1,"sig":"s","pake":"p"}"#;
        match HandshakeMessage::decode(json).expect("旧端报文应可解析") {
            HandshakeMessage::Hello { caps, .. } => assert_eq!(caps, None),
            other => panic!("应为 Hello，实际 {other:?}"),
        }
    }

    /// 配对码错误必须卡在 AUTH 校验（而不是“握手成功但密钥不同”）。
    #[test]
    fn wrong_pairing_code_fails_after_pake() {
        let good = PairingCode::parse("ABCD-EFGH-JK").expect("合法");
        let bad = PairingCode::parse("ABCD-EFGH-J1").expect("合法");
        let controller_id = identity_from_seed(3);
        let host_id = identity_from_seed(4);
        let controller_pub = controller_id.public_key_b64();

        let mut c = SessionHandshake::new_controller("s-2", controller_id, good);
        let mut h = SessionHandshake::new_host("s-2", host_id, bad, controller_pub);

        let hello = c.start(1000).expect("start").expect("HELLO");
        let ack = h.step(hello, 1000).expect("host hello").expect("ACK");
        let auth = c.step(ack, 1000).expect("controller ack").expect("AUTH");
        let err = h.step(auth, 1000).expect_err("必须失败");
        assert!(matches!(err, SecurityError::ConfirmationFailed));
        assert_eq!(h.state(), HandshakeState::Failed);
        assert!(!h.is_established());
    }

    /// 被控端必须拒绝与信令 peer_pub 不一致的身份公钥（防中间人替换）。
    #[test]
    fn host_rejects_unexpected_identity() {
        let code = PairingCode::parse("ABCD-EFGH-JK").expect("合法");
        let controller_id = identity_from_seed(5);
        let host_id = identity_from_seed(6);
        let wrong_pub = identity_from_seed(7).public_key_b64();
        let mut c = SessionHandshake::new_controller("s-3", controller_id, code.clone());
        let mut h = SessionHandshake::new_host("s-3", host_id, code, wrong_pub);
        let hello = c.start(1000).expect("start").expect("HELLO");
        assert!(h.step(hello, 1000).is_err());
    }

    /// 超出 ±30 s 的 HELLO 必须被拒绝。
    #[test]
    fn stale_hello_is_rejected() {
        let code = PairingCode::parse("ABCD-EFGH-JK").expect("合法");
        let controller_id = identity_from_seed(8);
        let host_id = identity_from_seed(9);
        let controller_pub = controller_id.public_key_b64();
        let mut c = SessionHandshake::new_controller("s-4", controller_id, code.clone());
        let mut h = SessionHandshake::new_host("s-4", host_id, code, controller_pub);
        let hello = c.start(1000).expect("start").expect("HELLO");
        assert!(h.step(hello, 1000 + 31_000).is_err());
    }

    /// 篡改 HELLO 的任意字段必须验签失败。
    #[test]
    fn tampered_hello_fails_signature() {
        let code = PairingCode::parse("ABCD-EFGH-JK").expect("合法");
        let controller_id = identity_from_seed(10);
        let host_id = identity_from_seed(11);
        let controller_pub = controller_id.public_key_b64();
        let mut c = SessionHandshake::new_controller("s-5", controller_id, code.clone());
        let mut h = SessionHandshake::new_host("s-5", host_id, code, controller_pub);
        let mut hello = c.start(1000).expect("start").expect("HELLO");
        if let HandshakeMessage::Hello { eph, .. } = &mut hello {
            // 改临时公钥而不重签，必须被拒绝。
            eph.push('A');
        }
        assert!(matches!(
            h.step(hello, 1000),
            Err(SecurityError::InvalidKeyLength { .. })
        ));
    }

    /// 免密挑战签名必须可验，时间窗外的签名必须拒绝。
    #[test]
    fn challenge_signature_roundtrip() {
        let id = identity_from_seed(12);
        let challenge = [0x55u8; CHALLENGE_LEN];
        let sig = sign_auth_challenge(&id, "s-6", &challenge, 1000);
        verify_auth_challenge(&id.public_key_b64(), "s-6", &challenge, 1000, &sig, 1000)
            .expect("验签");
        assert!(verify_auth_challenge(&id.public_key_b64(), "s-6", &challenge, 1000, &sig, 1_031_000).is_err());
    }
}
