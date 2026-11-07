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
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rdc_connect::{dial, establish, listen, DirectListener, EstablishedSession, QuicSignalClient};
use rdc_input::{create_injector, InputEvent, KeyEvent, PointerEvent, Viewport};
use rdc_protocol::{
    CallMode, CandidateDesc, CandidateProto, Capabilities, SignalMessage, SignalPath,
};
use rdc_security::{hmac_sha256_b64, IdentityKeyPair, PairingCode, SessionHandshake};
use rdc_signal::SignalServer;
use sha2::{Digest, Sha256};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetCursorPos, GetMessageW, GetSystemMetrics, PeekMessageW, PostThreadMessageW,
    SetCursorPos, SetWindowsHookExW, UnhookWindowsHookEx, HHOOK, KBDLLHOOKSTRUCT, LLKHF_INJECTED,
    MSG, PM_NOREMOVE, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN, SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN,
    WH_KEYBOARD_LL, WM_KEYDOWN, WM_KEYUP, WM_QUIT, WM_SYSKEYDOWN, WM_SYSKEYUP,
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
/// 采样到陈旧坐标时的最多重采次数（每次间隔 [`INPUT_SETTLE_STEP`]）。
const INPUT_SETTLE_TRIES: u32 = 12;
/// 重采间隔：系统偶发延迟处理注入输入时，等一次再读。
const INPUT_SETTLE_STEP: Duration = Duration::from_millis(3);

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

/// 建链里程碑（失败时打印整条轨迹，成功时不输出）。
static MILESTONES: Mutex<Vec<(u128, &'static str, &'static str)>> = Mutex::new(Vec::new());
/// 里程碑计时原点。
static MILESTONE_ORIGIN: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// 记录一个建链里程碑（设置 `RDC_S4_DEBUG` 时同步输出到 stderr）。
fn trace(side: &'static str, step: &'static str) {
    let at = MILESTONE_ORIGIN
        .get_or_init(Instant::now)
        .elapsed()
        .as_millis();
    if let Ok(mut list) = MILESTONES.lock() {
        list.push((at, side, step));
    }
    if std::env::var_os("RDC_S4_DEBUG").is_some() {
        eprintln!("[setup] +{at} ms {side}: {step}");
    }
}

/// 把已记录的建链轨迹拼成多行文本（仅在失败时调用）。
fn dump_marks() -> String {
    use std::fmt::Write as _;
    let Ok(list) = MILESTONES.lock() else {
        return "[setup] 建链轨迹不可用".to_string();
    };
    let mut text = String::from("[setup] 建链轨迹:");
    for (at, side, step) in list.iter() {
        let _ = write!(text, "\n  +{at} ms {side}: {step}");
    }
    text
}

/// 被控端建链：监听 → 注册 → 应答 → 候选 → 直连 → 握手。
///
/// # Errors
/// 端口、信令、直连、握手任一步失败时返回错误（失败时会打印建链轨迹）。
async fn setup_host(
    server_addr: SocketAddr,
    cert: &[u8],
    key: &[u8],
    host_id: IdentityKeyPair,
    code: PairingCode,
    registered: Arc<tokio::sync::Notify>,
) -> DemoResult<HostRuntime> {
    let result = setup_host_inner(server_addr, cert, key, host_id, code, registered).await;
    if let Err(err) = &result {
        eprintln!("[setup] 被控端建链失败：{err}\n{}", dump_marks());
    }
    result
}

/// `setup_host` 的实际实现（便于统一打印失败轨迹）。
async fn setup_host_inner(
    server_addr: SocketAddr,
    cert: &[u8],
    key: &[u8],
    host_id: IdentityKeyPair,
    code: PairingCode,
    registered: Arc<tokio::sync::Notify>,
) -> DemoResult<HostRuntime> {
    let listener = listen("127.0.0.1:0".parse::<SocketAddr>()?, cert, key).await?;
    let host_addr = listener.local_addr()?;
    trace("host", "listen 就绪");
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
    trace("host", "信令已注册");
    // 注册已在服务端生效后再放行呼叫方，避免 PeerNotFound 竞态。
    registered.notify_one();
    let (session, peer_pub) = loop {
        if let SignalMessage::Incoming {
            session, peer_pub, ..
        } = client.recv_push().await?.body
        {
            break (session, peer_pub);
        }
    };
    trace("host", "收到 Incoming");
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
    trace("host", "候选已上报");
    let direct = listener.accept().await?;
    trace("host", "直连已接受");
    let handshake = SessionHandshake::new_host(session.clone(), host_id, code, peer_pub);
    let secure = establish(direct, handshake, false).await?;
    trace("host", "握手完成");
    client.connected(&session, SignalPath::Udp).await?;
    trace("host", "已 connected");
    Ok(HostRuntime {
        _listener: listener,
        client,
        secure,
    })
}

/// 主控端建链：注册 → 呼叫 → 候选 → punch → 拨号 → 握手。
///
/// # Errors
/// 信令、拨号、握手任一步失败时返回错误（失败时会打印建链轨迹）。
async fn setup_ctrl(
    server_addr: SocketAddr,
    cert: &[u8],
    ctrl_id: IdentityKeyPair,
    code: PairingCode,
    code_proof: String,
    registered: Arc<tokio::sync::Notify>,
) -> DemoResult<CtrlRuntime> {
    let result = setup_ctrl_inner(server_addr, cert, ctrl_id, code, code_proof, registered).await;
    if let Err(err) = &result {
        eprintln!("[setup] 主控端建链失败：{err}\n{}", dump_marks());
    }
    result
}

/// `setup_ctrl` 的实际实现（便于统一打印失败轨迹）。
async fn setup_ctrl_inner(
    server_addr: SocketAddr,
    cert: &[u8],
    ctrl_id: IdentityKeyPair,
    code: PairingCode,
    code_proof: String,
    registered: Arc<tokio::sync::Notify>,
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
    trace("ctrl", "信令已注册");
    // 等被控端注册在服务端生效后再呼叫，避免 PeerNotFound 竞态。
    tokio::time::timeout(Duration::from_secs(5), registered.notified())
        .await
        .map_err(|_| "等待被控端注册超时")?;
    trace("ctrl", "被控端已注册");
    let reply = client
        .call("host-01", CallMode::Code, 0, &code_proof)
        .await?;
    let session = match reply.body {
        SignalMessage::Accept { session, .. } => session,
        other => return Err(format!("呼叫响应类型非法: {}", other.type_name()).into()),
    };
    trace("ctrl", "收到 Accept");
    client.candidates(&session, Vec::new()).await?;
    let peer_addr = loop {
        if let SignalMessage::Punch { peer_addr, .. } = client.recv_push().await?.body {
            break peer_addr;
        }
    };
    trace("ctrl", "收到 Punch");
    let peer_addr: SocketAddr = peer_addr.parse()?;
    let direct = dial(peer_addr, cert).await?;
    trace("ctrl", "拨号完成");
    let handshake = SessionHandshake::new_controller(session.clone(), ctrl_id, code);
    let secure = establish(direct, handshake, true).await?;
    trace("ctrl", "握手完成");
    client.connected(&session, SignalPath::Udp).await?;
    trace("ctrl", "已 connected");
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
    let ready = Arc::new(tokio::sync::Notify::new());
    let (host_rt, ctrl_rt) = tokio::join!(
        setup_host(
            fixture.addr,
            &fixture.cert,
            &fixture.key,
            host_id,
            code.clone(),
            ready.clone()
        ),
        setup_ctrl(
            fixture.addr,
            &fixture.cert,
            ctrl_id,
            code,
            code_proof,
            ready.clone()
        )
    );
    let mut host_rt = host_rt?;
    let mut ctrl_rt = ctrl_rt?;
    println!("[input] 会话就绪 +{} ms", start.elapsed().as_millis());

    let host_view = view;
    let host_fut = async {
        let mut samples: Vec<(i32, i32)> = Vec::with_capacity(INPUT_EVENTS);
        let mut settle_retries = 0usize;
        let result = match create_injector() {
            Ok(mut injector) => {
                injector.set_viewport(host_view);
                rdc_core::s4::input::run_host(
                    &mut host_rt.secure,
                    &mut *injector,
                    INPUT_EVENTS as u32,
                    Some(INPUT_PACE),
                    |event| {
                        if let InputEvent::Pointer(pointer) = event {
                            let target = (pointer.x, pointer.y);
                            let in_tolerance = |pos: Option<(i32, i32)>| match pos {
                                Some((x, y)) => {
                                    (x - target.0).abs() <= INPUT_TOLERANCE_PX
                                        && (y - target.1).abs() <= INPUT_TOLERANCE_PX
                                }
                                None => false,
                            };
                            // 注入后系统可能还没处理完，采到的是上一条目标的坐标；
                            // 在有限窗口内重采，记录“稳定后”的位置。
                            let mut sampled = cursor_pos();
                            let mut tries = 0;
                            while !in_tolerance(sampled) && tries < INPUT_SETTLE_TRIES {
                                std::thread::sleep(INPUT_SETTLE_STEP);
                                sampled = cursor_pos();
                                tries += 1;
                            }
                            if tries > 0 {
                                settle_retries += 1;
                            }
                            if let Some(pos) = sampled {
                                samples.push(pos);
                            }
                        }
                    },
                )
                .await
            }
            Err(err) => Err(rdc_core::s4::S4Error::Input(err)),
        };
        (result, samples, settle_retries)
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

    let (host_result, samples, settle_retries) = host_result;
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
        "[input] 坐标比对：成功 {matched}/{}，最大偏差 {max_dev} px（阈值 ≤ {INPUT_TOLERANCE_PX} px），\
         重采 {settle_retries} 条",
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

// =====** demo-keys =====**

/// demo-keys 使用的安全扫描码（A/B/C，不含任何修饰键）。
const KEY_SCAN_CODES: [u16; 6] = [0x1E, 0x30, 0x2E, 0x1E, 0x30, 0x2E];
/// demo-keys 每条注入后的等待。
const KEY_PACE: Duration = Duration::from_millis(8);

/// 一条低级钩子记录（只收注入事件）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct KeyRecord {
    /// 扫描码。
    scan_code: u16,
    /// 是否按下（否则为抬起）。
    down: bool,
    /// 是否带注入标记（本演示应恒为 true）。
    injected: bool,
}

/// 钩子回调写入的记录缓冲。
static KEY_EVENTS: Mutex<Vec<KeyRecord>> = Mutex::new(Vec::new());

/// 低级键盘钩子线程：安装 → 消息泵 → 收到 `WM_QUIT` 后卸载并退出。
struct KeyboardHook {
    /// 钩子线程。
    thread: JoinHandle<()>,
    /// 线程 id（用于投递 `WM_QUIT`）。
    thread_id: u32,
}

impl KeyboardHook {
    /// 安装 `WH_KEYBOARD_LL` 钩子并等待线程就绪。
    ///
    /// # Errors
    /// 线程创建失败或钩子安装失败（5 s 内未就绪）时返回错误。
    fn install() -> DemoResult<Self> {
        let (tx, rx) = mpsc::channel::<(u32, bool)>();
        let thread = std::thread::Builder::new()
            .name("s4-key-hook".into())
            .spawn(move || unsafe {
                let mut msg = MSG::default();
                // 先创建消息队列，保证主线程的 PostThreadMessage 不会落空。
                let _ = PeekMessageW(&mut msg, HWND(0), 0, 0, PM_NOREMOVE);
                let thread_id = GetCurrentThreadId();
                match SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), HINSTANCE(0), 0) {
                    Ok(hook) => {
                        let _ = tx.send((thread_id, true));
                        loop {
                            let ret = GetMessageW(&mut msg, HWND(0), 0, 0);
                            if ret.0 == 0 || ret.0 == -1 {
                                break;
                            }
                        }
                        let _ = UnhookWindowsHookEx(hook);
                    }
                    Err(_) => {
                        let _ = tx.send((thread_id, false));
                    }
                }
            })?;
        let (thread_id, installed) = rx.recv_timeout(Duration::from_secs(5))?;
        if !installed {
            return Err("SetWindowsHookExW(WH_KEYBOARD_LL) 失败".into());
        }
        Ok(Self { thread, thread_id })
    }

    /// 停止钩子线程并取走记录（保证线程 join，无残留）。
    fn stop(self) -> Vec<KeyRecord> {
        unsafe {
            let _ = PostThreadMessageW(self.thread_id, WM_QUIT, WPARAM(0), LPARAM(0));
        }
        let _ = self.thread.join();
        std::mem::take(&mut *KEY_EVENTS.lock().expect("钩子记录锁"))
    }
}

/// 低级键盘钩子过程：只记录带 `LLKHF_INJECTED` 的按下/抬起。
unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        let info = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
        if info.flags.0 & LLKHF_INJECTED.0 != 0 {
            let message = wparam.0 as u32;
            if message == WM_KEYDOWN
                || message == WM_KEYUP
                || message == WM_SYSKEYDOWN
                || message == WM_SYSKEYUP
            {
                if let Ok(mut events) = KEY_EVENTS.lock() {
                    events.push(KeyRecord {
                        scan_code: info.scanCode as u16,
                        down: message == WM_KEYDOWN || message == WM_SYSKEYDOWN,
                        injected: true,
                    });
                }
            }
        }
    }
    CallNextHookEx(HHOOK(0), code, wparam, lparam)
}

/// 按键通道演示：注入 A/B/C 的按下/抬起，低级钩子逐条比对。
///
/// # Returns
/// 记录条数、顺序与注入标记全部一致时返回 `Ok(())`。
///
/// # Errors
/// 钩子安装、会话、注入失败或比对不一致时返回错误。
pub async fn run_keys() -> DemoResult<()> {
    let start = Instant::now();
    KEY_EVENTS.lock().expect("钩子记录锁").clear();
    let hook = KeyboardHook::install()?;
    let mut events = Vec::with_capacity(KEY_SCAN_CODES.len() * 2);
    for scan_code in KEY_SCAN_CODES {
        events.push(InputEvent::Key(KeyEvent {
            scan_code,
            pressed: true,
            modifiers: 0,
        }));
        events.push(InputEvent::Key(KeyEvent {
            scan_code,
            pressed: false,
            modifiers: 0,
        }));
    }
    println!(
        "[keys] 钩子就绪，准备注入 {} 条（A/B/C 按下抬起）",
        events.len()
    );

    let fixture = signal_fixture().await?;
    let code = PairingCode::generate();
    let host_id = IdentityKeyPair::generate();
    let ctrl_id = IdentityKeyPair::generate();
    let code_proof = hmac_sha256_b64(code.as_str().as_bytes(), b"rdc-s4");
    let ready = Arc::new(tokio::sync::Notify::new());
    let (host_rt, ctrl_rt) = tokio::join!(
        setup_host(
            fixture.addr,
            &fixture.cert,
            &fixture.key,
            host_id,
            code.clone(),
            ready.clone()
        ),
        setup_ctrl(
            fixture.addr,
            &fixture.cert,
            ctrl_id,
            code,
            code_proof,
            ready.clone()
        )
    );
    let mut host_rt = host_rt?;
    let mut ctrl_rt = ctrl_rt?;

    let max_events = events.len() as u32;
    let host_fut = async {
        let out = match create_injector() {
            Ok(mut injector) => {
                rdc_core::s4::input::run_host(
                    &mut host_rt.secure,
                    &mut *injector,
                    max_events,
                    Some(KEY_PACE),
                    |_| {},
                )
                .await
            }
            Err(err) => Err(rdc_core::s4::S4Error::Input(err)),
        };
        match &out {
            Ok(count) => println!(
                "[keys] host 完成注入 {count} 条 +{} ms",
                start.elapsed().as_millis()
            ),
            Err(err) => println!(
                "[keys] host 失败: {err} +{} ms",
                start.elapsed().as_millis()
            ),
        }
        out
    };
    let ctrl_fut = async {
        let out = rdc_core::s4::input::run_ctrl(&mut ctrl_rt.secure, &events).await;
        match &out {
            Ok(counters) => println!(
                "[keys] ctrl 收到确认 {} 条 +{} ms",
                counters.injected,
                start.elapsed().as_millis()
            ),
            Err(err) => println!(
                "[keys] ctrl 失败: {err} +{} ms",
                start.elapsed().as_millis()
            ),
        }
        out
    };
    let (host_result, ctrl_result) = tokio::join!(host_fut, ctrl_fut);
    host_rt.close();
    ctrl_rt.close();
    let observed = hook.stop();

    let injected = host_result.map_err(|e| e.to_string())?;
    let counters = ctrl_result.map_err(|e| e.to_string())?;
    println!(
        "[keys] 发送 {} 条，注入 {} 条，钩子记录 {} 条（钩子仅统计 LLKHF_INJECTED）",
        counters.sent,
        injected,
        observed.len()
    );

    let mut failures = Vec::new();
    if observed.len() != events.len() {
        failures.push(format!(
            "记录条数 {} != 发送 {}",
            observed.len(),
            events.len()
        ));
    }
    for (index, (record, event)) in observed.iter().zip(events.iter()).enumerate() {
        if let InputEvent::Key(key) = event {
            if record.scan_code != key.scan_code || record.down != key.pressed {
                failures.push(format!(
                    "第 {index} 条错位：记录 {} {} / 期望 {} {}",
                    record.scan_code,
                    if record.down { "down" } else { "up" },
                    key.scan_code,
                    if key.pressed { "down" } else { "up" }
                ));
            }
        }
        if !record.injected {
            failures.push(format!("第 {index} 条缺少 LLKHF_INJECTED"));
        }
    }

    if failures.is_empty() {
        println!(
            "[keys] 顺序与注入标记一致：{}/{}；+{} ms",
            observed.len(),
            events.len(),
            start.elapsed().as_millis()
        );
        println!("[keys] PASS +{} ms", start.elapsed().as_millis());
        Ok(())
    } else {
        Err(format!("[keys] FAIL: {}", failures.join("；")).into())
    }
}

// =====** demo-clipboard =====**

/// demo-clipboard 控制器侧文本（多行，验证 LF 归一化后的往返一致性）。
const CLIP_CTRL_TEXT: &str = "来自控制器的第一段\r\n第二行 text-1";
/// demo-clipboard 被控端改写文本。
const CLIP_HOST_TEXT: &str = "被控端改写\r\nsecond line text-2";

/// 剪贴板通道演示：ctrl 写 → host 读 → host 改 → ctrl 读，并验证去重与回环。
///
/// 两端在同一进程内按 lockstep 交替执行，每一步都真正经过加密会话通道；
/// 最后 ctrl 发一条 `Done`，host 必须收到 `Done` 而不是又一条 `Clipboard`，
/// 以此证明“自己写入不会触发回环再写一次”。
///
/// # Returns
/// 往返内容、去重与回环判定全部符合预期时返回 `Ok(())`。
///
/// # Errors
/// 剪贴板读写、通道收发失败或任一项判定不符时返回错误。
pub async fn run_clipboard() -> DemoResult<()> {
    let start = Instant::now();
    let fixture = signal_fixture().await?;
    let code = PairingCode::generate();
    let host_id = IdentityKeyPair::generate();
    let ctrl_id = IdentityKeyPair::generate();
    let code_proof = hmac_sha256_b64(code.as_str().as_bytes(), b"rdc-s4");
    let ready = Arc::new(tokio::sync::Notify::new());
    let (host_rt, ctrl_rt) = tokio::join!(
        setup_host(
            fixture.addr,
            &fixture.cert,
            &fixture.key,
            host_id,
            code.clone(),
            ready.clone()
        ),
        setup_ctrl(
            fixture.addr,
            &fixture.cert,
            ctrl_id,
            code,
            code_proof,
            ready.clone()
        )
    );
    let mut host_rt = host_rt?;
    let mut ctrl_rt = ctrl_rt?;
    println!("[clipboard] 会话就绪 +{} ms", start.elapsed().as_millis());

    let mut host_task = rdc_core::s4::clipboard::ClipboardTask::new();
    let mut ctrl_task = rdc_core::s4::clipboard::ClipboardTask::new();
    let to_err = |e: rdc_core::s4::S4Error| e.to_string();

    // 1. ctrl 写入并外发。
    let sent1 = ctrl_task
        .publish(&mut ctrl_rt.secure, CLIP_CTRL_TEXT)
        .await
        .map_err(to_err)?;
    println!("[clipboard] 1. ctrl 写入并外发：{sent1}");
    // 2. host 收到并写回本机。
    let applied1 = host_task
        .apply_remote(&mut host_rt.secure)
        .await
        .map_err(to_err)?;
    let host_read1 = host_task.read().map_err(to_err)?;
    println!(
        "[clipboard] 2. host 应用远端：{}，直读 {}",
        applied1.as_deref().unwrap_or("<None>"),
        host_read1.as_deref().unwrap_or("<None>")
    );
    // 3. host 把刚写回的内容再交给同步状态机：应被判为回环，不外发。
    let echo = host_task
        .publish(&mut host_rt.secure, CLIP_CTRL_TEXT)
        .await
        .map_err(to_err)?;
    println!("[clipboard] 3. host 重写同一内容：外发={echo}（期望 false，回环）");
    // 4. host 改写并发往 ctrl。
    let sent2 = host_task
        .publish(&mut host_rt.secure, CLIP_HOST_TEXT)
        .await
        .map_err(to_err)?;
    println!("[clipboard] 4. host 改写并外发：{sent2}");
    // 5. ctrl 收到并写回本机。
    let applied2 = ctrl_task
        .apply_remote(&mut ctrl_rt.secure)
        .await
        .map_err(to_err)?;
    let ctrl_read2 = ctrl_task.read().map_err(to_err)?;
    println!(
        "[clipboard] 5. ctrl 应用远端：{}，直读 {}",
        applied2.as_deref().unwrap_or("<None>"),
        ctrl_read2.as_deref().unwrap_or("<None>")
    );
    // 6. ctrl 重发已经发过的内容：应被去重挡下，不产生新消息。
    let dup = ctrl_task
        .publish(&mut ctrl_rt.secure, CLIP_CTRL_TEXT)
        .await
        .map_err(to_err)?;
    println!("[clipboard] 6. ctrl 重发同一内容：外发={dup}（期望 false，去重）");
    // 7. 回环探针：ctrl 只发 Done，host 收到的必须也是 Done。
    rdc_core::s4::send(&mut ctrl_rt.secure, &rdc_core::s4::message::S4Message::Done)
        .await
        .map_err(to_err)?;
    let tail = rdc_core::s4::recv(&mut host_rt.secure)
        .await
        .map_err(to_err)?;
    let tail_is_done = matches!(tail, rdc_core::s4::message::S4Message::Done);
    println!(
        "[clipboard] 7. 回环探针：host 收到 {}（期望 Done）",
        if tail_is_done {
            "Done"
        } else {
            "多余剪贴板消息"
        }
    );

    let ctrl_stats = ctrl_task.stats();
    let host_stats = host_task.stats();
    host_rt.close();
    ctrl_rt.close();
    println!(
        "[clipboard] ctrl stats: sent={} applied={} dup={} echo={}",
        ctrl_stats.sent,
        ctrl_stats.applied,
        ctrl_stats.skipped_duplicates,
        ctrl_stats.skipped_echoes
    );
    println!(
        "[clipboard] host stats: sent={} applied={} dup={} echo={}",
        host_stats.sent,
        host_stats.applied,
        host_stats.skipped_duplicates,
        host_stats.skipped_echoes
    );

    let normalized_ctrl = rdc_clipboard::text::to_network(CLIP_CTRL_TEXT);
    let normalized_host = rdc_clipboard::text::to_network(CLIP_HOST_TEXT);
    let mut failures = Vec::new();
    if !sent1 {
        failures.push("ctrl 首条未外发");
    }
    if applied1.as_deref() != Some(normalized_ctrl.as_ref()) {
        failures.push("host 应用内容与 ctrl 发送不符");
    }
    if host_read1.as_deref() != Some(normalized_ctrl.as_ref()) {
        failures.push("host 直读内容不符");
    }
    if echo {
        failures.push("host 重写同一内容仍外发（回环未被挡住）");
    }
    if !sent2 {
        failures.push("host 改写后未外发");
    }
    if applied2.as_deref() != Some(normalized_host.as_ref()) {
        failures.push("ctrl 应用内容与 host 发送不符");
    }
    if ctrl_read2.as_deref() != Some(normalized_host.as_ref()) {
        failures.push("ctrl 直读内容不符");
    }
    if dup {
        failures.push("ctrl 重发同一内容未被去重");
    }
    if !tail_is_done {
        failures.push("回环探针收到多余剪贴板消息");
    }
    if host_stats.skipped_echoes == 0 {
        failures.push("host 未记录回环计数");
    }
    if ctrl_stats.skipped_duplicates == 0 {
        failures.push("ctrl 未记录去重计数");
    }

    if failures.is_empty() {
        println!("[clipboard] PASS +{} ms", start.elapsed().as_millis());
        Ok(())
    } else {
        Err(format!("[clipboard] FAIL: {}", failures.join("；")).into())
    }
}

// =====** demo-file =====**

/// demo-file 传输的文件大小（5 MiB，跨一个 4 MiB 块边界）。
const FILE_BYTES: u64 = 5 << 20;
/// demo-file 使用的分块大小（0 走 `rdc-filetransfer` 默认 4 MiB，这里显式用于算期望值）。
const FILE_CHUNK_BYTES: u64 = 4 << 20;

/// 临时文件守卫：无论成功失败，离开作用域即删除，避免残留。
struct TempFiles {
    /// 待清理路径。
    paths: Vec<PathBuf>,
}

impl Drop for TempFiles {
    fn drop(&mut self) {
        for path in &self.paths {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// 文件通道演示：5 MiB 随机文件经 1352 字节分片传完，比对两端 SHA-256。
///
/// 首次发送第一块时故意改坏一个字节，用来验证块校验、`FileAck{ok=false}` 与重传路径。
///
/// # Returns
/// 哈希一致、重传次数符合预期、分片数符合计划时返回 `Ok(())`。
///
/// # Errors
/// 造样本、会话、传输失败或任一项核对不符时返回错误。
pub async fn run_file() -> DemoResult<()> {
    let start = Instant::now();
    let pid = std::process::id();
    let src = std::env::temp_dir().join(format!("rdc-s4file-src-{pid}.bin"));
    let dst = std::env::temp_dir().join(format!("rdc-s4file-dst-{pid}.bin"));
    let _temp = TempFiles {
        paths: vec![src.clone(), dst.clone()],
    };
    write_random_file(&src, FILE_BYTES)?;
    let src_hash = hash_file(&src)?;
    println!(
        "[file] 源文件 {} 字节（{} MiB），SHA-256 {}",
        FILE_BYTES,
        FILE_BYTES / (1 << 20),
        hex(&src_hash)
    );

    let fixture = signal_fixture().await?;
    let code = PairingCode::generate();
    let host_id = IdentityKeyPair::generate();
    let ctrl_id = IdentityKeyPair::generate();
    let code_proof = hmac_sha256_b64(code.as_str().as_bytes(), b"rdc-s4");
    let ready = Arc::new(tokio::sync::Notify::new());
    let (host_rt, ctrl_rt) = tokio::join!(
        setup_host(
            fixture.addr,
            &fixture.cert,
            &fixture.key,
            host_id,
            code.clone(),
            ready.clone()
        ),
        setup_ctrl(
            fixture.addr,
            &fixture.cert,
            ctrl_id,
            code,
            code_proof,
            ready.clone()
        )
    );
    let mut host_rt = host_rt?;
    let mut ctrl_rt = ctrl_rt?;
    println!("[file] 会话就绪 +{} ms", start.elapsed().as_millis());

    let dst_path = dst.clone();
    let host_fut = async { rdc_core::s4::file::recv_file(&mut host_rt.secure, &dst_path).await };
    let src_path = src.clone();
    // 第一块首次发送故意改坏一个字节，驱动块校验 + 重传路径。
    let ctrl_fut =
        async { rdc_core::s4::file::send_file(&mut ctrl_rt.secure, &src_path, 0, true).await };
    let (host_result, ctrl_result) = tokio::join!(host_fut, ctrl_fut);
    host_rt.close();
    ctrl_rt.close();

    let sent = ctrl_result.map_err(|e| e.to_string())?;
    let received = host_result.map_err(|e| e.to_string())?;
    let dst_hash = hash_file(&dst)?;

    let expected_frags = expected_fragments();
    let first_chunk_frags = FILE_CHUNK_BYTES.div_ceil(rdc_core::s4::FILE_FRAGMENT_BYTES as u64);
    println!(
        "[file] 块 {} 个（每块 ≤ {} MiB），分片实际 {} / 期望 {}（重传片 {}），重传 {} 次",
        sent.chunks,
        FILE_CHUNK_BYTES / (1 << 20),
        sent.fragments,
        expected_frags + first_chunk_frags,
        first_chunk_frags,
        sent.retransmissions
    );
    println!(
        "[file] 耗时：发送 {} ms / 接收 {} ms，吞吐 {:.3} MB/s",
        sent.elapsed.as_millis(),
        received.elapsed.as_millis(),
        sent.throughput_mbps
    );
    println!("[file] SHA-256 源 {}", hex(&src_hash));
    println!("[file] SHA-256 收 {}", hex(&dst_hash));
    println!(
        "[file] 接收端自校验 {}（{} 字节）",
        if received.ok { "一致" } else { "不一致" },
        received.total_bytes
    );

    let mut failures = Vec::new();
    if src_hash != dst_hash {
        failures.push("接收端 SHA-256 与源文件不一致");
    }
    if received.actual_hash != src_hash {
        failures.push("接收端回报的哈希与源文件不一致");
    }
    if !received.ok {
        failures.push("接收端自校验未通过");
    }
    if received.total_bytes != FILE_BYTES {
        failures.push("接收字节数不符");
    }
    if sent.fragments != expected_frags + first_chunk_frags {
        failures.push("分片数不符（期望含一次整块重传）");
    }
    if sent.retransmissions != 1 {
        failures.push("重传次数不符（期望 1 次）");
    }
    if sent.chunks != 2 || received.chunks != 2 {
        failures.push("分块数不符（期望 2）");
    }

    if failures.is_empty() {
        println!("[file] PASS +{} ms", start.elapsed().as_millis());
        Ok(())
    } else {
        Err(format!("[file] FAIL: {}", failures.join("；")).into())
    }
}

/// 按计划推算出分片总数（不含重传）。
fn expected_fragments() -> u64 {
    let chunk_size = FILE_CHUNK_BYTES;
    let frag_size = rdc_core::s4::FILE_FRAGMENT_BYTES as u64;
    let mut remaining = FILE_BYTES;
    let mut total = 0u64;
    while remaining > 0 {
        let len = remaining.min(chunk_size);
        total += len.div_ceil(frag_size);
        remaining -= len;
    }
    total
}

/// 写入 `bytes` 字节的伪随机内容（splitmix64，够快且不可压缩），用于构造演示样本。
///
/// # Errors
/// 写盘失败时返回错误。
fn write_random_file(path: &Path, bytes: u64) -> DemoResult<()> {
    let mut buf = Vec::with_capacity(bytes as usize);
    let mut state = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x5EED)
        .wrapping_add(0x9E37_79B9_7F4A_7C15);
    while (buf.len() as u64) < bytes {
        state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        buf.extend_from_slice(&z.to_le_bytes());
    }
    buf.truncate(bytes as usize);
    std::fs::write(path, &buf)?;
    Ok(())
}

/// 分块读取文件算 SHA-256。
///
/// # Errors
/// 文件打不开或读取失败时返回错误。
fn hash_file(path: &Path) -> DemoResult<[u8; 32]> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher)?;
    let digest = hasher.finalize();
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    Ok(out)
}

/// 32 字节哈希转小写十六进制。
fn hex(hash: &[u8; 32]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(64);
    for byte in hash {
        let _ = write!(text, "{byte:02x}");
    }
    text
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
