//! rdc-cli / S2 本机回环 Demo
//!
//! 层级：L4 验证入口
//! 用途：在同一进程/机器上启动信令服务端 + 两个 QUIC 客户端实例，完成
//!       注册 → 呼叫 → 应答 → 候选交换 → 回环直连 → 安全握手 → 加密控制消息 → 断开，
//!       并按步骤打印耗时与 SAS 比对结果。
//! 调用方：`rdc-cli demo` 子命令。

use std::error::Error;
use std::net::SocketAddr;
use std::time::Instant;

use rdc_connect::{dial, establish, listen, QuicSignalClient};
use rdc_protocol::{
    CallMode, CandidateDesc, CandidateProto, Capabilities, SignalMessage, SignalPath,
};
use rdc_security::{
    hmac_sha256_b64, IdentityKeyPair, PairingCode, SessionHandshake,
};
use rdc_signal::SignalServer;

// =====** 类型别名 =====**

/// Demo 使用的错误类型：必须 `Send + Sync` 才能跨 `tokio::join!` 分支传播。
type DemoResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

// =====** 入口 =====**

/// 运行完整回环 Demo。
///
/// # Returns
/// 全部步骤成功返回 `Ok(())`；任一步失败返回带描述的 Demo 错误。
///
/// # Errors
/// 端口绑定、信令请求、直连、握手或控制消息失败时返回错误；
/// 超过 30 s 未完成时返回“Demo 超时”。
pub async fn run() -> DemoResult<()> {
    let start = Instant::now();
    println!("== RDC S2 本机回环 Demo ==");
    println!("步骤耗时（相对启动）：");

    // 1) 信令服务端：127.0.0.1:0 由系统分配端口，使用自签证书。
    let server = SignalServer::bind("127.0.0.1:0".parse::<SocketAddr>()?).await?;
    let server_addr = server.addr();
    let cert = server.cert_der().to_vec();
    let key = server.key_der().to_vec();
    let _server_task = tokio::spawn(async move { server.run().await });
    println!("  [1] 信令服务 udp/{server_addr} 就绪 +{} ms", start.elapsed().as_millis());

    // 2) 一次性配对码：两端共享，PAKE 口令来源。
    let code = PairingCode::generate();
    println!(
        "  [2] 一次性配对码 {}（TTL {} ms +{} ms）",
        code.display_grouped(),
        rdc_security::PAIRING_CODE_TTL_MS,
        start.elapsed().as_millis()
    );

    // 3) 两端身份与并发执行。
    let ctrl_id = IdentityKeyPair::generate();
    let host_id = IdentityKeyPair::generate();
    let code_proof = hmac_sha256_b64(code.as_str().as_bytes(), b"rdc-demo");

    let host = host_side(
        server_addr,
        &cert,
        &key,
        host_id,
        code.clone(),
        start,
    );
    let ctrl = controller_side(
        server_addr,
        &cert,
        ctrl_id,
        code,
        code_proof,
        start,
    );

    let (host_result, ctrl_result) = tokio::join!(host, ctrl);
    let host_sas = host_result?;
    let (ctrl_sas, pong_len) = ctrl_result?;

    // 4) 汇总断言：SAS 一致、控制消息往返成功。
    if host_sas != ctrl_sas {
        return Err(format!("SAS 不一致: host={host_sas} ctrl={ctrl_sas}").into());
    }
    println!(
        "  [8] SAS 比对一致：{ctrl_sas}；控制消息往返 {} 字节 +{} ms",
        pong_len,
        start.elapsed().as_millis()
    );
    println!(
        "== Demo 成功：配对、直连、加密会话、控制消息、断开全部完成，总耗时 {} ms ==",
        start.elapsed().as_millis()
    );
    Ok(())
}

// =====** 被控端 =====**

/// 被控端流程：注册 → 收 incoming → accept → 上报候选 → 接受直连 → 握手 → 收 ping → 回 pong → 断开。
///
/// # Arguments
/// * `server_addr` - 信令服务地址
/// * `cert` - 自签证书 DER
/// * `key` - 自签私钥 DER
/// * `host_id` - 被控端身份
/// * `code` - 配对码
/// * `start` - Demo 启动时刻
///
/// # Returns
/// 本端 4 位 SAS。
async fn host_side(
    server_addr: SocketAddr,
    cert: &[u8],
    key: &[u8],
    host_id: IdentityKeyPair,
    code: PairingCode,
    start: Instant,
) -> DemoResult<String> {
    // 直连监听：回环场景控制器直接拨号到该地址。
    let listener = listen("127.0.0.1:0".parse::<SocketAddr>()?, cert, key).await?;
    let host_addr = listener.local_addr()?;
    println!("  [host] 直连候选 {host_addr} +{} ms", start.elapsed().as_millis());

    let mut client = QuicSignalClient::connect_and_register(
        server_addr,
        cert,
        &host_id,
        "host-01",
        "loopback-host",
        "windows",
        Capabilities::DEFAULT,
    )
    .await?;
    println!("  [host] 注册完成 +{} ms", start.elapsed().as_millis());

    // 等待 incoming（跳过无关推送）。
    let (session, peer_pub) = loop {
        let env = client.recv_push().await?;
        if let SignalMessage::Incoming {
            session,
            peer_pub,
            ..
        } = env.body
        {
            break (session, peer_pub);
        }
    };
    println!("  [host] 收到 incoming session={session} +{} ms", start.elapsed().as_millis());

    client.accept(&session).await?;
    client
        .candidates(
            &session,
            vec![CandidateDesc {
                addr: host_addr.to_string(),
                proto: CandidateProto::Udp,
                priority: 0,
            }],
        )
        .await?;
    println!("  [host] 已应答并上报候选 +{} ms", start.elapsed().as_millis());

    let direct = listener.accept().await?;
    println!("  [host] 直连已建立 +{} ms", start.elapsed().as_millis());

    let handshake = SessionHandshake::new_host(session.clone(), host_id, code, peer_pub);
    let mut session_secure = establish(direct, handshake, false).await?;
    let sas = session_secure.sas().to_owned();
    println!("  [host] 安全握手完成 SAS={sas} +{} ms", start.elapsed().as_millis());

    let ping = session_secure.recv_control().await?;
    println!(
        "  [host] 收到控制消息 `{}` +{} ms",
        String::from_utf8_lossy(&ping),
        start.elapsed().as_millis()
    );
    session_secure.send_control(b"pong: loopback-ok").await?;

    client.connected(&session, SignalPath::Udp).await?;
    client.bye(&session, "demo done").await?;
    session_secure.close();
    client.close();
    println!("  [host] 已正常断开 +{} ms", start.elapsed().as_millis());
    Ok(sas)
}

// =====** 主控端 =====**

/// 主控端流程：注册 → 呼叫 → 等 accept → 上报候选 → 等 punch → 拨号 → 握手 → 发 ping → 收 pong → 断开。
///
/// # Arguments
/// * `server_addr` - 信令服务地址
/// * `cert` - 自签证书 DER
/// * `ctrl_id` - 主控端身份
/// * `code` - 配对码
/// * `code_proof` - 配对码证明（服务端只透传）
/// * `start` - Demo 启动时刻
///
/// # Returns
/// 本端 4 位 SAS 与 pong 明文长度。
async fn controller_side(
    server_addr: SocketAddr,
    cert: &[u8],
    ctrl_id: IdentityKeyPair,
    code: PairingCode,
    code_proof: String,
    start: Instant,
) -> DemoResult<(String, usize)> {
    let mut client = QuicSignalClient::connect_and_register(
        server_addr,
        cert,
        &ctrl_id,
        "ctrl-01",
        "loopback-controller",
        "windows",
        Capabilities::DEFAULT,
    )
    .await?;
    println!("  [ctrl] 注册完成 +{} ms", start.elapsed().as_millis());

    let reply = client
        .call("host-01", CallMode::Code, 0, &code_proof)
        .await?;
    let session = match reply.body {
        SignalMessage::Accept { session, .. } => session,
        other => return Err(format!("呼叫响应类型非法: {}", other.type_name()).into()),
    };
    println!("  [ctrl] 呼叫已被接受 session={session} +{} ms", start.elapsed().as_millis());

    // 主控端不监听入站端口，上报空列表；服务端仍会按被控端候选下发 punch。
    client.candidates(&session, Vec::new()).await?;

    // 等待 punch 提示（跳过转发的 candidates）。
    let peer_addr = loop {
        let env = client.recv_push().await?;
        if let SignalMessage::Punch { peer_addr, .. } = env.body {
            break peer_addr;
        }
    };
    let peer_addr: SocketAddr = peer_addr.parse()?;
    println!("  [ctrl] 收到 punch 对端={peer_addr} +{} ms", start.elapsed().as_millis());

    let direct = dial(peer_addr, cert).await?;
    println!("  [ctrl] 回环直连完成 +{} ms", start.elapsed().as_millis());

    let handshake = SessionHandshake::new_controller(session.clone(), ctrl_id, code);
    let mut session_secure = establish(direct, handshake, true).await?;
    let sas = session_secure.sas().to_owned();
    println!("  [ctrl] 安全握手完成 SAS={sas} +{} ms", start.elapsed().as_millis());

    session_secure.send_control(b"ping: hello-loopback").await?;
    let pong = session_secure.recv_control().await?;
    println!("  [ctrl] 收到 pong `{}` +{} ms", String::from_utf8_lossy(&pong), start.elapsed().as_millis());
    if pong != b"pong: loopback-ok" {
        return Err("控制消息内容不匹配".into());
    }
    let pong_len = pong.len();

    client.connected(&session, SignalPath::Udp).await?;
    let bye = client.bye(&session, "demo done").await?;
    if !matches!(bye.body, SignalMessage::Bye { .. }) {
        return Err("bye 未得到确认".into());
    }
    session_secure.close();
    client.close();
    println!("  [ctrl] 已正常断开 +{} ms", start.elapsed().as_millis());
    Ok((sas, pong_len))
}
