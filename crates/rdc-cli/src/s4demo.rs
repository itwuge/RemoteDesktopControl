//! rdc-cli / S4 功能层演示
//!
//! 层级：L4 验证入口
//! 用途：把 `rdc-core::s4` 的输入/剪贴板/文件任务接到真实会话上，一条命令跑通
//!       `demo-input` / `demo-keys` / `demo-clipboard` / `demo-file`。
//! 调用方：`rdc-cli` 子命令；超时由 main.rs 统一兜底。
//!
//! 形态：单进程内双角色（`tokio::join!`），一条 QUIC 直连 + AEAD 控制流，
//!       不创建子进程、不打开窗口，结束后自动断开，无残留。

use std::error::Error;
use std::net::SocketAddr;
use std::time::{Duration, Instant};

use rdc_connect::{dial, establish, listen, DirectListener, EstablishedSession, QuicSignalClient};
use rdc_input::{create_injector, InputEvent, PointerEvent, Viewport};
use rdc_protocol::{
    CallMode, CandidateDesc, CandidateProto, Capabilities, SignalMessage, SignalPath,
};
use rdc_security::{hmac_sha256_b64, IdentityKeyPair, PairingCode, SessionHandshake};
use rdc_signal::SignalServer;
use windows::Win32::Foundation::POINT;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetSystemMetrics, SetCursorPos, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
};

// =====** 类型与常量 =====**

/// 演示统一错误类型（必须可跨 `tokio::join!` 分支传播）。
type DemoResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

/// demo-input 的事件条数。
const INPUT_EVENTS: usize = 100;
/// demo-input 每条注入后的等待（让系统处理输入队列后再采样光标）。
const INPUT_PACE: Duration = Duration::from_millis(6);
/// 鼠标坐标允许的最大偏差（像素）。
const INPUT_TOLERANCE_PX: i32 = 2;

// =====** 会话配对 =====**

/// 信令夹具：服务地址与证书（同一进程内临时自签）。
struct SignalFixture {
    /// 信令服务地址。
    addr: SocketAddr,
    /// 证书 DER。
    cert: Vec<u8>,
    /// 私钥 DER。
    key: Vec<u8>,
}

/// 起一个临时信令服务并保留后台任务。
///
/// # Errors
/// 端口绑定失败时返回错误。
async fn signal_fixture() -> DemoResult<SignalFixture> {
    let server = SignalServer::bind("127.0.0.1:0".parse::<SocketAddr>()?).await?;
    let addr = server.addr();
    let cert = server.cert_der().to_vec();
    let key = server.key_der().to_vec();
    tokio::spawn(async move { server.run().await });
    Ok(SignalFixture { addr, cert, key })
}

/// 被控端运行时：监听器与信令客户端必须活到会话结束（端点同生命周期）。
struct HostRuntime {
    /// 直连监听器（保活）。
    _listener: DirectListener,
    /// 信令客户端（保活）。
    client: QuicSignalClient,
    /// 已建立的加密会话。
    secure: EstablishedSession,
}

impl HostRuntime {
    /// 正常关闭：先关会话再关信令，监听器随结构体一起释放。
    fn close(mut self) {
        self.secure.close();
        self.client.close();
    }
}

/// 主控端运行时。
struct CtrlRuntime {
    /// 信令客户端（保活）。
    client: QuicSignalClient,
    /// 已建立的加密会话。
    secure: EstablishedSession,
}

impl CtrlRuntime {
    /// 正常关闭。
    fn close(mut self) {
        self.secure.close();
        self.client.close();
    }
}

/// 被控端建链：监听 → 注册 → 应答 → 候选 → 直连 → 握手。
///
/// # Errors
/// 端口、信令、直连、握手任一步失败时返回错误。
async fn setup_host(
    server_addr: SocketAddr,
    cert: &[u8],
    key: &[u8],
    host_id: IdentityKeyPair,
    code: PairingCode,
) -> DemoResult<HostRuntime> {
    let listener = listen("127.0.0.1:0".parse::<SocketAddr>()?, cert, key).await?;
    let host_addr = listener.local_addr()?;
    let mut client = QuicSignalClient::connect_and_register(
        server_addr,
        cert,
        &host_id,
        "host-01",
        "s4-demo-host",
        "windows",
        Capabilities::DEFAULT,
    )
    .await?;
    let (session, peer_pub) = loop {
        if let SignalMessage::Incoming {
            session, peer_pub, ..
        } = client.recv_push().await?.body
        {
            break (session, peer_pub);
        }
    };
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
    let direct = listener.accept().await?;
    let handshake = SessionHandshake::new_host(session.clone(), host_id, code, peer_pub);
    let secure = establish(direct, handshake, false).await?;
    client.connected(&session, SignalPath::Udp).await?;
    Ok(HostRuntime {
        _listener: listener,
        client,
        secure,
    })
}

/// 主控端建链：注册 → 呼叫 → 候选 → punch → 拨号 → 握手。
///
/// # Errors
/// 信令、拨号、握手任一步失败时返回错误。
async fn setup_ctrl(
    server_addr: SocketAddr,
    cert: &[u8],
    ctrl_id: IdentityKeyPair,
    code: PairingCode,
    code_proof: String,
) -> DemoResult<CtrlRuntime> {
    let mut client = QuicSignalClient::connect_and_register(
        server_addr,
        cert,
        &ctrl_id,
        "ctrl-01",
        "s4-demo-ctrl",
        "windows",
        Capabilities::DEFAULT,
    )
    .await?;
    let reply = client
        .call("host-01", CallMode::Code, 0, &code_proof)
        .await?;
    let session = match reply.body {
        SignalMessage::Accept { session, .. } => session,
        other => return Err(format!("呼叫响应类型非法: {}", other.type_name()).into()),
    };
    client.candidates(&session, Vec::new()).await?;
    let peer_addr = loop {
        if let SignalMessage::Punch { peer_addr, .. } = client.recv_push().await?.body {
            break peer_addr;
        }
    };
    let peer_addr: SocketAddr = peer_addr.parse()?;
    let direct = dial(peer_addr, cert).await?;
    let handshake = SessionHandshake::new_controller(session.clone(), ctrl_id, code);
    let secure = establish(direct, handshake, true).await?;
    client.connected(&session, SignalPath::Udp).await?;
    Ok(CtrlRuntime { client, secure })
}

// =====** demo-input =====**

/// 输入通道演示：ctrl 发 100 条移动指令，host 注入并用 `GetCursorPos` 逐条比对。
///
/// # Returns
/// 全部注入且最大偏差 ≤ [`INPUT_TOLERANCE_PX`] 时返回 `Ok(())`。
///
/// # Errors
/// 会话、注入、坐标采样失败或偏差超限时返回错误。
pub async fn run_input() -> DemoResult<()> {
    let start = Instant::now();
    enable_dpi_awareness();
    let origin = cursor_pos().ok_or("GetCursorPos 读取初始位置失败")?;
    let view = virtual_desktop();
    println!(
        "[input] 虚拟桌面 {}x{} @({},{})；光标原位 ({},{})",
        view.width, view.height, view.x, view.y, origin.0, origin.1
    );

    let targets = generate_targets(view, INPUT_EVENTS);
    let fixture = signal_fixture().await?;
    let code = PairingCode::generate();
    let host_id = IdentityKeyPair::generate();
    let ctrl_id = IdentityKeyPair::generate();
    let code_proof = hmac_sha256_b64(code.as_str().as_bytes(), b"rdc-s4");
    let (host_rt, ctrl_rt) = tokio::join!(
        setup_host(
            fixture.addr,
            &fixture.cert,
            &fixture.key,
            host_id,
            code.clone()
        ),
        setup_ctrl(fixture.addr, &fixture.cert, ctrl_id, code, code_proof)
    );
    let mut host_rt = host_rt?;
    let mut ctrl_rt = ctrl_rt?;
    println!("[input] 会话就绪 +{} ms", start.elapsed().as_millis());

    let host_view = view;
    let host_fut = async {
        let mut samples: Vec<(i32, i32)> = Vec::with_capacity(INPUT_EVENTS);
        let result = match create_injector() {
            Ok(mut injector) => {
                injector.set_viewport(host_view);
                rdc_core::s4::input::run_host(
                    &mut host_rt.secure,
                    &mut *injector,
                    INPUT_EVENTS as u32,
                    Some(INPUT_PACE),
                    |event| {
                        if let InputEvent::Pointer(_) = event {
                            if let Some(pos) = cursor_pos() {
                                samples.push(pos);
                            }
                        }
                    },
                )
                .await
            }
            Err(err) => Err(rdc_core::s4::S4Error::Input(err)),
        };
        (result, samples)
    };

    let ctrl_events = targets
        .iter()
        .map(|&(x, y)| {
            InputEvent::Pointer(PointerEvent {
                x,
                y,
                button: None,
                pressed: false,
            })
        })
        .collect::<Vec<_>>();
    let ctrl_fut = async { rdc_core::s4::input::run_ctrl(&mut ctrl_rt.secure, &ctrl_events).await };

    let (host_result, ctrl_result) = tokio::join!(host_fut, ctrl_fut);
    host_rt.close();
    ctrl_rt.close();
    let restored = set_cursor_pos(origin);

    let (host_result, samples) = host_result;
    let injected = host_result.map_err(|e| e.to_string())?;
    let counters = ctrl_result.map_err(|e| e.to_string())?;

    // 逐条比对：样本按事件顺序采集，每样本对应同下标的指令。
    let mut max_dev = 0i32;
    let mut matched = 0usize;
    for (target, sample) in targets.iter().zip(samples.iter()) {
        let dx = (target.0 - sample.0).abs();
        let dy = (target.1 - sample.1).abs();
        max_dev = max_dev.max(dx.max(dy));
        if dx <= INPUT_TOLERANCE_PX && dy <= INPUT_TOLERANCE_PX {
            matched += 1;
        }
    }

    println!(
        "[input] 指令 {} 条，host 注入 {} 条，串行确认 {} 条，采样 {} 个",
        INPUT_EVENTS,
        injected,
        counters.injected,
        samples.len()
    );
    println!(
        "[input] 坐标比对：成功 {matched}/{}，最大偏差 {max_dev} px（阈值 ≤ {INPUT_TOLERANCE_PX} px）",
        targets.len()
    );
    println!(
        "[input] 光标复位：{} +{} ms",
        if restored { "成功" } else { "失败" },
        start.elapsed().as_millis()
    );

    let mut failures = Vec::new();
    if injected as usize != INPUT_EVENTS
        || counters.injected as usize != INPUT_EVENTS
        || samples.len() != INPUT_EVENTS
    {
        failures.push("注入条数不足");
    }
    if matched != INPUT_EVENTS {
        failures.push("坐标偏差超限");
    }
    if !restored {
        failures.push("光标未复位");
    }
    if failures.is_empty() {
        println!("[input] PASS +{} ms", start.elapsed().as_millis());
        Ok(())
    } else {
        Err(format!("[input] FAIL: {}", failures.join("、")).into())
    }
}

// =====** 平台小工具 =====**

/// 设置 Per-Monitor V2 DPI 感知；已设置过时忽略错误。
fn enable_dpi_awareness() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

/// 读取当前虚拟桌面（物理像素，原点可为负）。
fn virtual_desktop() -> Viewport {
    unsafe {
        Viewport {
            x: GetSystemMetrics(SM_XVIRTUALSCREEN),
            y: GetSystemMetrics(SM_YVIRTUALSCREEN),
            width: GetSystemMetrics(SM_CXVIRTUALSCREEN).max(1) as u32,
            height: GetSystemMetrics(SM_CYVIRTUALSCREEN).max(1) as u32,
            scale: 1.0,
        }
    }
}

/// 读取光标屏幕坐标。
fn cursor_pos() -> Option<(i32, i32)> {
    let mut point = POINT::default();
    unsafe {
        GetCursorPos(&mut point).ok()?;
    }
    Some((point.x, point.y))
}

/// 把光标移到指定坐标。
fn set_cursor_pos(pos: (i32, i32)) -> bool {
    unsafe { SetCursorPos(pos.0, pos.1).is_ok() }
}

/// 在虚拟桌面内生成 `count` 个互不重复的目标点（留 60 px 边距）。
fn generate_targets(view: Viewport, count: usize) -> Vec<(i32, i32)> {
    let margin = 60i32;
    let usable_w = (view.width as i32 - margin * 2).max(1);
    let usable_h = (view.height as i32 - margin * 2).max(1);
    let mut state = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x5EED);
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let x = view.x + margin + (state >> 33) as i32 % usable_w;
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let y = view.y + margin + (state >> 33) as i32 % usable_h;
        out.push((x, y));
    }
    out
}
