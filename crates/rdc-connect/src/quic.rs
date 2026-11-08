//! rdc-connect / QUIC 帧读写与端点构造
//!
//! 层级：L2 能力层（直连会话与信令客户端共用的传输底座）
//! 用途：统一“4 字节长度 + 载荷”的流帧格式；为客户端/服务端构造
//!       TLS 1.3 over QUIC 端点（信任根为 Demo 自签证书）。
//! 调用方：rdc-connect::signal_client、rdc-connect::session、rdc-cli::demo
//!
//! 安全前提：所有端点必须启用 TLS 1.3；客户端只信任传入的证书 DER，
//! 不做系统根证书回退，避免把自签 Demo 意外暴露给公网中间人。

use std::net::SocketAddr;
use std::sync::Arc;

use quinn::{Connection, Endpoint, RecvStream, SendStream};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rdc_protocol::ProtocolError;
use rdc_transport::TransportError;

use crate::ConnectError;

// =====** 常量 =====**

/// 信令/握手帧上限：与 4 KB 信令包一致，防止对端用超大长度拖垮内存。
pub const MAX_FRAME_LEN: usize = rdc_protocol::message::MAX_SIGNAL_PACKET;

/// TLS 服务端名称（必须与自签证书 SAN 一致）。
pub const TLS_SERVER_NAME: &str = "rdc.local";

// =====** 错误辅助 =====**

/// 把任意传输/IO 错误收敛为 [`ConnectError::Transport`]。
///
/// # Arguments
/// * `context` - 出错动作的可读描述
/// * `err` - 底层错误
///
/// # Returns
/// 带上下文的传输错误。
///
/// # Errors
/// 不会失败。
pub fn transport_error(context: &str, err: impl std::fmt::Display) -> ConnectError {
    ConnectError::Transport(TransportError::Internal(format!("{context}: {err}")))
}

// =====** 时间辅助 =====**

/// 取当前毫秒时间戳。
///
/// # Returns
/// 自 UNIX_EPOCH 起的毫秒数；系统时钟早于 1970 时返回 0。
///
/// # Errors
/// 不会失败。
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// =====** 端点构造 =====**

/// 构造客户端端点，只信任给定证书。
///
/// # Arguments
/// * `cert_der` - 服务端自签证书 DER（Demo 中由 `SignalServer::cert_der` 提供）
///
/// # Returns
/// 已安装默认客户端配置的端点。
///
/// # Errors
/// 证书解析、TLS 配置或端点绑定失败时返回 [`ConnectError::Transport`]。
pub fn client_endpoint(cert_der: &[u8]) -> Result<Endpoint, ConnectError> {
    // rustls 0.23 要求进程内安装 CryptoProvider；重复安装返回 Err 可忽略。
    let _ = rustls::crypto::ring::default_provider().install_default(); // gate-allow R-C-2: 进程级 CryptoProvider 已装好时返回 Err，重复安装无害

    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from(cert_der.to_vec()))
        .map_err(|e| transport_error("加入信任根", e))?;
    let tls = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls)
        .map_err(|e| transport_error("构造 QUIC 客户端配置", e))?;
    let client_config = quinn::ClientConfig::new(Arc::new(crypto));

    let mut endpoint = Endpoint::client("0.0.0.0:0".parse::<SocketAddr>().expect("固定地址"))
        .map_err(|e| transport_error("绑定客户端端点", e))?;
    endpoint.set_default_client_config(client_config);
    Ok(endpoint)
}

/// 构造服务端端点（自签证书 + 私钥）。
///
/// # Arguments
/// * `addr` - 监听地址
/// * `cert_der` - 证书 DER
/// * `key_der` - PKCS#8 私钥 DER
///
/// # Returns
/// 已绑定端口的服务端端点。
///
/// # Errors
/// TLS 配置或端口绑定失败时返回 [`ConnectError::Transport`]。
pub fn server_endpoint(
    addr: SocketAddr,
    cert_der: &[u8],
    key_der: &[u8],
) -> Result<Endpoint, ConnectError> {
    let _ = rustls::crypto::ring::default_provider().install_default(); // gate-allow R-C-2: 进程级 CryptoProvider 已装好时返回 Err，重复安装无害

    let tls = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(cert_der.to_vec())],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key_der.to_vec())),
        )
        .map_err(|e| transport_error("构造 TLS 服务端配置", e))?;
    let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(tls)
        .map_err(|e| transport_error("构造 QUIC 服务端配置", e))?;
    let server_config = quinn::ServerConfig::with_crypto(Arc::new(crypto));
    Endpoint::server(server_config, addr).map_err(|e| transport_error("绑定服务端端点", e))
}

/// 客户端拨号并完成 QUIC 握手。
///
/// # Arguments
/// * `endpoint` - 已配置信任根的客户端端点
/// * `addr` - 对端地址
///
/// # Returns
/// 已建立的 QUIC 连接。
///
/// # Errors
/// 地址不可达、TLS 校验失败或握手超时返回 [`ConnectError::Transport`]。
pub async fn connect(endpoint: &Endpoint, addr: SocketAddr) -> Result<Connection, ConnectError> {
    let connecting = endpoint
        .connect(addr, TLS_SERVER_NAME)
        .map_err(|e| transport_error("发起连接", e))?;
    connecting
        .await
        .map_err(|e| transport_error("QUIC 握手", e))
}

// =====** 流帧读写 =====**

/// 写入一帧：4 字节大端长度 + 载荷。
///
/// 不关闭发送半边：直连会话在同一流上连续发送多帧，必须由调用方在结束时关闭。
///
/// # Arguments
/// * `send` - 发送流
/// * `bytes` - 载荷（≤ [`MAX_FRAME_LEN`]）
///
/// # Returns
/// 写入成功返回 `Ok(())`。
///
/// # Errors
/// 载荷超长返回 [`ConnectError::Protocol`]；网络写失败返回 [`ConnectError::Transport`]。
pub async fn write_frame(send: &mut SendStream, bytes: &[u8]) -> Result<(), ConnectError> {
    if bytes.len() > MAX_FRAME_LEN {
        return Err(ConnectError::Protocol(ProtocolError::SignalPacketTooLarge {
            actual: bytes.len(),
            max: MAX_FRAME_LEN,
        }));
    }
    send.write_all(&(bytes.len() as u32).to_be_bytes())
        .await
        .map_err(|e| transport_error("写入长度", e))?;
    send.write_all(bytes)
        .await
        .map_err(|e| transport_error("写入载荷", e))?;
    Ok(())
}

/// 读取一帧：4 字节大端长度 + 载荷。
///
/// # Arguments
/// * `recv` - 接收流
///
/// # Returns
/// 载荷字节。
///
/// # Errors
/// 长度为 0 或超过 [`MAX_FRAME_LEN`] 返回 [`ConnectError::Protocol`]；
/// 网络读失败返回 [`ConnectError::Transport`]。
pub async fn read_frame(recv: &mut RecvStream) -> Result<Vec<u8>, ConnectError> {
    let mut len = [0u8; 4];
    recv.read_exact(&mut len)
        .await
        .map_err(|e| transport_error("读取长度", e))?;
    let size = u32::from_be_bytes(len) as usize;
    if size == 0 || size > MAX_FRAME_LEN {
        return Err(ConnectError::Protocol(ProtocolError::SignalPacketTooLarge {
            actual: size,
            max: MAX_FRAME_LEN,
        }));
    }
    let mut buf = vec![0u8; size];
    recv.read_exact(&mut buf)
        .await
        .map_err(|e| transport_error("读取载荷", e))?;
    Ok(buf)
}
