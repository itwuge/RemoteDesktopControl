//!
//! 层级：L4 验证入口
//! 用途：`demo-wall` —— 本机 1 个 host + 3 个观看者，各拉一路 480p 子码流，运行 10 s；
//!       打印每路帧率/时间戳单调性/端到端预览延迟 p99/host 额外 CPU/清理结果。
//! 调用方：`rdc-cli` 子命令；超时由 main.rs 统一兜底。
//!
//! 形态：单进程内多角色（1 个 host 任务 + 3 个 viewer 任务），3 条 QUIC 直连 +
//!       AEAD 控制流；不创建子进程、不打开窗口，结束后关闭全部会话。
//!
//! 判据：见 `logs/TESTPLAN-s5-1.md`（先落盘再运行）。

use std::error::Error;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rdc_codec::{CodecKind, EncodedPacket, VideoChroma, VideoConfig};
use rdc_connect::{dial, establish, listen, DirectListener, EstablishedSession, QuicSignalClient};
use rdc_core::s5::devices::{now_ms, DeviceList};
use rdc_core::s5::multi::{
    authorize_input, wall_capabilities, SessionRegistry, SessionRole, SessionSlot,
};
use rdc_core::s5::wall::{
    send_done, send_frame, send_start, ThumbnailEncoder, WallFrame, WallPreviewHub, WallReceiver,
    WALL_BITRATE_BPS, WALL_FPS, WALL_HEIGHT, WALL_WIDTH,
};
use rdc_protocol::{
    CallMode, CandidateDesc, CandidateProto, Capabilities, SignalMessage, SignalPath,
};
use rdc_runtime::FramePool;
use rdc_security::{hmac_sha256_b64, IdentityKeyPair, PairingCode, SessionHandshake};
use rdc_signal::SignalServer;

// =====** 常量 =====**

/// 演示统一错误类型。
type DemoResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

/// 观看者数量。
const VIEWERS: usize = 3;
/// 屏幕墙运行时长（秒）。
const WALL_SECS: u64 = 10;
/// 主码流基准阶段时长（秒）：只跑采集+主编码，用来测「单路主码流 CPU」。
const MAIN_ONLY_SECS: u64 = 3;
/// 屏幕墙单独测量阶段时长（秒）：只跑缩放+缩略编码+分发，不跑主编码。
///
/// 为什么单独测：主编码在本机把多核吃满（编码器内部多线程），并发差分会被饱和
/// 掩盖成 0；单独测同一套屏幕墙管线的边际成本，再与单路主码流比较，才是可解释的数字。
const WALL_ONLY_SECS: u64 = 3;
/// 合成源尺寸（主码流）。
const SRC_W: u32 = 1920;
const SRC_H: u32 = 1080;
/// 主码流帧率。
const MAIN_FPS: u32 = 30;
/// 主码流码率（只作配置）。
const MAIN_BITRATE_BPS: u32 = 8_000_000;
/// 建链总超时。
const SETUP_TIMEOUT: Duration = Duration::from_secs(40);
/// 单帧向单会话发送的超时（判定「未响应」）。
const SEND_TIMEOUT: Duration = Duration::from_millis(300);
/// 观看者收流总超时（覆盖 主码流基准 + 屏幕墙单测 + 10 s 判据窗口，再加余量）。
const VIEWER_TIMEOUT: Duration =
    Duration::from_secs(WALL_SECS + MAIN_ONLY_SECS + WALL_ONLY_SECS + 5);

/// 判据门槛。
const FPS_MIN: f64 = 5.0;
const P99_MAX_US: u64 = 500_000;
/// 4 路预览额外 CPU 上限：单路主码流的 30%。
const CPU_BUDGET_RATIO: f64 = 0.30;

/// 演示单调时钟原点（host 时间戳与 viewer 接收时刻共用）。
static ORIGIN: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// 以微秒返回自演示原点的时间。
fn origin_us() -> u64 {
    ORIGIN.get_or_init(Instant::now).elapsed().as_micros() as u64
}

// =====** 信令夹具 =====**

/// 信令夹具：同进程临时自签服务。
struct SignalFixture {
    /// 服务地址。
    addr: SocketAddr,
    /// 证书 DER。
    cert: Vec<u8>,
    /// 私钥 DER。
    key: Vec<u8>,
}

/// 起临时信令服务。
///
/// # Errors
/// 绑定失败时返回错误。
async fn signal_fixture() -> DemoResult<SignalFixture> {
    let server = SignalServer::bind("127.0.0.1:0".parse::<SocketAddr>()?).await?;
    let addr = server.addr();
    let cert = server.cert_der().to_vec();
    let key = server.key_der().to_vec();
    tokio::spawn(async move { server.run().await });
    Ok(SignalFixture { addr, cert, key })
}

// =====** 运行时 =====**

/// 被控端（1 个）与它的 3 路会话。
struct WallHost {
    /// 直连监听器（保活）。
    _listener: DirectListener,
    /// 信令客户端（保活）。
    client: QuicSignalClient,
    /// 3 路已建立的加密会话。
    sessions: Vec<EstablishedSession>,
    /// 每路会话的信令 session id（日志用）。
    session_ids: Vec<String>,
}

impl WallHost {
    /// 关闭信令（监听器随结构体释放）。
    fn close(self) {
        self.client.close();
    }
}

/// 单个观看者运行时。
struct WallViewer {
    /// 信令客户端（保活）。
    client: QuicSignalClient,
    /// 已建立的加密会话。
    secure: EstablishedSession,
}

impl WallViewer {
    /// 关闭会话与信令。
    fn close(mut self) {
        self.secure.close();
        self.client.close();
    }
}

/// 被控端建链（顺序接受 3 路观看者）。
///
/// 为什么顺序接受：3 个观看者共用同一个 host 身份与配对码，直连接受的顺序与信令
/// Incoming 的顺序无法可靠对应；顺序建链让「连接 ↔ 会话 ↔ 对端公钥」一一对应。
///
/// # Errors
/// 监听、信令、直连、握手任一步失败或超时返回错误。
async fn setup_wall_host(
    fixture: &SignalFixture,
    host_id: IdentityKeyPair,
    host_seed: [u8; 32],
    code: PairingCode,
    host_ready: Arc<AtomicBool>,
    viewers: usize,
) -> DemoResult<WallHost> {
    let listener = listen(
        "127.0.0.1:0".parse::<SocketAddr>()?,
        &fixture.cert,
        &fixture.key,
    )
    .await?;
    let host_addr = listener.local_addr()?;
    let mut client = QuicSignalClient::connect_and_register(
        fixture.addr,
        &fixture.cert,
        &host_id,
        "host-01",
        "s5-demo-host",
        "windows",
        wall_capabilities(Capabilities::DEFAULT),
    )
    .await?;
    host_ready.store(true, Ordering::Release);
    let mut sessions = Vec::with_capacity(viewers);
    let mut session_ids = Vec::with_capacity(viewers);
    for index in 0..viewers {
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
        let direct = tokio::time::timeout(Duration::from_secs(10), listener.accept())
            .await
            .map_err(|_| format!("viewer {} 直连超时", index + 1))??;
        let handshake = SessionHandshake::new_host(
            session.clone(),
            IdentityKeyPair::from_seed(&host_seed),
            code.clone(),
            peer_pub,
        );
        let secure = establish(direct, handshake, false).await?;
        client.connected(&session, SignalPath::Udp).await?;
        println!("[wall] host 已接受观看者 {}", index + 1);
        sessions.push(secure);
        session_ids.push(session);
    }
    Ok(WallHost {
        _listener: listener,
        client,
        sessions,
        session_ids,
    })
}

/// 观看者建链：注册 → 等 host 就绪 → 呼叫 → 候选 → Punch → 拨号 → 握手。
///
/// # Errors
/// 信令、拨号、握手任一步失败或超时返回错误。
async fn setup_wall_viewer(
    fixture: &SignalFixture,
    viewer_id: IdentityKeyPair,
    tag: String,
    code: PairingCode,
    code_proof: String,
    host_ready: Arc<AtomicBool>,
) -> DemoResult<WallViewer> {
    let mut client = QuicSignalClient::connect_and_register(
        fixture.addr,
        &fixture.cert,
        &viewer_id,
        &tag,
        "s5-demo-viewer",
        "windows",
        wall_capabilities(Capabilities::DEFAULT),
    )
    .await?;
    // 等 host 注册在服务端生效，避免 PeerNotFound 竞态。
    let wait_start = Instant::now();
    while !host_ready.load(Ordering::Acquire) {
        if wait_start.elapsed() > Duration::from_secs(10) {
            return Err("等待 host 注册超时".into());
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let reply = client.call("host-01", CallMode::Code, 0, &code_proof).await?;
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
    let direct = dial(peer_addr, &fixture.cert).await?;
    let handshake = SessionHandshake::new_controller(session.clone(), viewer_id, code);
    let secure = establish(direct, handshake, true).await?;
    client.connected(&session, SignalPath::Udp).await?;
    println!("[wall] 观看者 {tag} 已连接");
    Ok(WallViewer { client, secure })
}

// =====** 合成主码流 =====**

/// 生成两条预制行（64 像素棋盘格）。
fn pattern_rows(width: u32) -> [Vec<u8>; 2] {
    let dark = [32u8, 32, 40, 255];
    let light = [200u8, 200, 208, 255];
    let mut rows = [vec![0u8; width as usize * 4], vec![0u8; width as usize * 4]];
    for (index, color) in [dark, light].into_iter().enumerate() {
        for px in rows[index].chunks_exact_mut(4) {
            px.copy_from_slice(&color);
        }
    }
    rows
}

/// 往缓冲区画一帧动态图案（整行拷贝 + 一条移动白带，成本与像素数线性）。
fn paint_frame(buf: &mut [u8], width: u32, height: u32, tick: u32, rows: &[Vec<u8>; 2]) {
    let stride = width as usize * 4;
    for y in 0..height as usize {
        let band = (((y as u32 / 64) + tick) % 2) as usize;
        buf[y * stride..(y + 1) * stride].copy_from_slice(&rows[band]);
    }
    let bar = 64usize;
    let span = (width as usize - bar).max(1);
    let x0 = (tick as usize * 16) % span;
    for y in 0..height as usize {
        let offset = y * stride + x0 * 4;
        buf[offset..offset + bar * 4].fill(0xFF);
    }
}

// =====** host 发送 =====**

/// host 侧统计。
#[derive(Debug, Clone)]
struct HostStats {
    /// 主码流编码包数。
    main_packets: u64,
    /// 子码流编码包数。
    wall_packets: u64,
    /// 子码流字节数。
    wall_bytes: u64,
    /// 已发送分片数。
    fragments: u64,
    /// 发送失败/超时次数（判据 W6 要求 0）。
    send_failures: u64,
    /// 主码流-only 阶段 CPU（微秒/秒），未采到为 -1。
    main_cpu_us_per_s: f64,
    /// 主码流+屏幕墙阶段 CPU（微秒/秒）。
    total_cpu_us_per_s: f64,
    /// 屏幕墙额外 CPU（微秒/秒）。
    wall_extra_cpu_us_per_s: f64,
    /// 子码流编码器后端名。
    backend: String,
}

impl Default for HostStats {
    fn default() -> Self {
        Self {
            main_packets: 0,
            wall_packets: 0,
            wall_bytes: 0,
            fragments: 0,
            send_failures: 0,
            main_cpu_us_per_s: -1.0,
            total_cpu_us_per_s: -1.0,
            wall_extra_cpu_us_per_s: -1.0,
            backend: String::new(),
        }
    }
}

/// 向所有会话分发一帧（复用同一份编码字节，不重复编码）。
///
/// # Returns
/// 成功发送的会话数。
///
/// # Errors
/// 不会失败；失败按会话计数到 `send_failures`。
async fn fanout(
    sessions: &mut [EstablishedSession],
    stream_ids: &[u32],
    frame: &WallFrame,
    stats: &mut HostStats,
) {
    for (index, session) in sessions.iter_mut().enumerate() {
        let assigned = WallFrame {
            stream_id: stream_ids[index],
            ..frame.clone()
        };
        match tokio::time::timeout(SEND_TIMEOUT, send_frame(session, &assigned)).await {
            Ok(Ok(fragments)) => stats.fragments += fragments as u64,
            Ok(Err(err)) => {
                stats.send_failures += 1;
                eprintln!("[wall] 发送到流 {} 失败：{err}", stream_ids[index]);
            }
            Err(_) => {
                stats.send_failures += 1;
                eprintln!("[wall] 发送到流 {} 超时", stream_ids[index]);
            }
        }
    }
}

/// host 主循环：先主码流-only 基准，再主码流+屏幕墙 10 s，最后给每路发结束。
///
/// # Errors
/// 编码器创建/配置失败时返回错误。
async fn host_stream(
    mut sessions: Vec<EstablishedSession>,
    stream_ids: Vec<u32>,
) -> DemoResult<HostStats> {
    let rows = pattern_rows(SRC_W);
    let stride = SRC_W as usize * 4;
    let mut main_encoder = rdc_codec::create_encoder(CodecKind::H264)?;
    main_encoder.configure(VideoConfig {
        codec: CodecKind::H264,
        width: SRC_W,
        height: SRC_H,
        fps: MAIN_FPS,
        bitrate_bps: MAIN_BITRATE_BPS,
        chroma: VideoChroma::Yuv420,
        bit_depth: 8,
        low_latency: true,
    })?;
    let main_pool = FramePool::new(4, stride * SRC_H as usize);
    // 缩略图输入：与主码流同一 tick 重画一份（模拟“采集帧的副本”，避免持有主缓冲所有权）。
    let mut thumb_src = vec![0u8; stride * SRC_H as usize];
    let mut stats = HostStats::default();
    let mut tick: u32 = 0;

    // ---- 阶段 A：只跑主码流，得到「单路主码流 CPU」基线 ----
    let phase_a_start = Instant::now();
    let cpu_a0 = process_cpu_us();
    let mut next_at = phase_a_start;
    while phase_a_start.elapsed() < Duration::from_secs(MAIN_ONLY_SECS) {
        if let Some(mut buffer) = main_pool.acquire() {
            paint_frame(buffer.as_mut_slice(), SRC_W, SRC_H, tick, &rows);
            main_encoder.push(buffer, origin_us() * 1000, false)?;
            while main_encoder.poll_packet().is_some() {
                stats.main_packets += 1;
            }
        }
        tick = tick.wrapping_add(1);
        next_at += Duration::from_millis(1000 / MAIN_FPS as u64);
        sleep_until(next_at).await;
    }
    let cpu_a1 = process_cpu_us();
    let phase_a_secs = phase_a_start.elapsed().as_secs_f64();

    // ---- 阶段 B：主码流 + 共享缩略图编码器 + 3 路分发，10 s ----
    let mut thumb = ThumbnailEncoder::new(WALL_WIDTH, WALL_HEIGHT, WALL_FPS, WALL_BITRATE_BPS)?;
    stats.backend = thumb.backend_name().to_owned();
    let (thumb_w, thumb_h) = thumb.size();
    println!(
        "[wall] 共享缩略图编码器：backend={} {}x{} @ {} fps / {} kbps（实例数 1，观看者 {}）",
        stats.backend,
        thumb_w,
        thumb_h,
        WALL_FPS,
        WALL_BITRATE_BPS / 1000,
        sessions.len()
    );
    // ---- 阶段 W：只跑屏幕墙管线（缩放 + 缩略编码 + 3 路分发），用于量出额外 CPU ----
    let wall_start = Instant::now();
    let cpu_w0 = process_cpu_us();
    let mut w_next = wall_start;
    let mut w_tick = tick;
    let mut w_produced: u64 = 0;
    while wall_start.elapsed() < Duration::from_secs(WALL_ONLY_SECS) {
        paint_frame(&mut thumb_src, SRC_W, SRC_H, w_tick, &rows);
        let produced = thumb.encode(&thumb_src, SRC_W, SRC_H, origin_us() * 1000)?;
        for packet in produced {
            let frame = WallFrame {
                stream_id: 0,
                frame_id: w_produced as u32,
                pts_us: packet.pts_ns / 1000,
                width: thumb_w as u16,
                height: thumb_h as u16,
                data: packet.data,
            };
            w_produced += 1;
            fanout(&mut sessions, &stream_ids, &frame, &mut stats).await;
        }
        w_tick = w_tick.wrapping_add(1);
        w_next += Duration::from_millis(1000 / WALL_FPS as u64);
        sleep_until(w_next).await;
    }
    let cpu_w1 = process_cpu_us();
    let wall_phase_secs = wall_start.elapsed().as_secs_f64();
    println!(
        "[wall] 屏幕墙单测阶段：{w_produced} 帧（仅测量 CPU，不计入判据）"
    );

    // 通知观看者「从下一帧开始计时」，预热帧在观看者侧重组后丢弃。
    for (index, session) in sessions.iter_mut().enumerate() {
        let _ = tokio::time::timeout(SEND_TIMEOUT, send_start(session, stream_ids[index])).await;
    }

    // ---- 阶段 B：主码流 + 共享缩略图编码器 + 3 路分发，10 s ----
    let phase_b_start = Instant::now();
    let cpu_b0 = process_cpu_us();
    let mut frame_id: u32 = 0;
    let mut last_pts_us: u64 = 0;

    while phase_b_start.elapsed() < Duration::from_secs(WALL_SECS) {
        let mut produced: Vec<EncodedPacket> = Vec::new();
        if let Some(mut buffer) = main_pool.acquire() {
            paint_frame(buffer.as_mut_slice(), SRC_W, SRC_H, tick, &rows);
            main_encoder.push(buffer, origin_us() * 1000, false)?;
            while main_encoder.poll_packet().is_some() {
                stats.main_packets += 1;
            }
        }
        // 每 3 个主码流 tick 生成 1 帧缩略图（30 fps / 3 = 10 fps）。
        if tick % 3 == 0 {
            paint_frame(&mut thumb_src, SRC_W, SRC_H, tick, &rows);
            produced.extend(thumb.encode(&thumb_src, SRC_W, SRC_H, origin_us() * 1000)?);
        }
        tick = tick.wrapping_add(1);

        for packet in produced {
            let pts_us = packet.pts_ns / 1000;
            // 保证对外时间戳严格递增：同一时间戳的多包只保留首个。
            if pts_us <= last_pts_us {
                continue;
            }
            last_pts_us = pts_us;
            let frame = WallFrame {
                stream_id: 0,
                frame_id,
                pts_us,
                width: thumb_w as u16,
                height: thumb_h as u16,
                data: packet.data,
            };
            frame_id = frame_id.wrapping_add(1);
            stats.wall_packets += 1;
            stats.wall_bytes += frame.data.len() as u64;
            fanout(&mut sessions, &stream_ids, &frame, &mut stats).await;
        }

        next_at += Duration::from_millis(1000 / MAIN_FPS as u64);
        sleep_until(next_at).await;
    }

    // 冲刷缩略图流水线，把滞留帧也发出去。
    for packet in thumb.flush()? {
        let pts_us = packet.pts_ns / 1000;
        if pts_us <= last_pts_us {
            continue;
        }
        last_pts_us = pts_us;
        let frame = WallFrame {
            stream_id: 0,
            frame_id,
            pts_us,
            width: thumb_w as u16,
            height: thumb_h as u16,
            data: packet.data,
        };
        frame_id = frame_id.wrapping_add(1);
        stats.wall_packets += 1;
        stats.wall_bytes += frame.data.len() as u64;
        fanout(&mut sessions, &stream_ids, &frame, &mut stats).await;
    }
    let cpu_b1 = process_cpu_us();
    let phase_b_secs = phase_b_start.elapsed().as_secs_f64();

    // ---- 结束：每路一条 Done，观看者据此退出，不靠取消读操作 ----
    for (index, session) in sessions.iter_mut().enumerate() {
        let _ = tokio::time::timeout(SEND_TIMEOUT, send_done(session, stream_ids[index])).await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;

    if let (Some(a0), Some(a1), Some(w0), Some(w1), Some(b0), Some(b1)) =
        (cpu_a0, cpu_a1, cpu_w0, cpu_w1, cpu_b0, cpu_b1)
    {
        let main_rate = (a1.saturating_sub(a0)) as f64 / phase_a_secs.max(0.001);
        let wall_rate = (w1.saturating_sub(w0)) as f64 / wall_phase_secs.max(0.001);
        let total_rate = (b1.saturating_sub(b0)) as f64 / phase_b_secs.max(0.001);
        stats.main_cpu_us_per_s = main_rate;
        stats.total_cpu_us_per_s = total_rate;
        stats.wall_extra_cpu_us_per_s = wall_rate;
        println!(
            "[wall] CPU：单路主码流 {:.1}%，屏幕墙(3 路) {:.1}%，并发合计 {:.1}%（每核）",
            main_rate / 1_000_000.0 * 100.0,
            wall_rate / 1_000_000.0 * 100.0,
            total_rate / 1_000_000.0 * 100.0
        );
    }

    for session in sessions.iter_mut() {
        session.close();
    }
    Ok(stats)
}

/// 等 paced 循环的下一个时间点。
///
/// 已落后时也必须让出一次（哪怕 1 ms）：本演示的 host 与 3 个 viewer 在同一个
/// `join!` 里轮询，host 循环若因为编码耗时超过节拍而完全不 await，会把 viewer
/// 饿到整段结束，表现为「所有帧在最后一次性到达、预览延迟等于墙时长」。
async fn sleep_until(at: Instant) {
    let now = Instant::now();
    if at > now {
        tokio::time::sleep(at - now).await;
    } else {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    tokio::task::yield_now().await;
}

// =====** 观看者接收 =====**

/// 逐帧样本。
#[derive(Debug, Clone, Copy)]
struct FrameSample {
    /// 帧号。
    frame_id: u32,
    /// host 时间戳（微秒）。
    pts_us: u64,
    /// 接收时刻（微秒）。
    recv_us: u64,
    /// 端到端预览延迟（微秒）。
    latency_us: u64,
    /// 编码字节数。
    bytes: usize,
}

/// 单个观看者的统计。
#[derive(Debug, Clone, Default)]
struct ViewerStats {
    /// 观看者标签。
    tag: String,
    /// 样本条数。
    frames: usize,
    /// 时间戳是否严格递增。
    monotonic: bool,
    /// p50 延迟（微秒）。
    p50_us: u64,
    /// p99 延迟（微秒）。
    p99_us: u64,
    /// 最大延迟（微秒）。
    max_us: u64,
    /// 重组丢弃帧数。
    dropped: u64,
}

impl ViewerStats {
    /// 预览帧率（按 10 s 窗口）。
    fn fps(&self) -> f64 {
        self.frames as f64 / WALL_SECS as f64
    }
}

/// 观看者收流循环：重组 → 记录样本 → 更新最新帧汇总。
async fn viewer_loop(
    mut viewer: WallViewer,
    stream_id: u32,
    tag: String,
    hub: Arc<Mutex<WallPreviewHub>>,
) -> DemoResult<(ViewerStats, Vec<FrameSample>)> {
    let mut receiver = WallReceiver::new(stream_id);
    let mut samples = Vec::new();
    let mut monotonic = true;
    let mut last_pts = 0u64;
    let mut first = true;

    while let Some(frame) = receiver.recv_next(&mut viewer.secure).await? {
        // 预热帧（Start 之前）只用于 CPU 测量，不计入判据。
        if !receiver.is_started() {
            continue;
        }
        let recv_us = origin_us();
        let latency_us = recv_us.saturating_sub(frame.pts_us);
        if !first && frame.pts_us <= last_pts {
            monotonic = false;
        }
        first = false;
        last_pts = frame.pts_us;
        samples.push(FrameSample {
            frame_id: frame.frame_id,
            pts_us: frame.pts_us,
            recv_us,
            latency_us,
            bytes: frame.data.len(),
        });
        if let Ok(mut guard) = hub.lock() {
            guard.update(&frame);
        }
    }

    let dropped = receiver.dropped();
    let mut latencies: Vec<u64> = samples.iter().map(|s| s.latency_us).collect();
    latencies.sort_unstable();
    let p50_us = percentile(&latencies, 0.50);
    let p99_us = percentile(&latencies, 0.99);
    let max_us = *latencies.last().unwrap_or(&0);
    viewer.close();
    Ok((
        ViewerStats {
            tag,
            frames: samples.len(),
            monotonic,
            p50_us,
            p99_us,
            max_us,
            dropped,
        },
        samples,
    ))
}

/// 最近秩分位数。
fn percentile(sorted: &[u64], q: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let index = ((sorted.len() - 1) as f64 * q).ceil() as usize;
    sorted[index.min(sorted.len() - 1)]
}

// =====** CPU 采样 =====**

/// 进程累计 CPU（内核 + 用户），单位微秒。
#[cfg(target_os = "windows")]
fn process_cpu_us() -> Option<u64> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
    let zero = FILETIME {
        dwLowDateTime: 0,
        dwHighDateTime: 0,
    };
    let mut creation = zero;
    let mut exit = zero;
    let mut kernel = zero;
    let mut user = zero;
    unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        )
        .ok()?;
    }
    Some(filetime_us(&kernel) + filetime_us(&user))
}

/// FILETIME（100 ns）转微秒。
#[cfg(target_os = "windows")]
fn filetime_us(ft: &windows::Win32::Foundation::FILETIME) -> u64 {
    (((ft.dwHighDateTime as u64) << 32) | ft.dwLowDateTime as u64) / 10
}

/// 非 Windows 不采集（本演示仅在 Windows运行）。
#[cfg(not(target_os = "windows"))]
fn process_cpu_us() -> Option<u64> {
    None
}

// =====** 主流程 =====**

/// `demo-wall` 主流程。
///
/// # Returns
/// 全部判据通过返回 `Ok(())`。
///
/// # Errors
/// 建链、编码、收流、发送或判据不达标时返回错误。
pub async fn run() -> DemoResult<()> {
    let started = Instant::now();
    ORIGIN.get_or_init(Instant::now);
    println!(
        "[wall] 1 host + {VIEWERS} viewers，子码流 {WALL_WIDTH}x{WALL_HEIGHT}@{WALL_FPS}fps，运行 {WALL_SECS}s"
    );

    let fixture = signal_fixture().await?;
    let code = PairingCode::generate();
    let code_proof = hmac_sha256_b64(code.as_str().as_bytes(), b"rdc-s5");
    let host_seed = [0x5Au8; 32];
    let host_id = IdentityKeyPair::from_seed(&host_seed);
    let host_ready = Arc::new(AtomicBool::new(false));

    // 被控端与 3 个观看者并发建链；观看者顺序建立以避免直连顺序错配。
    let (mut host, viewers) = tokio::time::timeout(SETUP_TIMEOUT, async {
        let host_fut = setup_wall_host(
            &fixture,
            host_id,
            host_seed,
            code.clone(),
            host_ready.clone(),
            VIEWERS,
        );
        let viewers_fut = async {
            let mut out = Vec::with_capacity(VIEWERS);
            for index in 0..VIEWERS {
                let viewer_id = IdentityKeyPair::generate();
                let tag = format!("viewer-{:02}", index + 1);
                out.push(
                    setup_wall_viewer(
                        &fixture,
                        viewer_id,
                        tag,
                        code.clone(),
                        code_proof.clone(),
                        host_ready.clone(),
                    )
                    .await?,
                );
            }
            Ok::<_, Box<dyn Error + Send + Sync>>(out)
        };
        let (host, viewers) = tokio::join!(host_fut, viewers_fut);
        Ok::<_, Box<dyn Error + Send + Sync>>((host?, viewers?))
    })
    .await
    .map_err(|_| format!("建链超过 {} s", SETUP_TIMEOUT.as_secs()))??;
    println!("[wall] 建链完成，用时 {:?}", started.elapsed());

    // ---- 会话注册与角色隔离（观看者不得注入输入）----
    let mut registry = SessionRegistry::new(VIEWERS);
    for (index, session_id) in host.session_ids.iter().enumerate() {
        let decision = authorize_input(SessionRole::Viewer);
        assert!(!decision.allowed);
        registry.register(SessionSlot {
            session_id: (index + 1) as u64,
            role: SessionRole::Viewer,
            device_id: session_id.clone(),
            started_ms: now_ms(),
        })?;
    }
    println!(
        "[wall] 多会话注册：{} 路观看者，观看者注入输入 -> {:?}",
        registry.len(),
        authorize_input(SessionRole::Viewer)
    );

    // ---- 设备列表：上线 3 台观看者设备（原子写）----
    let log_dir = ensure_log_dir()?;
    let mut devices = DeviceList::load(log_dir.join("devices.json"))?;
    for (index, session_id) in host.session_ids.iter().enumerate() {
        devices.upsert_online(session_id, &format!("wall-viewer-{}", index + 1), now_ms());
    }
    devices.save(log_dir.join("devices.json"))?;
    println!("[wall] 设备列表已原子写入：{} 台", devices.len());

    // ---- 并发收流 ----
    let hub = Arc::new(Mutex::new(WallPreviewHub::new(&[1, 2, 3])));
    let stream_ids: Vec<u32> = (1..=VIEWERS as u32).collect();
    let host_sessions = std::mem::take(&mut host.sessions);
    let host_task = host_stream(host_sessions, stream_ids.clone());

    let mut viewer_futs: Vec<_> = viewers
        .into_iter()
        .enumerate()
        .map(|(index, viewer)| {
            let stream_id = stream_ids[index];
            let tag = format!("viewer-{:02}", index + 1);
            let hub = hub.clone();
            async move {
                tokio::time::timeout(VIEWER_TIMEOUT, viewer_loop(viewer, stream_id, tag, hub))
                    .await
                    .map_err(|_| -> Box<dyn Error + Send + Sync> {
                        format!("viewer {} 收流超时（未收到 Done）", index + 1).into()
                    })?
            }
        })
        .collect();
    let v0 = viewer_futs.remove(0);
    let v1 = viewer_futs.remove(0);
    let v2 = viewer_futs.remove(0);

    let (host_stats, r0, r1, r2) = tokio::time::timeout(
        Duration::from_secs(WALL_SECS + 20),
        async { tokio::join!(host_task, v0, v1, v2) },
    )
    .await
    .map_err(|_| "收流阶段超过时限（存在未响应）".to_string())?;
    let host_stats = host_stats?;
    let viewer_results = vec![r0?, r1?, r2?];

    let mut stats: Vec<ViewerStats> = Vec::with_capacity(VIEWERS);
    let mut samples: Vec<(String, Vec<FrameSample>)> = Vec::with_capacity(VIEWERS);
    for (index, (viewer_stats, viewer_samples)) in viewer_results.into_iter().enumerate() {
        stats.push(viewer_stats);
        samples.push((format!("viewer-{:02}", index + 1), viewer_samples));
    }

    // ---- 写日志 ----
    for (tag, samples) in &samples {
        write_viewer_csv(&log_dir.join(format!("{tag}.csv")), samples)?;
    }
    let summary = build_summary(&host_stats, &stats, &hub)?;
    write_text(&log_dir.join("host.json"), &host_json(&host_stats))?;
    write_text(&log_dir.join("summary.txt"), &summary)?;
    println!("{summary}");

    // ---- 关闭与残留检查 ----
    host.close();
    let leftover = hub.lock().map(|guard| guard.latest()).unwrap_or_default();
    println!(
        "[wall] 最新帧接口（退出前）：{} 路有帧",
        leftover.iter().filter(|f| f.is_some()).count()
    );

    // ---- 判据 ----
    let mut failures = Vec::new();
    for stat in &stats {
        if stat.fps() < FPS_MIN {
            failures.push(format!("{} 帧率 {:.2} < {FPS_MIN}", stat.tag, stat.fps()));
        }
        if !stat.monotonic {
            failures.push(format!("{} 时间戳非单调", stat.tag));
        }
        if stat.p99_us > P99_MAX_US {
            failures.push(format!("{} p99 {} us > {P99_MAX_US}", stat.tag, stat.p99_us));
        }
    }
    if host_stats.send_failures > 0 {
        failures.push(format!("host 发送失败/超时 {} 次", host_stats.send_failures));
    }
    if host_stats.main_cpu_us_per_s <= 0.0 {
        failures.push("未采集到主码流 CPU 基线（W4 无法量化）".to_string());
    } else {
        let extra_4 = host_stats.wall_extra_cpu_us_per_s * 4.0 / VIEWERS as f64;
        let budget = host_stats.main_cpu_us_per_s * CPU_BUDGET_RATIO;
        if extra_4 > budget {
            failures.push(format!(
                "4 路外推额外 CPU {extra_4:.0} us/s 超过主码流 30% ({budget:.0} us/s)"
            ));
        }
    }
    if failures.is_empty() {
        println!("[wall] PASS（退出：单进程、0 子进程、会话已关闭）");
        Ok(())
    } else {
        for failure in &failures {
            eprintln!("[wall] FAIL: {failure}");
        }
        Err(format!("{} 项判据未通过", failures.len()).into())
    }
}

/// 日志目录。
///
/// # Errors
/// 目录创建失败时返回错误。
fn ensure_log_dir() -> DemoResult<PathBuf> {
    let dir = PathBuf::from("logs").join("s5-1-wall");
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

/// 写观看者逐帧 CSV。
///
/// # Errors
/// 写文件失败时返回错误。
fn write_viewer_csv(path: &std::path::Path, samples: &[FrameSample]) -> DemoResult<()> {
    use std::fmt::Write as _;
    let mut text = String::from("frame_id,pts_us,recv_us,latency_us,bytes\n");
    for sample in samples {
        let _ = writeln!(
            text,
            "{},{},{},{},{}",
            sample.frame_id, sample.pts_us, sample.recv_us, sample.latency_us, sample.bytes
        );
    }
    write_text(path, &text)
}

/// 写文本文件。
///
/// # Errors
/// 写文件失败时返回错误。
fn write_text(path: &std::path::Path, text: &str) -> DemoResult<()> {
    std::fs::write(path, text)?;
    Ok(())
}

/// host 统计 JSON。
fn host_json(stats: &HostStats) -> String {
    format!(
        "{{\n  \"main_packets\": {},\n  \"wall_packets\": {},\n  \"wall_bytes\": {},\n  \
         \"fragments\": {},\n  \"send_failures\": {},\n  \"backend\": \"{}\",\n  \
         \"main_cpu_us_per_s\": {:.0},\n  \"total_cpu_us_per_s\": {:.0},\n  \
         \"wall_extra_cpu_us_per_s\": {:.0},\n  \"wall_extra_cpu_percent\": {:.2}\n}}\n",
        stats.main_packets,
        stats.wall_packets,
        stats.wall_bytes,
        stats.fragments,
        stats.send_failures,
        stats.backend,
        stats.main_cpu_us_per_s,
        stats.total_cpu_us_per_s,
        stats.wall_extra_cpu_us_per_s,
        stats.wall_extra_cpu_us_per_s / 1_000_000.0 * 100.0
    )
}

/// 组装可读报告。
///
/// # Errors
/// 最新帧汇总锁中毒时返回错误。
fn build_summary(
    host: &HostStats,
    stats: &[ViewerStats],
    hub: &Arc<Mutex<WallPreviewHub>>,
) -> DemoResult<String> {
    use std::fmt::Write as _;
    let mut text = String::new();
    let _ = writeln!(text, "S5-1 demo-wall");
    let _ = writeln!(
        text,
        "子码流：{}x{} @ {} fps / {} kbps（共享缩略图编码器，backend={}）",
        WALL_WIDTH,
        WALL_HEIGHT,
        WALL_FPS,
        WALL_BITRATE_BPS / 1000,
        host.backend
    );
    let _ = writeln!(
        text,
        "host：主码流包 {}，子码流包 {}，子码流 {} KiB，分片 {}，发送失败 {}",
        host.main_packets,
        host.wall_packets,
        host.wall_bytes / 1024,
        host.fragments,
        host.send_failures
    );
    let main_rate = host.main_cpu_us_per_s;
    let extra = host.wall_extra_cpu_us_per_s;
    let extra_4 = extra * 4.0 / VIEWERS as f64;
    let budget = main_rate * CPU_BUDGET_RATIO;
    let _ = writeln!(
        text,
        "CPU：单路主码流（1080p30 编码管线）{:.1}%（每核）；屏幕墙 3 路单独实测 {:.1}%，4 路外推 {:.1}%；预算（主码流 30%）{:.1}%",
        main_rate / 1_000_000.0 * 100.0,
        extra / 1_000_000.0 * 100.0,
        extra_4 / 1_000_000.0 * 100.0,
        budget / 1_000_000.0 * 100.0
    );
    for stat in stats {
        let _ = writeln!(
            text,
            "{}：帧 {}（{:.2} fps），单调={}，p50={} us，p99={} us，max={} us，重组丢弃={}",
            stat.tag,
            stat.frames,
            stat.fps(),
            stat.monotonic,
            stat.p50_us,
            stat.p99_us,
            stat.max_us,
            stat.dropped
        );
    }
    let latest = hub
        .lock()
        .map_err(|_| "最新帧汇总锁中毒".to_string())?
        .latest();
    for (index, frame) in latest.iter().enumerate() {
        match frame {
            Some(frame) => {
                let _ = writeln!(
                    text,
                    "最新帧[流 {}]：frame={} ts={} us {}x{} {} B texture={:?}",
                    index + 1,
                    frame.frame_id,
                    frame.pts_us,
                    frame.width,
                    frame.height,
                    frame.bytes,
                    frame.texture_id
                );
            }
            None => {
                let _ = writeln!(text, "最新帧[流 {}]：无", index + 1);
            }
        }
    }
    Ok(text)
}
