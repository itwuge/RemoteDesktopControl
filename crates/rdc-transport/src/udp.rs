//! rdc-transport / UDP 实际收发
//!
//! 层级：L2 能力层
//! 用途：把 32 字节包头编解码接到真实 socket 上（docs/04 §1）：发送端拼头 + 载荷，
//!       接收端校验头部后拆出载荷。
//! 调用方：rdc-core 的 `rdc-tx` / `rdc-rx`（本模块自身不创建线程），S3b 端到端工具
//!
//! 实现取舍：S3b 用阻塞 socket + 调用方线程，先打通「真实收发 + 重组 + 抖动缓冲」；
//! tokio 化与 AEAD 加密在后续阶段接入，本模块不承担加密职责。

use std::io::ErrorKind;
use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;

use rdc_protocol::PacketHeader;

use crate::error::TransportError;
use crate::packet::HEADER_LEN;

/// 单包接收缓冲上限：1400 字节总长 + 少量余量，避免异常大包撑爆缓冲。
const RECV_BUFFER_LEN: usize = 1500;

// =====** UdpTransport =====**

/// 一个绑定好的 UDP 端点。
pub struct UdpTransport {
    /// 底层 socket；阻塞模式，读超时由 [`UdpTransport::recv`] 控制。
    socket: UdpSocket,
}

impl UdpTransport {
    /// 绑定本地地址。
    ///
    /// # Arguments
    /// * `bind_addr` - 本地地址，例如 `127.0.0.1:0`（0 表示由系统分配端口）
    ///
    /// # Errors
    /// 端口占用、地址非法等 IO 错误返回 [`TransportError::Io`]。
    pub fn bind(bind_addr: &str) -> Result<Self, TransportError> {
        let socket = UdpSocket::bind(bind_addr).map_err(|e| TransportError::Io(e.to_string()))?;
        Ok(Self { socket })
    }

    /// 本端实际地址（含系统分配的端口）。
    pub fn local_addr(&self) -> Result<SocketAddr, TransportError> {
        self.socket
            .local_addr()
            .map_err(|e| TransportError::Io(e.to_string()))
    }

    /// 发送一个带包头的报文。
    ///
    /// # Arguments
    /// * `header` - 已填好 `payload_len` 的 32 字节包头
    /// * `payload` - 明文载荷，长度必须与 `header.payload_len` 一致
    ///
    /// # Returns
    /// 实际写出的总字节数（头 + 载荷）。
    ///
    /// # Errors
    /// 载荷长度与包头声明的长度不一致、包头编码失败或 IO 失败时返回错误。
    pub fn send(&self, header: &PacketHeader, payload: &[u8]) -> Result<usize, TransportError> {
        if payload.len() != header.payload_len as usize {
            return Err(TransportError::Internal(format!(
                "载荷长度 {} 与包头声明 {} 不一致",
                payload.len(),
                header.payload_len
            )));
        }
        let head = header.encode()?;
        let mut datagram = Vec::with_capacity(HEADER_LEN + payload.len());
        datagram.extend_from_slice(&head);
        datagram.extend_from_slice(payload);
        self.socket
            .send(&datagram)
            .map_err(|e| TransportError::Io(e.to_string()))
    }

    /// 发送到指定对端。
    ///
    /// # Arguments
    /// * `peer` - 对端地址
    /// * `header` - 包头
    /// * `payload` - 载荷
    ///
    /// # Returns
    /// 实际写出的总字节数。
    pub fn send_to(
        &self,
        peer: SocketAddr,
        header: &PacketHeader,
        payload: &[u8],
    ) -> Result<usize, TransportError> {
        if payload.len() != header.payload_len as usize {
            return Err(TransportError::Internal(format!(
                "载荷长度 {} 与包头声明 {} 不一致",
                payload.len(),
                header.payload_len
            )));
        }
        let head = header.encode()?;
        let mut datagram = Vec::with_capacity(HEADER_LEN + payload.len());
        datagram.extend_from_slice(&head);
        datagram.extend_from_slice(payload);
        self.socket
            .send_to(&datagram, peer)
            .map_err(|e| TransportError::Io(e.to_string()))
    }

    /// 接收一个报文，最多等待 `timeout`。
    ///
    /// # Arguments
    /// * `timeout` - 读超时；`Duration::ZERO` 表示非阻塞
    ///
    /// # Returns
    /// 收到合法报文时返回 `(包头, 载荷)`；超时返回 `Ok(None)`。
    ///
    /// # Errors
    /// 包头解析失败（魔数/版本/超长）返回 [`TransportError::Protocol`]；
    /// 载荷长度与包头不符返回 [`TransportError::Internal`]。
    ///
    /// 注意：这些错误只影响当前这一个包，调用方应计数后继续收下一个，
    /// 不得因此断开连接（docs/04 §2.4「未知 stream 不报错不断连」同理）。
    pub fn recv(&self, timeout: Duration) -> Result<Option<(PacketHeader, Vec<u8>)>, TransportError> {
        Ok(self
            .recv_from(timeout)?
            .map(|(_, header, payload)| (header, payload)))
    }

    /// 接收一个报文并返回对端地址（握手/学习控制端地址时使用）。
    ///
    /// # Arguments
    /// * `timeout` - 读超时；`Duration::ZERO` 表示非阻塞
    ///
    /// # Returns
    /// 收到合法报文时返回 `(对端地址, 包头, 载荷)`；超时返回 `Ok(None)`。
    ///
    /// # Errors
    /// 同 [`UdpTransport::recv`]。
    pub fn recv_from(
        &self,
        timeout: Duration,
    ) -> Result<Option<(SocketAddr, PacketHeader, Vec<u8>)>, TransportError> {
        self.socket
            .set_read_timeout(if timeout.is_zero() { None } else { Some(timeout) })
            .map_err(|e| TransportError::Io(e.to_string()))?;
        let mut buffer = [0u8; RECV_BUFFER_LEN];
        let (len, peer) = match self.socket.recv_from(&mut buffer) {
            Ok(value) => value,
            Err(e)
                if matches!(
                    e.kind(),
                    ErrorKind::WouldBlock | ErrorKind::TimedOut | ErrorKind::Interrupted
                ) =>
            {
                return Ok(None)
            }
            Err(e) => return Err(TransportError::Io(e.to_string())),
        };
        if len < HEADER_LEN {
            return Err(TransportError::Protocol(
                rdc_protocol::ProtocolError::PacketTooShort {
                    actual: len,
                    min: HEADER_LEN,
                },
            ));
        }
        let header = PacketHeader::decode(&buffer[..len])?;
        header.ensure_supported()?;
        let payload_len = header.payload_len as usize;
        if HEADER_LEN + payload_len > len {
            return Err(TransportError::Internal(format!(
                "载荷声明 {payload_len} 超出实际包长 {}",
                len - HEADER_LEN
            )));
        }
        Ok(Some((
            peer,
            header,
            buffer[HEADER_LEN..HEADER_LEN + payload_len].to_vec(),
        )))
    }
}

// =====** 自检 =====**

#[cfg(test)]
mod tests {
    use super::*;
    use rdc_protocol::StreamKind;

    /// 回环收发必须逐字节还原包头与载荷。
    #[test]
    fn loopback_roundtrip() {
        let sender = UdpTransport::bind("127.0.0.1:0").expect("绑定发送端");
        let receiver = UdpTransport::bind("127.0.0.1:0").expect("绑定接收端");
        let peer = receiver.local_addr().expect("接收端地址");
        let payload = vec![7u8; 100];
        let header = PacketHeader::new(9, StreamKind::VideoMain.as_u8(), 3, 1234, 5, 100)
            .expect("合法包头");
        sender.send_to(peer, &header, &payload).expect("发送成功");
        let (got_header, got_payload) = receiver
            .recv(Duration::from_millis(200))
            .expect("接收成功")
            .expect("应有报文");
        assert_eq!(got_header, header);
        assert_eq!(got_payload, payload);
    }

    /// 载荷长度与包头不一致时必须拒绝发送，避免接收端按错误长度切包。
    #[test]
    fn mismatched_payload_len_is_rejected() {
        let sender = UdpTransport::bind("127.0.0.1:0").expect("绑定");
        let header = PacketHeader::new(1, 0, 0, 0, 0, 100).expect("合法包头");
        assert!(sender.send(&header, &[0u8; 99]).is_err());
    }

    /// 没有报文时按超时返回 None，而不是永久阻塞。
    #[test]
    fn timeout_returns_none() {
        let receiver = UdpTransport::bind("127.0.0.1:0").expect("绑定");
        let start = std::time::Instant::now();
        assert!(receiver.recv(Duration::from_millis(50)).expect("超时不是错误").is_none());
        assert!(start.elapsed() < Duration::from_millis(500));
    }

    /// 魔数错误的报文必须报协议错误，供上层计数丢弃。
    #[test]
    fn bad_magic_is_reported() {
        let receiver = UdpTransport::bind("127.0.0.1:0").expect("绑定");
        let peer_addr = receiver.local_addr().expect("地址");
        let sender = UdpTransport::bind("127.0.0.1:0").expect("绑定发送");
        sender
            .socket
            .send_to(&[0u8; 64], peer_addr)
            .expect("直接发原始字节");
        match receiver.recv(Duration::from_millis(200)) {
            Err(TransportError::Protocol(_)) => {}
            other => panic!("应返回协议错误，实际 {other:?}"),
        }
    }
}
