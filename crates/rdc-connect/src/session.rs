//! rdc-connect / 直连会话：QUIC 直连 + 安全握手 + 控制通道 AEAD
//!
//! 层级：L2 能力层
//! 用途：在候选交换/打洞完成后建立点对点 TLS 1.3 over QUIC 连接，运行
//!       docs/07 §2 的 4 报文握手（X25519 + SPAKE2 + HKDF），并用
//!       `ChannelKeys::control` 做 AEAD 控制消息收发，附重放窗口。
//! 调用方：rdc-cli 的本机回环 Demo、后续 rdc-core 的会话建立流程
//!
//! 安全前提：握手失败必须整会话关闭，不可重试；控制帧的明文序号参与 nonce
//! 派生，篡改序号或密文都会被 AEAD 拒绝；重放窗口只在解密成功后推进。

use std::net::SocketAddr;

use quinn::{Connection, Endpoint, RecvStream, SendStream};
use rdc_security::{
    nonce_from_counter, AeadKey, HandshakeMessage, HandshakeRole, ReplayWindow, SecurityError,
    SessionHandshake,
};

use crate::quic::{
    connect, now_ms, read_frame, server_endpoint, transport_error, write_frame, MAX_FRAME_LEN,
};
use crate::ConnectError;

// =====** 直连端点 =====**

/// 直连监听器（被控端）。
pub struct DirectListener {
    /// QUIC 服务端端点。
    endpoint: Endpoint,
}

impl DirectListener {
    /// 返回监听地址（填入候选列表）。
    ///
    /// # Returns
    /// 实际 `ip:port`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn local_addr(&self) -> Result<SocketAddr, ConnectError> {
        self.endpoint
            .local_addr()
            .map_err(|e| transport_error("读取监听地址", e))
    }

    /// 接受一条直连（回环场景即控制器拨号）。
    ///
    /// # Returns
    /// 已建立的直连（含端点所有权）。
    ///
    /// # Errors
    /// TLS/QUIC 握手失败或端点关闭时返回 [`ConnectError::Transport`]。
    pub async fn accept(&self) -> Result<DirectConnection, ConnectError> {
        let incoming = self
            .endpoint
            .accept()
            .await
            .ok_or_else(|| transport_error("接受直连", "端点已关闭"))?;
        let conn = incoming
            .await
            .map_err(|e| transport_error("直连 TLS 握手", e))?;
        Ok(DirectConnection {
            endpoint: self.endpoint.clone(),
            conn,
        })
    }
}

/// 已建立的直连（连接 + 端点所有权）。
pub struct DirectConnection {
    /// QUIC 端点；必须与连接同生命周期。
    endpoint: Endpoint,
    /// QUIC 连接。
    conn: Connection,
}

/// 启动直连监听器（被控端）。
///
/// # Arguments
/// * `addr` - 监听地址；Demo 传 `127.0.0.1:0`
/// * `cert_der` - 自签证书 DER（与信令服务共用同一张证书，便于 Demo 分发信任根）
/// * `key_der` - PKCS#8 私钥 DER
///
/// # Returns
/// 监听器（调用 [`DirectListener::local_addr`] 取候选地址）。
///
/// # Errors
/// TLS 配置或端口绑定失败返回 [`ConnectError::Transport`]。
pub async fn listen(
    addr: SocketAddr,
    cert_der: &[u8],
    key_der: &[u8],
) -> Result<DirectListener, ConnectError> {
    let endpoint = server_endpoint(addr, cert_der, key_der)?;
    Ok(DirectListener { endpoint })
}

/// 拨号直连（主控端）。
///
/// # Arguments
/// * `peer_addr` - 候选地址（回环 Demo 即被控端监听地址）
/// * `cert_der` - 信任根证书 DER
///
/// # Returns
/// 已完成的 QUIC 连接。
///
/// # Errors
/// 地址不可达或 TLS 校验失败返回 [`ConnectError::Transport`]。
pub async fn dial(peer_addr: SocketAddr, cert_der: &[u8]) -> Result<DirectConnection, ConnectError> {
    let endpoint = crate::quic::client_endpoint(cert_der)?;
    let conn = connect(&endpoint, peer_addr).await?;
    Ok(DirectConnection { endpoint, conn })
}

// =====** 安全握手 =====**

/// 已建立的安全会话。
pub struct EstablishedSession {
    /// QUIC 端点。
    endpoint: Endpoint,
    /// QUIC 连接。
    conn: Connection,
    /// 控制流发送半边。
    send: SendStream,
    /// 控制流接收半边。
    recv: RecvStream,
    /// 控制通道 AEAD 密钥。
    key: AeadKey,
    /// 接收重放窗口。
    replay: ReplayWindow,
    /// 发送计数器（主控端从 1 起，被控端从 1<<63 起，避免同密钥 nonce 冲突）。
    tx_counter: u64,
    /// 4 位 SAS。
    sas: String,
    /// 本端角色。
    role: HandshakeRole,
}

impl EstablishedSession {
    /// 返回 4 位 SAS（供用户比对）。
    ///
    /// # Returns
    /// 4 位数字串。
    ///
    /// # Errors
    /// 不会失败。
    pub fn sas(&self) -> &str {
        &self.sas
    }

    /// 返回本端角色。
    ///
    /// # Returns
    /// 主控端或被控端。
    ///
    /// # Errors
    /// 不会失败。
    pub fn role(&self) -> HandshakeRole {
        self.role
    }

    /// 发送一条加密控制消息。
    ///
    /// # Arguments
    /// * `payload` - 明文业务消息
    ///
    /// # Returns
    /// 本次使用的计数器（便于日志与断言）。
    ///
    /// # Errors
    /// AEAD 加密或网络写失败返回 [`ConnectError`]。
    pub async fn send_control(&mut self, payload: &[u8]) -> Result<u64, ConnectError> {
        self.tx_counter = self.tx_counter.wrapping_add(1);
        let counter = self.tx_counter;
        let frame = seal_control(&self.key, counter, payload)?;
        write_frame(&mut self.send, &frame).await?;
        Ok(counter)
    }

    /// 接收一条加密控制消息。
    ///
    /// # Returns
    /// 解密后的明文。
    ///
    /// # Errors
    /// 帧格式非法、AEAD 校验失败或重放时返回 [`ConnectError::Security`] /
    /// [`ConnectError::Protocol`]；网络读失败返回 [`ConnectError::Transport`]。
    pub async fn recv_control(&mut self) -> Result<Vec<u8>, ConnectError> {
        let frame = read_frame(&mut self.recv).await?;
        open_control(&self.key, &mut self.replay, &frame).map_err(ConnectError::from)
    }

    /// 正常关闭会话。
    ///
    /// # Returns
    /// 不会失败。
    ///
    /// # Errors
    /// 不会失败。
    pub fn close(&mut self) {
        let _ = self.send.finish(); // gate-allow R-C-2: 关闭路径不再有可恢复动作，finish 失败由随后的 conn.close 一并收敛
        self.conn.close(quinn::VarInt::from_u32(0), b"session closed");
        self.endpoint.close(quinn::VarInt::from_u32(0), b"session closed");
    }
}

//[握手必须按 docs/07 的报文顺序串行完成] 因此这里用单条双向流，不做并发读。
/// 在已建立的直连上运行安全握手。
///
/// # Arguments
/// * `direct` - 已建立的 QUIC 直连
/// * `handshake` - 已配置角色/配对码/期望公钥的握手状态机
/// * `controller` - 本端是否为主控端
///
/// # Returns
/// 已建立的安全会话（含控制流与通道密钥）。
///
/// # Errors
/// 握手消息非法、验签失败、PAKE 不一致或状态机拒绝时返回 [`ConnectError::Security`]。
pub async fn establish(
    direct: DirectConnection,
    mut handshake: SessionHandshake,
    controller: bool,
) -> Result<EstablishedSession, ConnectError> {
    let DirectConnection { endpoint, conn } = direct;
    let (mut send, mut recv) = if controller {
        conn.open_bi()
            .await
            .map_err(|e| transport_error("打开握手流", e))?
    } else {
        conn.accept_bi()
            .await
            .map_err(|e| transport_error("接受握手流", e))?
    };

    if controller {
        // 主控端必须先发 HELLO；被控端在 Idle 状态等待。
        let hello = handshake
            .start(now_ms())?
            .ok_or_else(|| transport_error("启动握手", "主控端未产生 HELLO"))?;
        write_frame(&mut send, &hello.encode()?).await?;
    }

    loop {
        let frame = read_frame(&mut recv).await?;
        let msg = HandshakeMessage::decode(&frame)?;
        let reply = handshake.step(msg, now_ms())?;
        if let Some(reply) = reply {
            write_frame(&mut send, &reply.encode()?).await?;
        }
        if handshake.is_established() {
            break;
        }
    }

    let keys = handshake
        .keys()
        .ok_or_else(|| transport_error("取通道密钥", "握手未派生密钥"))?;
    let key = AeadKey::new(keys.control);
    let sas = handshake
        .sas()
        .ok_or_else(|| transport_error("取 SAS", "握手未派生 SAS"))?
        .to_owned();
    // 双向共用同一 control 密钥，因此用计数器区间隔离方向：主控端低半区、
    // 被控端高半区，保证同一密钥下 nonce 永不重复。
    let tx_counter = if controller { 1 } else { 1 << 63 };

    Ok(EstablishedSession {
        endpoint,
        conn,
        send,
        recv,
        key,
        replay: ReplayWindow::new(),
        tx_counter,
        sas,
        role: if controller {
            HandshakeRole::Controller
        } else {
            HandshakeRole::Host
        },
    })
}

// =====** 控制帧编解码（可单测） =====**

/// 加密一条控制消息。
///
/// 帧格式：`counter(8 字节大端) ‖ AEAD 密文`。nonce 由 counter 派生，
/// 因此明文序号与密文强绑定：改 counter 会让解密失败。
///
/// # Arguments
/// * `key` - 控制通道密钥
/// * `counter` - 单调递增序号（不得重复）
/// * `payload` - 明文
///
/// # Returns
/// 可直接写入 QUIC 流的帧字节。
///
/// # Errors
/// 载荷超过帧上限或 AEAD 失败返回 [`SecurityError`]。
pub fn seal_control(key: &AeadKey, counter: u64, payload: &[u8]) -> Result<Vec<u8>, SecurityError> {
    if payload.len() + 8 + rdc_security::TAG_LEN > MAX_FRAME_LEN {
        return Err(SecurityError::HandshakeStateInvalid {
            state: format!("控制消息超过 {MAX_FRAME_LEN} 字节"),
        });
    }
    let nonce = nonce_from_counter(counter);
    let ciphertext = key.seal(payload, &nonce)?;
    let mut frame = Vec::with_capacity(8 + ciphertext.len());
    frame.extend_from_slice(&counter.to_be_bytes());
    frame.extend_from_slice(&ciphertext);
    Ok(frame)
}

/// 解密一条控制消息并推进重放窗口。
///
/// # Arguments
/// * `key` - 控制通道密钥
/// * `replay` - 接收重放窗口
/// * `frame` - `counter ‖ 密文`
///
/// # Returns
/// 明文。
///
/// # Errors
/// 帧过短返回 [`SecurityError::AeadFailure`]；AEAD 校验失败返回
/// [`SecurityError::AeadFailure`]；序号重复返回 [`SecurityError::ReplayDetected`]。
pub fn open_control(
    key: &AeadKey,
    replay: &mut ReplayWindow,
    frame: &[u8],
) -> Result<Vec<u8>, SecurityError> {
    if frame.len() < 8 + rdc_security::TAG_LEN {
        return Err(SecurityError::AeadFailure { source: None });
    }
    let counter = u64::from_be_bytes(
        frame[..8]
            .try_into()
            .map_err(|error| SecurityError::AeadFailure {
                source: Some(Box::new(error)),
            })?,
    );
    let nonce = nonce_from_counter(counter);
    // 先解密再推进窗口：伪造包不能把合法序号挤出窗口。
    let plaintext = key.open(&frame[8..], &nonce)?;
    replay.check_and_update(counter)?;
    Ok(plaintext)
}

// =====** 单元测试：篡改与重放 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个确定性控制密钥，避免测试依赖随机数。
    fn test_key() -> AeadKey {
        AeadKey::new([0x11; 32])
    }

    /// 正常往返必须还原明文。
    #[test]
    fn control_roundtrip() {
        let key = test_key();
        let frame = seal_control(&key, 7, b"ping").expect("加密");
        let mut replay = ReplayWindow::new();
        assert_eq!(open_control(&key, &mut replay, &frame).expect("解密"), b"ping");
    }

    /// 篡改密文任意字节必须被 AEAD 拒绝。
    #[test]
    fn tampered_ciphertext_is_rejected() {
        let key = test_key();
        let frame = seal_control(&key, 9, b"secret").expect("加密");
        let mut bad = frame.clone();
        // 改最后一个字节（落在 AEAD tag 内）。
        let last = bad.len() - 1;
        bad[last] ^= 1;
        let mut replay = ReplayWindow::new();
        assert!(matches!(
            open_control(&key, &mut replay, &bad),
            Err(SecurityError::AeadFailure { .. })
        ));
    }

    /// 篡改明文序号必须导致 nonce 不匹配、解密失败。
    #[test]
    fn tampered_counter_is_rejected() {
        let key = test_key();
        let frame = seal_control(&key, 5, b"secret").expect("加密");
        let mut bad = frame.clone();
        bad[..8].copy_from_slice(&6u64.to_be_bytes());
        let mut replay = ReplayWindow::new();
        assert!(matches!(
            open_control(&key, &mut replay, &bad),
            Err(SecurityError::AeadFailure { .. })
        ));
    }

    /// 同一条密文第二次提交必须按重放拒绝。
    #[test]
    fn replay_is_rejected() {
        let key = test_key();
        let frame = seal_control(&key, 11, b"once").expect("加密");
        let mut replay = ReplayWindow::new();
        open_control(&key, &mut replay, &frame).expect("首次合法");
        assert!(matches!(
            open_control(&key, &mut replay, &frame),
            Err(SecurityError::ReplayDetected { .. })
        ));
    }

    /// 过短的帧必须明确拒绝，不能 panic。
    #[test]
    fn short_frame_is_rejected() {
        let key = test_key();
        let mut replay = ReplayWindow::new();
        assert!(matches!(
            open_control(&key, &mut replay, &[0u8; 8]),
            Err(SecurityError::AeadFailure { .. })
        ));
    }
}
