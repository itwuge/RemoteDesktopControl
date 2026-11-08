//! rdc-signal / QUIC 信令服务端
//!
//! 层级：L1 服务端
//! 用途：在 UDP 21116 上提供 TLS 1.3 over QUIC 信令：注册、呼叫、应答、候选交换、
//!       打洞提示、断开；按 IP/目标做失败退避与封禁（docs/03 §3、§4）。
//! 调用方：rdc-signal 二进制入口、rdc-cli 的 loopback demo（同进程启动）
//!
//! 安全前提：所有请求（除 register）都必须带 HMAC(token) 且序号未重放；
//! register 必须用身份私钥签名。任何校验失败都返回 `auth_failed` 且不泄漏细节。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use quinn::{Connection, Endpoint, RecvStream, SendStream};
use rand_core::{OsRng, RngCore};
use rdc_protocol::message::{CandidateDesc, MAX_SIGNAL_PACKET, CALL_TIMEOUT_MS};
use rdc_protocol::{
    CallMode, PunchRole, SignalEnvelope, SignalErrorCode, SignalMessage, SignalPath,
};
use rdc_security::{
    verify_hmac_sha256_b64, verify_signature_b64, FailureLimiter, ReplayWindow, ThrottlePolicy,
    ThrottleReject,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::sync::{mpsc, oneshot};

use crate::state::{transition, SessionEvent, SessionState};

// =====** 常量 =====**

/// 信令 token 有效期（毫秒）。
pub const SIGNAL_TOKEN_TTL_MS: i64 = 60 * 60 * 1000;

/// 单连接请求帧上限（4 KB 信令包 + 4 字节长度前缀）。
const MAX_FRAME_BYTES: usize = MAX_SIGNAL_PACKET;

/// TLS 自签证书的 SAN（客户端必须用同一名称连接）。
pub const TLS_SERVER_NAME: &str = "rdc.local";

/// 每连接服务端推送队列上限；信令是低频控制面，64 条足够，超出说明对端已停摆。
const PUSH_CHANNEL_CAPACITY: usize = 64;

// =====** 错误类型 =====**

/// 服务端启动/运行错误。
#[derive(Debug, thiserror::Error)]
pub enum SignalServerError {
    /// 端口绑定或 IO 失败。
    #[error("信令服务 IO 失败: {0}")]
    Io(#[from] std::io::Error),
    /// TLS 配置失败。
    #[error("TLS 配置失败: {0}")]
    Tls(String),
    /// 证书生成失败。
    #[error("证书生成失败: {0}")]
    Certificate(String),
}

// =====** 数据结构 =====**

/// 已注册对端。
struct PeerRecord {
    /// 身份公钥（Base64），用于填充 `incoming` 给被控端验签。
    pub_key: String,
    /// 会话 token（HMAC 密钥），只存在内存。
    token: String,
    /// token 签发时间（毫秒）。
    token_issued_ms: i64,
    /// 服务端向该对端推送消息的通道（有界，capacity: 64）。
    push_tx: mpsc::Sender<SignalEnvelope>,
    /// 连接稳定 ID，用于断链清理。
    conn_id: usize,
    /// 重放窗口（只在校验通过的包上推进）。
    replay: ReplayWindow,
}

/// 服务端会话记录。
struct SessionRecord {
    /// 主控端 peer ID。
    controller: String,
    /// 被控端 peer ID。
    host: String,
    /// 会话状态。
    state: SessionState,
    /// 呼叫请求的挂起应答通道（call 请求流等待它）。
    controller_tx: Option<oneshot::Sender<SignalEnvelope>>,
    /// 主控端候选。
    controller_candidates: Option<Vec<CandidateDesc>>,
    /// 被控端候选。
    host_candidates: Option<Vec<CandidateDesc>>,
}

/// 跨任务共享状态。
struct Shared {
    /// peer ID → 记录。
    registry: Mutex<HashMap<String, PeerRecord>>,
    /// session ID → 记录。
    sessions: Mutex<HashMap<String, SessionRecord>>,
    /// 按 IP 的失败限流。
    ip_limiter: Mutex<FailureLimiter>,
    /// 按目标设备的失败限流。
    target_limiter: Mutex<FailureLimiter>,
}

// =====** SignalServer =====**

/// 信令服务端。
pub struct SignalServer {
    /// QUIC 端点（`close()` 可停止 accept 循环）。
    endpoint: Endpoint,
    /// 实际监听地址。
    addr: SocketAddr,
    /// 自签证书 DER（Demo 中交给客户端作为信任根）。
    cert_der: Vec<u8>,
    /// 私钥 DER（Demo 中直连端也复用同一证书）。
    key_der: Vec<u8>,
    /// 共享状态。
    shared: Arc<Shared>,
}

impl SignalServer {
    /// 绑定端口并生成自签证书。
    ///
    /// # Arguments
    /// * `addr` - 监听地址；Demo 传 `127.0.0.1:0` 由系统分配端口
    ///
    /// # Returns
    /// 已准备好 accept 的服务端。
    ///
    /// # Errors
    /// 证书生成、TLS 配置或端口绑定失败时返回 [`SignalServerError`]。
    pub async fn bind(addr: SocketAddr) -> Result<Self, SignalServerError> {
        // rustls 0.23 要求进程内安装一次 CryptoProvider；重复安装返回 Err 且可忽略。
        let _ = rustls::crypto::ring::default_provider().install_default(); // gate-allow R-C-2: 进程级 CryptoProvider 已装好时返回 Err，重复安装无害

        // 自签证书：文档允许本地使用自签，客户端把该证书作为唯一信任根。
        let certified = rcgen::generate_simple_self_signed(vec![TLS_SERVER_NAME.to_owned()])
            .map_err(|e| SignalServerError::Certificate(e.to_string()))?;
        let cert_der = certified.cert.der().to_vec();
        let key_der = certified.key_pair.serialize_der();

        let tls = rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(
                vec![CertificateDer::from(cert_der.clone())],
                PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der.clone())),
            )
            .map_err(|e| SignalServerError::Tls(e.to_string()))?;
        // quinn 0.11 要求把 rustls 配置包装成 QUIC 专用的 crypto 配置。
        let quic_crypto = quinn::crypto::rustls::QuicServerConfig::try_from(tls)
            .map_err(|e| SignalServerError::Tls(e.to_string()))?;
        let server_config = quinn::ServerConfig::with_crypto(Arc::new(quic_crypto));
        let endpoint = Endpoint::server(server_config, addr)?;
        let addr = endpoint.local_addr()?;

        Ok(Self {
            endpoint,
            addr,
            cert_der,
            key_der,
            shared: Arc::new(Shared {
                registry: Mutex::new(HashMap::new()),
                sessions: Mutex::new(HashMap::new()),
                ip_limiter: Mutex::new(FailureLimiter::new()),
                target_limiter: Mutex::new(FailureLimiter::with_policy(ThrottlePolicy::target())),
            }),
        })
    }

    /// 返回实际监听地址。
    ///
    /// # Returns
    /// `ip:port`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// 返回自签证书 DER。
    ///
    /// # Returns
    /// DER 字节切片。
    ///
    /// # Errors
    /// 不会失败。
    pub fn cert_der(&self) -> &[u8] {
        &self.cert_der
    }

    /// 返回私钥 DER（仅供 Demo 复用证书，生产环境必须由配置注入）。
    ///
    /// # Returns
    /// PKCS#8 DER 字节切片。
    ///
    /// # Errors
    /// 不会失败。
    pub fn key_der(&self) -> &[u8] {
        &self.key_der
    }

    /// 运行 accept 循环直到 [`Self::shutdown`] 被调用。
    ///
    /// # Returns
    /// 端点关闭后返回。
    ///
    /// # Errors
    /// 不会失败（单个连接错误只影响该连接）。
    pub async fn run(&self) {
        while let Some(connecting) = self.endpoint.accept().await {
            let shared = self.shared.clone();
            tokio::spawn(async move {
                if let Ok(conn) = connecting.await {
                    handle_connection(conn, shared).await;
                }
            });
        }
    }

    /// 关闭端点，停止 accept 循环。
    ///
    /// # Returns
    /// 不会失败。
    pub fn shutdown(&self) {
        // QUIC 应用错误码 0 表示正常关闭；reason 仅用于日志诊断。
        self.endpoint.close(quinn::VarInt::from_u32(0), b"shutdown");
    }

    /// 返回共享状态（Demo 用于输出在线设备数）。
    ///
    /// # Returns
    /// 在线 peer 数。
    pub fn peer_count(&self) -> usize {
        lock(&self.shared.registry).len()
    }
}

// =====** 连接与请求处理 =====**

/// 处理一条 QUIC 连接：并发接收多条请求流，断开时清理该连接的 peer/session。
async fn handle_connection(conn: Connection, shared: Arc<Shared>) {
    let remote = conn.remote_address();
    let conn_id = conn.stable_id();
    // 连接关闭属于正常退出路径；其它错误只影响本连接。
    while let Ok((send, recv)) = conn.accept_bi().await {
        let shared = shared.clone();
        tokio::spawn(async move {
            handle_request(send, recv, shared, remote, conn_id).await;
        });
    }
    cleanup_connection(&shared, conn_id);
}

/// 处理单条请求流（长度前缀 + JSON 信封）。
async fn handle_request(
    send: SendStream,
    mut recv: RecvStream,
    shared: Arc<Shared>,
    remote: SocketAddr,
    conn_id: usize,
) {
    let frame = match read_frame(&mut recv).await {
        Ok(frame) => frame,
        Err(_) => return,
    };
    let env = match SignalEnvelope::decode(&frame) {
        Ok(env) => env,
        Err(err) => {
            let message = format!("报文不可解析: {err}");
            let body = SignalMessage::Error {
                code: SignalErrorCode::BadRequest,
                message,
            };
            respond(send, server_msg(body)).await;
            return;
        }
    };

    let now = now_ms();
    let ip = remote.ip().to_string();
    // 限流检查必须先落成一个不含守卫的布尔/结果值，避免 MutexGuard 跨 await 持有。
    let allowed = lock(&shared.ip_limiter).check_allowed(&ip, now);
    if let Err(reject) = allowed {
        respond(send, throttle_error(reject)).await;
        return;
    }

    // register 用身份签名；其余消息用 token HMAC。
    if matches!(env.body, SignalMessage::Register { .. }) {
        handle_register(send, env, shared, ip, remote, conn_id).await;
        return;
    }

    if let Err(code) = verify_request(&shared, &env, now) {
        record_failure(&shared, &ip, &env, now);
        let message = "请求校验失败（签名/序号/token）".to_owned();
        respond(send, server_msg(SignalMessage::Error { code, message })).await;
        return;
    }
    lock(&shared.ip_limiter).record_success(&ip);

    match env.body.clone() {
        SignalMessage::Keepalive => {
            let body = SignalMessage::AuthResult {
                session: String::new(),
                ok: true,
                reason: None,
            };
            respond(send, server_msg(body)).await;
        }
        SignalMessage::Call {
            target,
            allow_relay,
            mode,
            chal_c,
            ts_c,
            sig_c,
            ..
        } => {
            handle_call(
                send,
                &shared,
                &env,
                target,
                allow_relay,
                mode,
                chal_c,
                ts_c,
                sig_c,
                now,
            )
            .await;
        }
        SignalMessage::Accept { session, .. } => {
            handle_accept(send, &shared, &env, &session).await;
        }
        SignalMessage::Refuse { session, reason } => {
            handle_refuse(send, &shared, &env, &session, &reason).await;
        }
        SignalMessage::Candidates { session, list } => {
            handle_candidates(send, &shared, &env, &session, list).await;
        }
        SignalMessage::Connected { session, path } => {
            handle_connected(send, &shared, &env, &session, path).await;
        }
        SignalMessage::Bye { session, reason } => {
            handle_bye(send, &shared, &env, &session, &reason).await;
        }
        SignalMessage::RelayRequest { .. } => {
            let body = SignalMessage::Error {
                code: SignalErrorCode::RelayDisabled,
                message: "S2 未启用中继".to_owned(),
            };
            respond(send, server_msg(body)).await;
        }
        // 服务端不接受客户端发来的这些消息类型。
        _ => {
            let body = SignalMessage::Error {
                code: SignalErrorCode::BadRequest,
                message: "消息类型不允许由客户端发送".to_owned(),
            };
            respond(send, server_msg(body)).await;
        }
    }
}

/// 处理注册。
async fn handle_register(
    mut send: SendStream,
    env: SignalEnvelope,
    shared: Arc<Shared>,
    ip: String,
    remote: SocketAddr,
    conn_id: usize,
) {
    let SignalMessage::Register {
        pub_key,
        device_name,
        platform,
        caps: _caps,
    } = env.body.clone()
    else {
        return;
    };
    let now = now_ms();
    if env.id.trim().is_empty() || env.id.len() > 64 {
        reject_register(send, &shared, &ip, SignalErrorCode::BadRequest, "peer id 非法").await;
        return;
    }
    let Some(sig) = env.sig.as_deref() else {
        reject_register(send, &shared, &ip, SignalErrorCode::AuthFailed, "注册缺少签名").await;
        return;
    };
    let payload = match env.canonical_auth_bytes() {
        Ok(bytes) => bytes,
        Err(_) => {
            reject_register(send, &shared, &ip, SignalErrorCode::BadRequest, "信封无法规范化").await;
            return;
        }
    };
    if verify_signature_b64(&pub_key, &payload, sig).is_err() {
        reject_register(send, &shared, &ip, SignalErrorCode::AuthFailed, "注册签名无效").await;
        return;
    }

    let (push_tx, mut push_rx) = mpsc::channel::<SignalEnvelope>(PUSH_CHANNEL_CAPACITY);
    let token = random_token();
    {
        let mut registry = lock(&shared.registry);
        // 同一 peer 重连：新连接覆盖旧连接（旧 push 通道随旧 writer 退出而失效）。
        registry.insert(
            env.id.clone(),
            PeerRecord {
                pub_key: pub_key.clone(),
                token: token.clone(),
                token_issued_ms: now,
                push_tx,
                conn_id,
                replay: ReplayWindow::new(),
            },
        );
    }
    lock(&shared.ip_limiter).record_success(&ip);
    tracing::info!(peer = %env.id, device = %device_name, %platform, remote = %remote, "peer registered");

    let body = SignalMessage::RegisterOk {
        observed: remote.to_string(),
        token,
        ttl: (SIGNAL_TOKEN_TTL_MS / 1000) as u32,
    };
    let response = server_msg(body);
    if let Ok(bytes) = response.encode() {
        if let Err(err) = write_frame(&mut send, &bytes).await {
            tracing::debug!(peer = %env.id, error = %err, "注册响应写入失败，对端可能已断开");
        }
    }

    // 注册流的上行半部从此只承载服务端推送；客户端已不再写入。
    let shared_writer = shared.clone();
    let peer_id = env.id.clone();
    tokio::spawn(async move {
        while let Some(env) = push_rx.recv().await {
            let Ok(bytes) = env.encode() else { break };
            if write_frame(&mut send, &bytes).await.is_err() {
                break;
            }
        }
        // 推送通道关闭说明连接已断：仅移除本连接的记录，避免误删重连的新记录。
        let mut registry = lock(&shared_writer.registry);
        if registry.get(&peer_id).is_some_and(|p| p.conn_id == conn_id) {
            registry.remove(&peer_id);
        }
    });
}

/// 处理呼叫：等待被控端 accept/refuse（最长 30 s）。
#[allow(clippy::too_many_arguments)]
async fn handle_call(
    send: SendStream,
    shared: &Arc<Shared>,
    env: &SignalEnvelope,
    target: String,
    allow_relay: bool,
    mode: CallMode,
    chal_c: Option<String>,
    ts_c: Option<i64>,
    sig_c: Option<String>,
    now: i64,
) {
    if target == env.id {
        let body = SignalMessage::Error {
            code: SignalErrorCode::BadRequest,
            message: "不能呼叫自己".to_owned(),
        };
        respond(send, server_msg(body)).await;
        return;
    }
    let target_tx = {
        let registry = lock(&shared.registry);
        registry.get(&target).map(|p| p.push_tx.clone())
    };
    let Some(target_tx) = target_tx else {
        record_target_failure(shared, &target, now);
        let body = SignalMessage::Error {
            code: SignalErrorCode::PeerNotFound,
            message: format!("目标 {target} 不在线"),
        };
        respond(send, server_msg(body)).await;
        return;
    };
    // 同样先取结果再 await，避免跨 await 持有锁守卫。
    let allowed = lock(&shared.target_limiter).check_allowed(&target, now);
    if let Err(reject) = allowed {
        respond(send, throttle_error(reject)).await;
        return;
    }

    let session = random_session_id();
    // 主控端公钥在 register 时绑定到 peer 记录；被控端必须拿到它才能验 `sig_c`。
    let controller_pub = lock(&shared.registry)
        .get(&env.id)
        .map(|p| p.pub_key.clone())
        .unwrap_or_default();
    let incoming = SignalMessage::Incoming {
        peer_id: env.id.clone(),
        peer_pub: controller_pub,
        session: session.clone(),
        allow_relay,
        mode,
        chal_c,
        ts_c,
        sig_c,
    };

    let (tx, rx) = oneshot::channel::<SignalEnvelope>();
    {
        let mut sessions = lock(&shared.sessions);
        sessions.insert(
            session.clone(),
            SessionRecord {
                controller: env.id.clone(),
                host: target.clone(),
                state: SessionState::Calling,
                controller_tx: Some(tx),
                controller_candidates: None,
                host_candidates: None,
            },
        );
    }
    if target_tx.try_send(server_msg(incoming)).is_err() {
        lock(&shared.sessions).remove(&session);
        let body = SignalMessage::Error {
            code: SignalErrorCode::PeerNotFound,
            message: "目标连接已关闭".to_owned(),
        };
        respond(send, server_msg(body)).await;
        return;
    }
    tracing::info!(session = %session, from = %env.id, to = %target, "call forwarded");

    match tokio::time::timeout(Duration::from_millis(CALL_TIMEOUT_MS), rx).await {
        Ok(Ok(env)) => {
            // accept 由被控端产生，会话保留供候选交换；refuse 已由对应处理器移除。
            respond(send, env).await;
        }
        _ => {
            lock(&shared.sessions).remove(&session);
            let body = SignalMessage::Error {
                code: SignalErrorCode::SessionExpired,
                message: "呼叫等待超时".to_owned(),
            };
            respond(send, server_msg(body)).await;
        }
    }
}

/// 处理接受。
async fn handle_accept(send: SendStream, shared: &Arc<Shared>, env: &SignalEnvelope, session: &str) {
    let outcome = {
        let mut sessions = lock(&shared.sessions);
        // 只有被控端本人可以接受该会话；其他 peer 一律视为会话不存在。
        match sessions.get_mut(session) {
            Some(record) if record.host == env.id => {
                match transition(record.state, SessionEvent::Accept) {
                    Ok(next) => {
                        record.state = next;
                        record.controller_tx.take()
                    }
                    Err(_) => None,
                }
            }
            _ => None,
        }
    };
    match outcome {
        Some(tx) => {
            if tx.send(env.clone()).is_err() {
                tracing::debug!(session = %session, "主控端应答通道已关闭，accept 通知未送达");
            }
            let body = SignalMessage::AuthResult {
                session: session.to_owned(),
                ok: true,
                reason: None,
            };
            respond(send, server_msg(body)).await;
        }
        None => {
            let body = SignalMessage::Error {
                code: SignalErrorCode::SessionExpired,
                message: "会话不存在、已结束或状态非法".to_owned(),
            };
            respond(send, server_msg(body)).await;
        }
    }
}

/// 处理拒绝。
async fn handle_refuse(
    send: SendStream,
    shared: &Arc<Shared>,
    env: &SignalEnvelope,
    session: &str,
    reason: &str,
) {
    let ok = {
        let mut sessions = lock(&shared.sessions);
        if let Some(record) = sessions.get_mut(session) {
            if record.host == env.id
                && transition(record.state, SessionEvent::Refuse).is_ok()
            {
                if let Some(tx) = record.controller_tx.take() {
                    if tx.send(env.clone()).is_err() {
                        tracing::debug!(session = %session, "主控端应答通道已关闭，refuse 通知未送达");
                    }
                }
                true
            } else {
                false
            }
        } else {
            false
        }
    };
    if ok {
        lock(&shared.sessions).remove(session);
        let body = SignalMessage::Bye {
            session: session.to_owned(),
            reason: reason.to_owned(),
        };
        respond(send, server_msg(body)).await;
    } else {
        let body = SignalMessage::Error {
            code: SignalErrorCode::SessionExpired,
            message: "会话不存在或状态非法".to_owned(),
        };
        respond(send, server_msg(body)).await;
    }
}

/// 处理候选交换：转发给对端，双方候选齐备后向双方下发 punch 提示。
async fn handle_candidates(
    send: SendStream,
    shared: &Arc<Shared>,
    env: &SignalEnvelope,
    session: &str,
    list: Vec<CandidateDesc>,
) {
    let Some((peer_id, is_controller, punch_me)) = update_candidates(shared, env, session, list)
    else {
        let body = SignalMessage::Error {
            code: SignalErrorCode::SessionExpired,
            message: "会话不存在或发送方不是成员".to_owned(),
        };
        respond(send, server_msg(body)).await;
        return;
    };

    // 把原始 candidates 转发给对端（对端只信任自己收到的列表）。
    let forwarded = env.clone();
    if !push_to_peer(shared, &peer_id, forwarded) {
        let body = SignalMessage::Error {
            code: SignalErrorCode::PeerNotFound,
            message: "对端连接已关闭".to_owned(),
        };
        respond(send, server_msg(body)).await;
        return;
    }
    // 只有双方列表都到齐才会返回 punch 地址；分别向两端推送各自的对端地址与角色，
    // 每端只收到“我要打给谁、我是谁”，不泄露无关信息。
    if let Some((controller_addr, host_addr)) = punch_me {
        let (controller_id, host_id) = if is_controller {
            (env.id.clone(), peer_id.clone())
        } else {
            (peer_id.clone(), env.id.clone())
        };
        let _ = push_to_peer( // gate-allow R-C-2: push_to_peer 队列满时已内部记 warn，false 表示对端离线，无需上层重复处理
            shared,
            &controller_id,
            server_msg(SignalMessage::Punch {
                peer_addr: host_addr,
                session: session.to_owned(),
                role: PunchRole::Controller,
            }),
        );
        let _ = push_to_peer( // gate-allow R-C-2: push_to_peer 队列满时已内部记 warn，false 表示对端离线，无需上层重复处理
            shared,
            &host_id,
            server_msg(SignalMessage::Punch {
                peer_addr: controller_addr,
                session: session.to_owned(),
                role: PunchRole::Host,
            }),
        );
    }
    let body = SignalMessage::AuthResult {
        session: session.to_owned(),
        ok: true,
        reason: None,
    };
    respond(send, server_msg(body)).await;
}

/// 处理 connected（仅记录/转发）。
async fn handle_connected(
    send: SendStream,
    shared: &Arc<Shared>,
    env: &SignalEnvelope,
    session: &str,
    path: SignalPath,
) {
    let peer_id = {
        let mut sessions = lock(&shared.sessions);
        sessions.get_mut(session).and_then(|record| {
            let is_member = record.controller == env.id || record.host == env.id;
            if !is_member {
                return None;
            }
            // 幂等：双方都可能上报 connected，已处于 Connected 时忽略转换错误。
            match transition(record.state, SessionEvent::Connected) {
                Ok(next) => record.state = next,
                Err(_) if record.state == SessionState::Connected => {}
                Err(_) => return None,
            }
            Some(if record.controller == env.id {
                record.host.clone()
            } else {
                record.controller.clone()
            })
        })
    };
    if let Some(peer_id) = peer_id {
        let _ = push_to_peer( // gate-allow R-C-2: push_to_peer 队列满时已内部记 warn，false 表示对端离线，无需上层重复处理
            shared,
            &peer_id,
            server_msg(SignalMessage::Connected {
                session: session.to_owned(),
                path,
            }),
        );
        let body = SignalMessage::AuthResult {
            session: session.to_owned(),
            ok: true,
            reason: None,
        };
        respond(send, server_msg(body)).await;
    } else {
        let body = SignalMessage::Error {
            code: SignalErrorCode::SessionExpired,
            message: "会话不存在".to_owned(),
        };
        respond(send, server_msg(body)).await;
    }
}

/// 处理断开：转发 bye 并移除会话。
async fn handle_bye(
    send: SendStream,
    shared: &Arc<Shared>,
    env: &SignalEnvelope,
    session: &str,
    reason: &str,
) {
    let peer_id = {
        let mut sessions = lock(&shared.sessions);
        sessions.get_mut(session).and_then(|record| {
            let is_member = record.controller == env.id || record.host == env.id;
            if !is_member {
                return None;
            }
            record.state = SessionState::Closed;
            Some(if record.controller == env.id {
                record.host.clone()
            } else {
                record.controller.clone()
            })
        })
    };
    let bye = server_msg(SignalMessage::Bye {
        session: session.to_owned(),
        reason: reason.to_owned(),
    });
    if let Some(peer_id) = peer_id {
        let _ = push_to_peer(shared, &peer_id, bye.clone()); // gate-allow R-C-2: push_to_peer 队列满时已内部记 warn，false 表示对端离线，无需上层重复处理
    }
    lock(&shared.sessions).remove(session);
    // bye 幂等：无论之前是否存在，都回 bye 确认。
    respond(send, bye).await;
}

// =====** 认证与限流辅助 =====**

/// 校验请求的 token HMAC 与重放窗口。
fn verify_request(
    shared: &Shared,
    env: &SignalEnvelope,
    now_ms: i64,
) -> Result<(), SignalErrorCode> {
    let payload = env
        .canonical_auth_bytes()
        .map_err(|_| SignalErrorCode::BadRequest)?;
    let mut registry = lock(&shared.registry);
    let peer = registry.get_mut(&env.id).ok_or(SignalErrorCode::AuthFailed)?;
    if now_ms > peer.token_issued_ms + SIGNAL_TOKEN_TTL_MS {
        return Err(SignalErrorCode::AuthFailed);
    }
    let sig = env.sig.as_deref().ok_or(SignalErrorCode::AuthFailed)?;
    verify_hmac_sha256_b64(peer.token.as_bytes(), &payload, sig)
        .map_err(|_| SignalErrorCode::AuthFailed)?;
    // 关键顺序：HMAC 通过后才推进窗口，避免伪造包把合法序号“挤”出窗口。
    peer.replay
        .check_and_update(env.seq)
        .map_err(|_| SignalErrorCode::AuthFailed)?;
    Ok(())
}

/// 记录一次失败（IP + 目标）。
fn record_failure(shared: &Shared, ip: &str, env: &SignalEnvelope, now: i64) {
    lock(&shared.ip_limiter).record_failure(ip, now);
    if let Some(session) = env.body.session() {
        let target = lock(&shared.sessions)
            .get(session)
            .map(|r| if r.controller == env.id { r.host.clone() } else { r.controller.clone() });
        if let Some(target) = target {
            lock(&shared.target_limiter).record_failure(&target, now);
        }
    }
}

/// 记录目标失败。
fn record_target_failure(shared: &Shared, target: &str, now: i64) {
    lock(&shared.target_limiter).record_failure(target, now);
}

/// 注册失败时统一返回并计数。
async fn reject_register(
    send: SendStream,
    shared: &Shared,
    ip: &str,
    code: SignalErrorCode,
    message: &str,
) {
    lock(&shared.ip_limiter).record_failure(ip, now_ms());
    let body = SignalMessage::Error {
        code,
        message: message.to_owned(),
    };
    respond(send, server_msg(body)).await;
}

/// 把限流拒绝转换成 error 回包。
fn throttle_error(reject: ThrottleReject) -> SignalEnvelope {
    let message = match reject {
        ThrottleReject::Banned { until_ms } => format!("已封禁至 {until_ms}"),
        ThrottleReject::RetryAfter { retry_after_ms } => {
            format!("退避中，{retry_after_ms} ms 后重试")
        }
    };
    server_msg(SignalMessage::Error {
        code: SignalErrorCode::RateLimited,
        message,
    })
}

/// 双方候选齐备后的 punch 地址：`(主控端首候选, 被控端首候选)`。
type PunchPlan = (String, String);

/// 候选更新结果：`(对端 peer ID, 发送方是否为主控端, 可选 punch 地址)`。
type CandidateOutcome = (String, bool, Option<PunchPlan>);

/// 更新会话候选并判断是否可以下发 punch。
///
/// # Returns
/// `(对端 peer ID, 发送方是否为主控端, 双方首候选地址)`；发送方不是会话成员时返回 `None`。
fn update_candidates(
    shared: &Shared,
    env: &SignalEnvelope,
    session: &str,
    list: Vec<CandidateDesc>,
) -> Option<CandidateOutcome> {
    let mut sessions = lock(&shared.sessions);
    let record = sessions.get_mut(session)?;
    let is_controller = record.controller == env.id;
    let is_host = record.host == env.id;
    if !is_controller && !is_host {
        return None;
    }
    let peer_id = if is_controller {
        record.host.clone()
    } else {
        record.controller.clone()
    };
    if is_controller {
        record.controller_candidates = Some(list);
    } else {
        record.host_candidates = Some(list);
    }
    let punch = match (&record.controller_candidates, &record.host_candidates) {
        (Some(c), Some(h)) => {
            let controller_addr = c.first().map(|x| x.addr.clone()).unwrap_or_default();
            let host_addr = h.first().map(|x| x.addr.clone()).unwrap_or_default();
            Some((controller_addr, host_addr))
        }
        _ => None,
    };
    Some((peer_id, is_controller, punch))
}

/// 向某 peer 推送一条消息。
///
/// # Returns
/// 已写入推送队列返回 `true`；对端不在线、连接关闭或队列已满返回 `false`。
fn push_to_peer(shared: &Shared, peer_id: &str, env: SignalEnvelope) -> bool {
    let tx = lock(&shared.registry).get(peer_id).map(|p| p.push_tx.clone());
    match tx {
        Some(tx) => match tx.try_send(env) {
            Ok(()) => true,
            Err(mpsc::error::TrySendError::Full(_)) => {
                tracing::warn!(peer = %peer_id, capacity = PUSH_CHANNEL_CAPACITY, "推送队列已满，丢弃信令消息");
                false
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        },
        None => false,
    }
}

/// 连接断开后清理该连接的 peer 与会话。
fn cleanup_connection(shared: &Shared, conn_id: usize) {
    let removed: Vec<String> = {
        let mut registry = lock(&shared.registry);
        let ids: Vec<String> = registry
            .iter()
            .filter(|(_, peer)| peer.conn_id == conn_id)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &ids {
            registry.remove(id);
        }
        ids
    };
    let mut sessions = lock(&shared.sessions);
    sessions.retain(|_, record| {
        !removed.contains(&record.controller) && !removed.contains(&record.host)
    });
}

// =====** 帧与信封辅助 =====**

/// 读取一帧：4 字节大端长度 + JSON。
async fn read_frame(recv: &mut RecvStream) -> Result<Vec<u8>, String> {
    let mut len = [0u8; 4];
    recv.read_exact(&mut len)
        .await
        .map_err(|e| format!("读取长度失败: {e}"))?;
    let size = u32::from_be_bytes(len) as usize;
    if size == 0 || size > MAX_FRAME_BYTES {
        return Err(format!("帧长度非法: {size}"));
    }
    let mut buf = vec![0u8; size];
    recv.read_exact(&mut buf)
        .await
        .map_err(|e| format!("读取报文失败: {e}"))?;
    Ok(buf)
}

/// 写入一帧：4 字节大端长度 + JSON。
async fn write_frame(send: &mut SendStream, bytes: &[u8]) -> Result<(), String> {
    send.write_all(&(bytes.len() as u32).to_be_bytes())
        .await
        .map_err(|e| format!("写入长度失败: {e}"))?;
    send.write_all(bytes)
        .await
        .map_err(|e| format!("写入报文失败: {e}"))
}

/// 发送一条响应并关闭发送半边（drop 时自动关闭）。
async fn respond(mut send: SendStream, env: SignalEnvelope) {
    if let Ok(bytes) = env.encode() {
        if let Err(err) = write_frame(&mut send, &bytes).await {
            tracing::debug!(peer = %env.id, error = %err, "响应写入失败，对端可能已断开");
        }
    }
}

/// 构造服务端消息信封（无签名，客户端不校验服务端 HMAC）。
fn server_msg(body: SignalMessage) -> SignalEnvelope {
    SignalEnvelope::new("server", 0, now_ms(), body)
}

/// 取当前毫秒时间戳。
fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// 加锁并在 mutex 中毒时继续使用内部数据（服务端不应因单任务 panic 全面停止）。
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

/// 生成 32 字节随机 token（Base64）。
fn random_token() -> String {
    let mut raw = [0u8; 32];
    OsRng.fill_bytes(&mut raw);
    STANDARD.encode(raw)
}

/// 生成 64 位随机会话 ID。
fn random_session_id() -> String {
    format!("{:016x}", OsRng.next_u64())
}
