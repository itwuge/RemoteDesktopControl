//! rdc-connect / QUIC 信令客户端
//!
//! 层级：L2 能力层
//! 用途：实现 docs/03 的最小信令客户端：注册、呼叫、应答、候选交换、上报 connected、
//!       断开；注册流的下行半边常驻接收服务端推送（incoming / punch / candidates / bye）。
//! 调用方：rdc-cli 的本机回环 Demo、后续 rdc-core 的连接编排
//!
//! 安全前提：所有请求（除 register）都以 token 做 HMAC，且序号单调递增；
//! `register` 用身份私钥对规范化信封签名。token 只存在本进程内存。

use std::net::SocketAddr;

use quinn::{Connection, Endpoint, RecvStream};
use rdc_protocol::message::MAX_SIGNAL_PACKET;
use rdc_protocol::{
    CallMode, CandidateDesc, Capabilities, SignalEnvelope, SignalErrorCode, SignalMessage,
    SignalPath,
};
use rdc_security::{hmac_sha256_b64, IdentityKeyPair};

use crate::quic::{client_endpoint, connect, now_ms, read_frame, write_frame};
use crate::ConnectError;

// =====** QuicSignalClient =====**

/// 已注册的信令客户端。
///
/// 生命周期：`connect_and_register` 建立 QUIC 连接并完成注册；`close()` 或 Drop
/// 触发端点关闭。注册流的上行半边在注册完成后不再使用，下行半边用于推送。
pub struct QuicSignalClient {
    /// QUIC 端点（必须与连接同生命周期，否则连接会被关闭）。
    endpoint: Endpoint,
    /// 与信令服务的连接。
    conn: Connection,
    /// 本端 peer ID。
    peer_id: String,
    /// 服务端签发的会话 token（HMAC 密钥）。
    token: String,
    /// 请求序号；每次请求递增，服务端按窗口校验防重放。
    seq: u64,
    /// 注册流的下行半边：服务端推送通道。
    push_recv: RecvStream,
}

impl std::fmt::Debug for QuicSignalClient {
    /// 脱敏 Debug：不输出 token。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QuicSignalClient")
            .field("peer_id", &self.peer_id)
            .field("seq", &self.seq)
            .finish_non_exhaustive()
    }
}

impl QuicSignalClient {
    /// 连接信令服务并完成注册。
    ///
    /// # Arguments
    /// * `server_addr` - 信令服务地址
    /// * `cert_der` - 服务端自签证书 DER（唯一信任根）
    /// * `identity` - 本端身份密钥（注册签名用）
    /// * `peer_id` - 本端设备 ID
    /// * `device_name` - 可读设备名（展示用）
    /// * `platform` - 平台标识
    /// * `caps` - 能力位图
    ///
    /// # Returns
    /// 已注册、可收发推送的客户端。
    ///
    /// # Errors
    /// 连接失败、注册被拒或响应不是 `register_ok` 时返回 [`ConnectError`]。
    pub async fn connect_and_register(
        server_addr: SocketAddr,
        cert_der: &[u8],
        identity: &IdentityKeyPair,
        peer_id: &str,
        device_name: &str,
        platform: &str,
        caps: Capabilities,
    ) -> Result<Self, ConnectError> {
        let endpoint = client_endpoint(cert_der)?;
        let conn = connect(&endpoint, server_addr).await?;
        let (mut send, mut recv) = conn
            .open_bi()
            .await
            .map_err(|e| crate::quic::transport_error("打开注册流", e))?;

        // 注册信封：序号从 0 开始，服务端对 register 只验签不校验序号。
        let mut env = SignalEnvelope::new(
            peer_id,
            0,
            now_ms(),
            SignalMessage::Register {
                pub_key: identity.public_key_b64(),
                device_name: device_name.to_owned(),
                platform: platform.to_owned(),
                caps,
            },
        );
        let payload = env.canonical_auth_bytes()?;
        env = env.signed(identity.sign_b64(&payload));
        write_frame(&mut send, &env.encode()?).await?;
        // 单次请求流：写完即关闭发送半边。
        let _ = send.finish();

        let reply = read_frame(&mut recv).await?;
        let reply = SignalEnvelope::decode(&reply)?;
        let (token, observed) = match reply.body {
            SignalMessage::RegisterOk { token, observed, .. } => (token, observed),
            SignalMessage::Error { code, message } => {
                return Err(ConnectError::Signaling { code, message });
            }
            other => {
                return Err(ConnectError::Signaling {
                    code: SignalErrorCode::Internal,
                    message: format!("注册响应类型非法: {}", other.type_name()),
                });
            }
        };
        tracing::debug!(peer = %peer_id, %observed, "信令注册完成");

        Ok(Self {
            endpoint,
            conn,
            peer_id: peer_id.to_owned(),
            token,
            seq: 0,
            push_recv: recv,
        })
    }

    /// 返回本端 peer ID。
    ///
    /// # Returns
    /// 注册时使用的设备 ID。
    ///
    /// # Errors
    /// 不会失败。
    pub fn peer_id(&self) -> &str {
        &self.peer_id
    }

    /// 发送一次带 token HMAC 的请求并等待单包响应。
    ///
    /// # Arguments
    /// * `body` - 请求消息体
    ///
    /// # Returns
    /// 服务端响应信封（已排除 `error`）。
    ///
    /// # Errors
    /// 服务端返回 `error` 时转换为 [`ConnectError::Signaling`]；网络失败返回
    /// [`ConnectError::Transport`]。
    async fn request(&mut self, body: SignalMessage) -> Result<SignalEnvelope, ConnectError> {
        self.seq += 1;
        let mut env = SignalEnvelope::new(&self.peer_id, self.seq, now_ms(), body);
        let payload = env.canonical_auth_bytes()?;
        env = env.signed(hmac_sha256_b64(self.token.as_bytes(), &payload));

        let (mut send, mut recv) = self
            .conn
            .open_bi()
            .await
            .map_err(|e| crate::quic::transport_error("打开请求流", e))?;
        write_frame(&mut send, &env.encode()?).await?;
        let _ = send.finish();
        let reply = read_frame(&mut recv).await?;
        let reply = SignalEnvelope::decode(&reply)?;
        if let SignalMessage::Error { code, message } = &reply.body {
            return Err(ConnectError::Signaling {
                code: *code,
                message: message.clone(),
            });
        }
        Ok(reply)
    }

    /// 读取一条服务端推送（阻塞直到有推送或连接断开）。
    ///
    /// # Returns
    /// 推送信封。
    ///
    /// # Errors
    /// 连接断开或报文非法时返回 [`ConnectError`]。
    pub async fn recv_push(&mut self) -> Result<SignalEnvelope, ConnectError> {
        let frame = read_frame(&mut self.push_recv).await?;
        SignalEnvelope::decode(&frame).map_err(ConnectError::from)
    }

    /// 发起呼叫并等待被控端应答（最长 30 s，由服务端超时控制）。
    ///
    /// # Arguments
    /// * `target` - 目标 peer ID
    /// * `mode` - 呼叫模式（Demo 使用一次性配对码）
    /// * `policy` - 请求的权限位
    /// * `code_proof` - 配对码证明（服务端只透传）
    ///
    /// # Returns
    /// 被控端的 `accept` 信封（含 session 与可选挑战）。
    ///
    /// # Errors
    /// 目标不在线、被拒绝或超时返回 [`ConnectError::Signaling`]。
    pub async fn call(
        &mut self,
        target: &str,
        mode: CallMode,
        policy: u16,
        code_proof: &str,
    ) -> Result<SignalEnvelope, ConnectError> {
        self.request(SignalMessage::Call {
            target: target.to_owned(),
            allow_relay: false,
            policy,
            code_proof: code_proof.to_owned(),
            mode,
            chal_c: None,
            ts_c: None,
            sig_c: None,
        })
        .await
    }

    /// 接受一次呼叫。
    ///
    /// # Arguments
    /// * `session` - 会话 ID
    ///
    /// # Returns
    /// 服务端确认信封。
    ///
    /// # Errors
    /// 会话不存在或状态非法时返回 [`ConnectError::Signaling`]。
    pub async fn accept(&mut self, session: &str) -> Result<SignalEnvelope, ConnectError> {
        self.request(SignalMessage::Accept {
            session: session.to_owned(),
            chal_h: None,
            ts_h: None,
            sig_h: None,
        })
        .await
    }

    /// 拒绝一次呼叫。
    ///
    /// # Arguments
    /// * `session` - 会话 ID
    /// * `reason` - 面向对端的原因（不含敏感信息）
    ///
    /// # Returns
    /// 服务端确认信封。
    ///
    /// # Errors
    /// 会话不存在时返回 [`ConnectError::Signaling`]。
    pub async fn refuse(&mut self, session: &str, reason: &str) -> Result<SignalEnvelope, ConnectError> {
        self.request(SignalMessage::Refuse {
            session: session.to_owned(),
            reason: reason.to_owned(),
        })
        .await
    }

    /// 上报本端候选列表。
    ///
    /// # Arguments
    /// * `session` - 会话 ID
    /// * `list` - 候选地址（按优先级排列）
    ///
    /// # Returns
    /// 服务端确认信封。
    ///
    /// # Errors
    /// 会话不存在或长度超限时返回 [`ConnectError`]。
    pub async fn candidates(
        &mut self,
        session: &str,
        list: Vec<CandidateDesc>,
    ) -> Result<SignalEnvelope, ConnectError> {
        self.request(SignalMessage::Candidates {
            session: session.to_owned(),
            list,
        })
        .await
    }

    /// 上报最终路径。
    ///
    /// # Arguments
    /// * `session` - 会话 ID
    /// * `path` - 实际使用的路径
    ///
    /// # Returns
    /// 服务端确认信封。
    ///
    /// # Errors
    /// 会话不存在时返回 [`ConnectError::Signaling`]。
    pub async fn connected(
        &mut self,
        session: &str,
        path: SignalPath,
    ) -> Result<SignalEnvelope, ConnectError> {
        self.request(SignalMessage::Connected {
            session: session.to_owned(),
            path,
        })
        .await
    }

    /// 结束会话（幂等）。
    ///
    /// # Arguments
    /// * `session` - 会话 ID
    /// * `reason` - 结束原因
    ///
    /// # Returns
    /// 服务端 bye 确认。
    ///
    /// # Errors
    /// 连接不可用时返回 [`ConnectError::Transport`]。
    pub async fn bye(&mut self, session: &str, reason: &str) -> Result<SignalEnvelope, ConnectError> {
        self.request(SignalMessage::Bye {
            session: session.to_owned(),
            reason: reason.to_owned(),
        })
        .await
    }

    /// 关闭端点与连接。
    ///
    /// # Returns
    /// 不会失败。
    ///
    /// # Errors
    /// 不会失败。
    pub fn close(&self) {
        self.conn.close(quinn::VarInt::from_u32(0), b"bye");
        self.endpoint.close(quinn::VarInt::from_u32(0), b"bye");
    }

    /// 返回可发送的最大帧长度（供调用方做预检查）。
    ///
    /// # Returns
    /// 4 KB。
    ///
    /// # Errors
    /// 不会失败。
    pub const fn max_frame_len() -> usize {
        MAX_SIGNAL_PACKET
    }
}
