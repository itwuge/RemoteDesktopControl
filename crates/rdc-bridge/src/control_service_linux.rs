//! rdc-bridge / Linux 控制页后端（S8-3）
//!
//! 用途：让 Flutter Linux 桌面壳拥有一套可用的「本机回环」画面链路：
//!   X11 采集 → 软编（外部 ffmpeg / libx264）→ TCP 127.0.0.1 回环 → 外部 ffmpeg 解码
//!   → BGRA→RGBA → Dart 侧 RGBA 帧上屏（FRB 外部纹理在 Linux 上不可用，降级方案）。
//!
//! 与 Windows `control_service` 的关系：接口一一对应（start/stop/status/最新帧），
//! 仅实现不同；DTO 字段完全一致，Dart 侧共用同一套 model/controller。
//!
//! 边界：
//! - 无 GPU：软编软解，1080p 不会达到 60fps，实测值由状态快照如实上报；
//! - 输入注入不经过本模块，由 `input_service` 的 XTest 注入器完成（同一台机器）；
//! - 缩略流（屏幕墙）不在本模块范围，`wall_service` 在 Linux 返回明确的「不支持」。
//!
//! 线程模型：
//! - `rdc-ctl-loop-host`：accept + 采集 + 编码 + 发送（采集回调里只做编码入队，禁止阻塞）；
//! - `rdc-ctl-loop-client`：接收 + 解码 + RGBA 转换 + 更新最新帧；
//! - 两线程都只通过 `Shared` 下的短临界区更新状态，不跨线程持有锁做 IO。

use std::collections::VecDeque;
use std::io::ErrorKind;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, TryLockError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rdc_capture::{create_source, CaptureConfig, ChromaFormat, Frame, FrameSink};
use rdc_codec::{create_encoder, CodecKind, VideoChroma, VideoConfig, VideoEncoder};
use rdc_linux_e2e::clock::mono_ns;
use rdc_linux_e2e::decoder::Decoder;
use rdc_linux_e2e::protocol::{self, Packet, StreamConfig};

use crate::error::lock_poisoned_text;

/// 目标帧率：VM 无 GPU，800p 软编软解在 30fps 附近，1080p 会更低。
const TARGET_FPS: u32 = 30;
/// 目标码率（bit/s）：与 S8-2 一致，便于横向比较。
const TARGET_BITRATE_BPS: u32 = 8_000_000;
/// 等待控制端（本机 client 线程）连接的上限。
const ACCEPT_TIMEOUT_MS: u64 = 6_000;
/// 客户端连接重试上限（host 线程已先 bind，正常情况下首次即成功）。
const CONNECT_RETRY_MS: u64 = 2_000;
/// 延迟样本窗口（p50/p99 基于最近 256 帧）。
const LATENCY_WINDOW: usize = 256;
/// 读超时：让客户端线程能按 `stop` 及时退出，而不是卡在阻塞读上。
const READ_TIMEOUT_MS: u64 = 50;

// =====** 对外快照 =====**

/// 控制页状态快照（字段与 Windows `control_service::ControlSnapshot` 保持一致）。
#[derive(Debug, Clone)]
pub struct ControlSnapshot {
    /// 是否运行。
    pub running: bool,
    /// 采集后端。
    pub backend: String,
    /// 编码器名称。
    pub encoder: String,
    /// 解码器名称。
    pub decoder: String,
    /// 上屏通道（Linux 为 `rgba-frame` 降级通道）。
    pub texture_path: String,
    /// 解码输出路径（Linux 为 `system-memory`）。
    pub decode_path: String,
    /// 画面宽。
    pub width: u32,
    /// 画面高。
    pub height: u32,
    /// 实测帧率。
    pub fps: f64,
    /// 采集帧数。
    pub captured_frames: u64,
    /// 编码包数。
    pub encoded_frames: u64,
    /// 解码帧数。
    pub decoded_frames: u64,
    /// 采集/解码丢帧数。
    pub dropped_frames: u64,
    /// 编码输出累计字节数。
    pub encoded_bytes: u64,
    /// 实测码率（千比特/秒）。
    pub bitrate_kbps: u64,
    /// 最近端到端延迟（微秒）。
    pub latency_last_us: u64,
    /// 延迟 p50（微秒）。
    pub latency_p50_us: u64,
    /// 延迟 p99（微秒）。
    pub latency_p99_us: u64,
    /// 时间码回读次数（Linux 不适用，恒为 0）。
    pub readback_count: u64,
    /// 共享纹理句柄 A（Linux 不适用，恒为 0）。
    pub share_handle_a: i64,
    /// 共享纹理句柄 B（Linux 不适用，恒为 0）。
    pub share_handle_b: i64,
    /// 上屏事件句柄 A（Linux 不适用，恒为 0）。
    pub event_a: i64,
    /// 上屏事件句柄 B（Linux 不适用，恒为 0）。
    pub event_b: i64,
    /// 最近一次错误。
    pub last_error: String,
}

impl ControlSnapshot {
    /// 空状态（未启动时返回）。
    fn idle() -> Self {
        Self {
            running: false,
            backend: String::new(),
            encoder: String::new(),
            decoder: String::new(),
            texture_path: String::new(),
            decode_path: String::new(),
            width: 0,
            height: 0,
            fps: 0.0,
            captured_frames: 0,
            encoded_frames: 0,
            decoded_frames: 0,
            dropped_frames: 0,
            encoded_bytes: 0,
            bitrate_kbps: 0,
            latency_last_us: 0,
            latency_p50_us: 0,
            latency_p99_us: 0,
            readback_count: 0,
            share_handle_a: 0,
            share_handle_b: 0,
            event_a: 0,
            event_b: 0,
            last_error: String::new(),
        }
    }
}

/// 一帧 RGBA 画面（已做 BGRA→RGBA 通道转换，Dart 侧直接 `decodeImageFromPixels`）。
#[derive(Debug, Clone)]
pub struct FrameSnapshot {
    /// 帧序号（自客户端线程启动单调递增）。
    pub frame_id: u64,
    /// 宽。
    pub width: u32,
    /// 高。
    pub height: u32,
    /// 采集时间戳（微秒，host 侧单调时钟）。
    pub pts_us: u64,
    /// 端到端延迟（微秒）。
    pub latency_us: u64,
    /// RGBA8888 像素。
    pub bytes: Vec<u8>,
}

// =====** 内部状态 =====**

struct Shared {
    snapshot: Mutex<ControlSnapshot>,
    latest: Mutex<Option<FrameSnapshot>>,
}

struct Service {
    stop: Arc<AtomicBool>,
    host: JoinHandle<()>,
    client: JoinHandle<()>,
    shared: Arc<Shared>,
}

static STATE: Mutex<Option<Service>> = Mutex::new(None);

/// 采集回调：只做编码入队；拿不到编码器锁就丢帧并计数（采集线程禁止阻塞）。
struct EncodeSink {
    encoder: Arc<Mutex<Box<dyn VideoEncoder>>>,
    shared: Arc<Shared>,
    captured: AtomicU64,
}

impl FrameSink for EncodeSink {
    fn on_frame(&self, frame: Frame) {
        // 端到端延迟必须两端同基准：采集侧时间码（`frame.meta.pts_ns`）与解码侧
        // `mono_ns()` 不是同一时钟（曾测出 p50 ≈ 11,549,xxx ms 的荒谬值）。
        // 本机回环两端同进程，直接用 `mono_ns()` 在帧进入编码队列时打戳，
        // 客户端用同一函数取值相减，得到「采集回调 → 解码出帧」的真实延迟。
        let pts_ns = mono_ns();
        match self.encoder.try_lock() {
            Ok(mut encoder) => {
                if encoder.push(frame.buffer, pts_ns, false).is_ok() {
                    self.captured.fetch_add(1, Ordering::Relaxed);
                } else if let Ok(mut snapshot) = self.shared.snapshot.lock() {
                    snapshot.dropped_frames = snapshot.dropped_frames.saturating_add(1);
                }
            }
            Err(TryLockError::Poisoned(_)) => {}
            Err(TryLockError::WouldBlock) => {
                if let Ok(mut snapshot) = self.shared.snapshot.lock() {
                    snapshot.dropped_frames = snapshot.dropped_frames.saturating_add(1);
                }
            }
        }
    }
}

// =====** 生命周期 =====**

/// 启动本机回环控制链路。
///
/// # Returns
/// 启动后的状态快照（`texture_path = "rgba-frame"` 表示走 RGBA 降级通道）。
///
/// # Errors
/// X11 采集不可用、编码器无法配置或端口无法绑定时返回错误文本。
pub fn start() -> Result<ControlSnapshot, String> {
    let mut guard = STATE.lock().map_err(|poison| lock_poisoned_text("控制服务状态锁被污染", poison))?;
    if let Some(service) = guard.as_ref() {
        if service.shared.snapshot.lock().map(|s| s.running).unwrap_or(false) {
            drop(guard);
            return status();
        }
        // 线程已退出但状态未清理：先回收再重启。
        let _ = guard.take(); // gate-allow R-C-2: 非错误值，清空已退出服务的状态槽，旧 Service 随 take 释放
    }
    drop(guard);

    let (width, height, display_id, backend) = probe_display()?;

    let listener = TcpListener::bind("127.0.0.1:0").map_err(|e| format!("回环端口绑定失败: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("读取回环端口失败: {e}"))?
        .port();

    let shared = Arc::new(Shared {
        snapshot: Mutex::new(ControlSnapshot {
            running: true,
            backend,
            encoder: "linux-ffmpeg-x264".to_owned(),
            decoder: "ffmpeg-h264-sw".to_owned(),
            texture_path: "rgba-frame".to_owned(),
            decode_path: "system-memory".to_owned(),
            width,
            height,
            ..ControlSnapshot::idle()
        }),
        latest: Mutex::new(None),
    });
    let stop = Arc::new(AtomicBool::new(false));

    let host_shared = Arc::clone(&shared);
    let host_stop = Arc::clone(&stop);
    let host = thread::Builder::new()
        .name("rdc-ctl-loop-host".into())
        .spawn(move || {
            let _ = host_main(listener, host_stop, host_shared, display_id); // gate-allow R-C-2: 线程入口把错误写入共享快照，返回值无需重复上报
        })
        .map_err(|e| format!("回环 host 线程创建失败: {e}"))?;

    let client_shared = Arc::clone(&shared);
    let client_stop = Arc::clone(&stop);
    let client = thread::Builder::new()
        .name("rdc-ctl-loop-client".into())
        .spawn(move || {
            let _ = client_main(port, client_stop, client_shared); // gate-allow R-C-2: 线程入口把错误写入共享快照，返回值无需重复上报
        })
        .map_err(|e| format!("回环 client 线程创建失败: {e}"))?;

    let mut guard = STATE.lock().map_err(|poison| lock_poisoned_text("控制服务状态锁被污染", poison))?;
    *guard = Some(Service {
        stop,
        host,
        client,
        shared: Arc::clone(&shared),
    });
    drop(guard);

    // 等 host 侧完成采集初始化，让首帧前状态里就带真实宽高与后端名。
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        if let Ok(snapshot) = shared.snapshot.lock() {
            if snapshot.width > 0 && snapshot.running {
                break;
            }
            if !snapshot.last_error.is_empty() {
                return Err(snapshot.last_error.clone());
            }
        }
        thread::sleep(Duration::from_millis(10));
    }

    status()
}

/// 停止回环链路并回收线程。
///
/// # Returns
/// 停止后的状态快照。
///
/// # Errors
/// 状态锁被污染时返回错误文本。
pub fn stop() -> Result<ControlSnapshot, String> {
    let service = {
        let mut guard = STATE.lock().map_err(|poison| lock_poisoned_text("控制服务状态锁被污染", poison))?;
        guard.take()
    };
    let Some(service) = service else {
        return Ok(ControlSnapshot::idle());
    };
    service.stop.store(true, Ordering::SeqCst);
    let _ = service.host.join(); // gate-allow R-C-2: 停链路时回收 host 线程，join 失败不改变停止结论
    let _ = service.client.join(); // gate-allow R-C-2: 同上，回收 client 线程
    let mut snapshot = service
        .shared
        .snapshot
        .lock()
        .map_err(|poison| lock_poisoned_text("控制服务状态锁被污染", poison))?
        .clone();
    snapshot.running = false;
    if let Ok(mut latest) = service.shared.latest.lock() {
        latest.take();
    }
    Ok(snapshot)
}

/// 读取当前状态快照。
///
/// # Returns
/// 运行中返回实时状态；未启动返回空状态。
///
/// # Errors
/// 状态锁被污染时返回错误文本。
pub fn status() -> Result<ControlSnapshot, String> {
    let shared = {
        let guard = STATE.lock().map_err(|poison| lock_poisoned_text("控制服务状态锁被污染", poison))?;
        guard.as_ref().map(|service| Arc::clone(&service.shared))
    };
    let Some(shared) = shared else {
        return Ok(ControlSnapshot::idle());
    };
    let snapshot = shared
        .snapshot
        .lock()
        .map_err(|poison| lock_poisoned_text("控制服务状态锁被污染", poison))?
        .clone();
    Ok(snapshot)
}

/// 取最新一帧 RGBA 画面。
///
/// # Returns
/// 已有解码帧返回 `Some`；尚未出首帧返回 `None`。
///
/// # Errors
/// 状态锁被污染时返回错误文本。
pub fn latest_frame() -> Result<Option<FrameSnapshot>, String> {
    let shared = {
        let guard = STATE.lock().map_err(|poison| lock_poisoned_text("控制服务状态锁被污染", poison))?;
        guard.as_ref().map(|service| Arc::clone(&service.shared))
    };
    let Some(shared) = shared else {
        return Ok(None);
    };
    let frame = shared
        .latest
        .lock()
        .map_err(|poison| lock_poisoned_text("控制服务状态锁被污染", poison))?
        .clone();
    Ok(frame)
}

// =====** 显示器探测 =====**

/// 枚举当前显示器，返回 `(宽, 高, display_id, backend)`。
///
/// 说明：这里单独建一个采集源只为读分辨率；真正的采集源在 host 线程内创建，
/// 以满足采集会话的线程亲和性。
fn probe_display() -> Result<(u32, u32, u32, String), String> {
    let source = create_source().map_err(|e| format!("创建采集源失败: {e}"))?;
    let displays = source.enumerate();
    let display = displays
        .iter()
        .find(|d| d.is_primary)
        .or_else(|| displays.first())
        .ok_or_else(|| "未枚举到任何显示器（DISPLAY 是否可用？）".to_owned())?;
    Ok((
        display.width,
        display.height,
        display.id,
        source.backend_name().to_owned(),
    ))
}

// =====** host 侧 =====**

fn host_main(
    listener: TcpListener,
    stop: Arc<AtomicBool>,
    shared: Arc<Shared>,
    display_id: u32,
) -> Result<(), String> {
    let result = host_run(listener, &stop, &shared, display_id);
    if let Err(error) = &result {
        if let Ok(mut snapshot) = shared.snapshot.lock() {
            snapshot.last_error = error.clone();
            snapshot.running = false;
        }
    }
    result
}

fn host_run(
    listener: TcpListener,
    stop: &Arc<AtomicBool>,
    shared: &Arc<Shared>,
    display_id: u32,
) -> Result<(), String> {
    let (width, height) = {
        let snapshot = shared.snapshot.lock().map_err(|poison| lock_poisoned_text("状态锁被污染", poison))?;
        (snapshot.width, snapshot.height)
    };

    let mut encoder = create_encoder(CodecKind::H264).map_err(|e| format!("创建编码器失败: {e}"))?;
    encoder
        .configure(VideoConfig {
            codec: CodecKind::H264,
            width,
            height,
            fps: TARGET_FPS,
            bitrate_bps: TARGET_BITRATE_BPS,
            chroma: VideoChroma::Yuv420,
            bit_depth: 8,
            low_latency: true,
        })
        .map_err(|e| format!("编码器配置失败: {e}"))?;
    let encoder = Arc::new(Mutex::new(encoder));

    // accept：非阻塞 + 轮询，避免 stop 时卡死。
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("监听端口设置非阻塞失败: {e}"))?;
    let deadline = Instant::now() + Duration::from_millis(ACCEPT_TIMEOUT_MS);
    let mut stream = loop {
        if stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        match listener.accept() {
            Ok((stream, _)) => break stream,
            Err(error) if error.kind() == ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(format!(
                        "控制端未在 {} ms 内连接回环端口",
                        ACCEPT_TIMEOUT_MS
                    ));
                }
                thread::sleep(Duration::from_millis(5));
            }
            Err(error) => return Err(format!("接受回环连接失败: {error}")),
        }
    };
    stream
        .set_nodelay(true)
        .map_err(|e| format!("回环连接设置 TCP_NODELAY 失败: {e}"))?;
    stream
        .set_write_timeout(Some(Duration::from_millis(2000)))
        .map_err(|e| format!("设置写超时失败: {e}"))?;

    let config = StreamConfig {
        width,
        height,
        fps: TARGET_FPS,
    };
    Packet {
        kind: protocol::KIND_CONFIG,
        payload: config.to_payload(),
    }
    .write_to(&mut stream)
    .map_err(|e| format!("发送流配置失败: {e}"))?;

    let mut source = create_source().map_err(|e| format!("创建采集源失败: {e}"))?;
    let sink = Arc::new(EncodeSink {
        encoder: Arc::clone(&encoder),
        shared: Arc::clone(shared),
        captured: AtomicU64::new(0),
    });
    source.set_sink(sink.clone());
    source
        .start(CaptureConfig {
            display_id,
            target_fps: TARGET_FPS,
            include_cursor: false,
            chroma: ChromaFormat::Bgra,
            bit_depth: 8,
        })
        .map_err(|e| format!("启动采集失败: {e}"))?;
    if let Ok(mut snapshot) = shared.snapshot.lock() {
        snapshot.backend = source.backend_name().to_owned();
    }

    let mut window_start = Instant::now();
    let mut window_frames = 0u64;
    let mut window_bytes = 0u64;
    let mut encoded = 0u64;
    let mut encoded_bytes = 0u64;

    while !stop.load(Ordering::SeqCst) {
        let packet = match encoder.try_lock() {
            Ok(mut guard) => guard.poll_packet(),
            Err(TryLockError::WouldBlock) => None,
            Err(TryLockError::Poisoned(_)) => return Err("编码器锁被污染".to_owned()),
        };
        if let Some(packet) = packet {
            let payload = protocol::video_payload(packet.pts_ns, packet.is_keyframe, &packet.data);
            let bytes = payload.len() + 5;
            let outbound = Packet {
                kind: protocol::KIND_VIDEO,
                payload,
            };
            if outbound.write_to(&mut stream).is_err() {
                // 对端断开：正常收尾，不算错误。
                break;
            }
            encoded = encoded.saturating_add(1);
            encoded_bytes = encoded_bytes.saturating_add(bytes as u64);
            window_frames = window_frames.saturating_add(1);
            window_bytes = window_bytes.saturating_add(bytes as u64);
        }

        if window_start.elapsed() >= Duration::from_millis(1000) {
            let secs = window_start.elapsed().as_secs_f64();
            if let Ok(mut snapshot) = shared.snapshot.lock() {
                snapshot.captured_frames = sink.captured.load(Ordering::Relaxed);
                snapshot.encoded_frames = encoded;
                snapshot.encoded_bytes = encoded_bytes;
                snapshot.bitrate_kbps = ((window_bytes as f64 * 8.0 / 1000.0) / secs) as u64;
                let _ = window_frames; // gate-allow R-C-2: 非错误值，window_frames 是预留计数变量，此处显式消费以避免未使用告警
            }
            window_start = Instant::now();
            window_frames = 0;
            window_bytes = 0;
        }
        thread::sleep(Duration::from_millis(1));
    }

    // 收尾：把编码器缓存的尾包发完，再告知对端。
    if let Ok(mut guard) = encoder.lock() {
        if let Ok(tail) = guard.flush() {
            for packet in tail {
                let payload =
                    protocol::video_payload(packet.pts_ns, packet.is_keyframe, &packet.data);
                let _ = Packet { // gate-allow R-C-2: 冲刷尾包到已建立的流，写失败说明对端已断开，无恢复动作
                    kind: protocol::KIND_VIDEO,
                    payload,
                }
                .write_to(&mut stream);
            }
        }
    }
    let _ = Packet { // gate-allow R-C-2: 结束通知写给已建立的流，写失败说明对端已断开，无恢复动作
        kind: protocol::KIND_BYE,
        payload: Vec::new(),
    }
    .write_to(&mut stream);
    source.stop();
    Ok(())
}

// =====** client 侧 =====**

fn client_main(port: u16, stop: Arc<AtomicBool>, shared: Arc<Shared>) -> Result<(), String> {
    let result = client_run(port, &stop, &shared);
    if let Err(error) = &result {
        if let Ok(mut snapshot) = shared.snapshot.lock() {
            snapshot.last_error = error.clone();
            snapshot.running = false;
        }
    }
    result
}

fn client_run(port: u16, stop: &Arc<AtomicBool>, shared: &Arc<Shared>) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_millis(CONNECT_RETRY_MS);
    let mut stream = loop {
        match TcpStream::connect(("127.0.0.1", port)) {
            Ok(stream) => break stream,
            Err(error) => {
                if stop.load(Ordering::SeqCst) {
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    return Err(format!("连接本机回环端口失败: {error}"));
                }
                thread::sleep(Duration::from_millis(20));
            }
        }
    };
    stream
        .set_nodelay(true)
        .map_err(|e| format!("回环连接设置 TCP_NODELAY 失败: {e}"))?;
    stream
        .set_read_timeout(Some(Duration::from_millis(READ_TIMEOUT_MS)))
        .map_err(|e| format!("设置读超时失败: {e}"))?;

    // 首个包必须是配置包。
    let config = loop {
        if stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        match protocol::read_packet(&mut stream) {
            Ok(Some(packet)) if packet.kind == protocol::KIND_CONFIG => {
                break StreamConfig::from_payload(&packet.payload)
                    .ok_or_else(|| "回环配置包格式错误".to_owned())?;
            }
            Ok(Some(_)) => continue,
            Ok(None) => return Ok(()),
            Err(error) if is_timeout(&error) => continue,
            Err(error) => return Err(format!("读回环配置失败: {error}")),
        }
    };

    let mut decoder = Decoder::spawn(config.width, config.height)?;
    let mut latencies: VecDeque<u64> = VecDeque::with_capacity(LATENCY_WINDOW);
    let mut frame_id = 0u64;
    let mut fps_window = Instant::now();
    let mut fps_frames = 0u64;

    loop {
        if stop.load(Ordering::SeqCst) {
            break;
        }
        match protocol::read_packet(&mut stream) {
            Ok(Some(packet)) => match packet.kind {
                protocol::KIND_VIDEO => {
                    let Some((pts_ns, _keyframe, data)) = protocol::parse_video(&packet.payload)
                    else {
                        continue;
                    };
                    if decoder.push(data, pts_ns).is_err() {
                        break;
                    }
                    while let Some(decoded) = decoder.poll() {
                        let now = mono_ns();
                        let latency_us = now.saturating_sub(decoded.pts_ns) / 1000;
                        latencies.push_back(latency_us);
                        if latencies.len() > LATENCY_WINDOW {
                            latencies.pop_front();
                        }
                        frame_id = frame_id.saturating_add(1);
                        fps_frames = fps_frames.saturating_add(1);

                        // 就地 BGRA→RGBA（不额外分配第二块缓冲）。
                        let mut rgba = decoded.data;
                        for pixel in rgba.chunks_exact_mut(4) {
                            pixel.swap(0, 2);
                            pixel[3] = 255;
                        }

                        let mut snapshot = shared.snapshot.lock().map_err(|poison| lock_poisoned_text("状态锁被污染", poison))?;
                        snapshot.decoded_frames = frame_id;
                        snapshot.dropped_frames = decoder.dropped();
                        snapshot.latency_last_us = latency_us;
                        drop(snapshot);

                        if let Ok(mut latest) = shared.latest.lock() {
                            *latest = Some(FrameSnapshot {
                                frame_id,
                                width: config.width,
                                height: config.height,
                                pts_us: decoded.pts_ns / 1000,
                                latency_us,
                                bytes: rgba,
                            });
                        }
                    }
                }
                protocol::KIND_BYE => break,
                _ => {}
            },
            Ok(None) => break,
            Err(error) if is_timeout(&error) => continue,
            Err(error) => return Err(format!("读回环包失败: {error}")),
        }

        if fps_window.elapsed() >= Duration::from_millis(1000) {
            let secs = fps_window.elapsed().as_secs_f64();
            let mut sorted: Vec<u64> = latencies.iter().copied().collect();
            sorted.sort_unstable();
            let p50 = percentile(&sorted, 0.50);
            let p99 = percentile(&sorted, 0.99);
            if let Ok(mut snapshot) = shared.snapshot.lock() {
                snapshot.fps = fps_frames as f64 / secs;
                snapshot.latency_p50_us = p50;
                snapshot.latency_p99_us = p99;
            }
            fps_window = Instant::now();
            fps_frames = 0;
        }
    }

    decoder.finish();
    Ok(())
}

fn is_timeout(error: &std::io::Error) -> bool {
    matches!(error.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut)
}

fn percentile(sorted: &[u64], ratio: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let index = ((sorted.len() as f64 - 1.0) * ratio).round() as usize;
    sorted[index.min(sorted.len() - 1)]
}
