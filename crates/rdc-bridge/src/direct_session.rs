// ============================================================
// 文件用途：直连会话 —— TCP 套接字的所有者（阶段 6）
//
// 层级：L4 桥接层
// 被谁调用：api.rs 的 direct_connect / direct_disconnect / direct_status /
//           direct_send_input；Flutter 直连页只负责输入参数与展示。
//
// 为什么 socket 放 Rust 而不是 Dart：
//   1) 接收线程需要一个固定内存预算的队列，Flutter isolate 不适合长期 socket 泵；
//   2) 解码前要合成 VideoMessage 再进既有重组/轮询管线，跨 FFI 搬 Annex-B 会翻倍拷贝；
//   3) 触摸/文本事件必须在同一层完成 Pointer 语义换算与编码，避免 Dart 侧再实现一遍线格式。
//
// 对端协议：Linux 被控端 `rdc-serve` 的 S8-2 最小推流协议（明文 TCP，非 docs/ 生产协议）：
//         `kind u8 | len u32 LE | payload[len]`，
//         kind 3=CONFIG(width,height,fps)、1=VIDEO(pts_ns,flags,Annex-B)、
//         2=INPUT(rdc_input 单事件)、4=BYE。
//         说明见 RDC-linux/research/android-connect-linux.md。
//
// 边界：本轮不做配对校验（对端协议无配对字段）；不发明新报文类型。
// ============================================================

use std::collections::VecDeque;
use std::io::{ErrorKind, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rdc_protocol::{
    VideoCodec, VideoColorRange, VideoColorSpace, VideoFrameHeader, VideoMessage,
    VideoStreamConfig, VIDEO_FRAGMENT_BYTES, VIDEO_MAX_FRAME_BYTES, VIDEO_STREAM_MAIN,
};

use crate::input_service::InputEventDto;

// =====** 常量 =====**

/// 协议包类型：视频（host → client）。
const KIND_VIDEO: u8 = 1;
/// 协议包类型：输入（client → host）。
const KIND_INPUT: u8 = 2;
/// 协议包类型：配置（host → client）。
const KIND_CONFIG: u8 = 3;
/// 协议包类型：断开（双向）。
const KIND_BYE: u8 = 4;
/// 单包载荷上限（与对端 `MAX_PAYLOAD` 一致）。
const MAX_PAYLOAD: usize = 8 * 1024 * 1024;
/// 连接超时。
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// 等待 CONFIG 的超时：对端要先起采集才会发。
const CONFIG_TIMEOUT: Duration = Duration::from_secs(8);
/// 帧队列内存预算：编码帧最多占用 16 MiB（Windows 端无界队列曾涨到 11.7 GB）。
const QUEUE_MAX_BYTES: usize = 16 * 1024 * 1024;
/// 帧队列条数上限（防止大量小帧把元数据撑爆）。
const QUEUE_MAX_ITEMS: usize = 64;
/// 等 CONFIG 阶段的读超时（只用于轮询截止时间）。
const CONFIG_POLL_TIMEOUT: Duration = Duration::from_millis(200);
/// 输入写超时：对端卡住时不无限阻塞调用方。
const WRITE_TIMEOUT: Duration = Duration::from_secs(1);
/// 内部哨兵：`read_packet` 的“本轮无数据”，调用方继续循环而不是当错误。
const RETRY: &str = "__RETRY__";

// =====** 错误码 =====**

/// 参数非法（IP 为空、端口越界）。
pub const E_ARGS: &str = "E_ARGS";
/// 地址解析失败。
pub const E_RESOLVE: &str = "E_RESOLVE";
/// 连接被拒：被控端未启动或端口不可达。
pub const E_CONNECT_REFUSED: &str = "E_CONNECT_REFUSED";
/// 连接超时：网络不可达或被防火墙丢弃。
pub const E_CONNECT_TIMEOUT: &str = "E_CONNECT_TIMEOUT";
/// 已连接但迟迟没有 CONFIG：被控端忙（单会话）或采集未启动。
pub const E_CONFIG_TIMEOUT: &str = "E_CONFIG_TIMEOUT";
/// 包头/长度/版本非法。
pub const E_PROTOCOL: &str = "E_PROTOCOL";
/// 读写出错（对端关闭、网络中断）。
pub const E_IO: &str = "E_IO";
/// 对端主动 BYE。
pub const E_BYE: &str = "E_BYE";

fn coded(code: &str, message: impl Into<String>) -> String {
    format!("{code}: {}", message.into())
}

// =====** DTO =====**

/// 直连会话状态与计数（Flutter 侧诊断展示用）。
#[derive(Debug, Clone, Default)]
pub struct DirectSessionDto {
    /// 是否已建立并正在接收。
    pub connected: bool,
    /// 目标主机（原样回显）。
    pub host: String,
    /// 目标端口。
    pub port: u16,
    /// 是否已收到 CONFIG。
    pub configured: bool,
    /// 远端画面宽（CONFIG）。
    pub width: u32,
    /// 远端画面高（CONFIG）。
    pub height: u32,
    /// 远端帧率（CONFIG）。
    pub fps: u32,
    /// 收到的字节数。
    pub bytes_received: u64,
    /// 收到的视频帧数（进入队列前）。
    pub frames_received: u64,
    /// 因内存预算被丢掉的帧数（丢最旧）。
    pub frames_dropped: u64,
    /// 合成/重组失败数。
    pub ingest_errors: u64,
    /// 已发送输入事件数。
    pub input_sent: u64,
    /// 输入发送失败数。
    pub input_errors: u64,
    /// 最近错误（`CODE: 人话`）。
    pub last_error: Option<String>,
}

// =====** 有界帧队列 =====**

struct FrameItem {
    config_id: u32,
    pts_us: u64,
    keyframe: bool,
    data: Vec<u8>,
}

struct QueueInner {
    items: VecDeque<FrameItem>,
    bytes: usize,
}

/// 内存预算 + 条数双上限的帧队列：满时丢最旧，内存不随网络速率增长。
struct FrameQueue {
    inner: Mutex<QueueInner>,
    cv: Condvar,
    dropped: AtomicU64,
    closed: AtomicBool,
}

impl FrameQueue {
    fn new() -> Self {
        Self {
            inner: Mutex::new(QueueInner {
                items: VecDeque::with_capacity(QUEUE_MAX_ITEMS),
                bytes: 0,
            }),
            cv: Condvar::new(),
            dropped: AtomicU64::new(0),
            closed: AtomicBool::new(false),
        }
    }

    fn push(&self, item: FrameItem) {
        if self.closed.load(Ordering::Relaxed) {
            return;
        }
        let mut guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        while guard.items.len() >= QUEUE_MAX_ITEMS
            || guard.bytes.saturating_add(item.data.len()) > QUEUE_MAX_BYTES
        {
            let Some(old) = guard.items.pop_front() else {
                break;
            };
            guard.bytes = guard.bytes.saturating_sub(old.data.len());
            self.dropped.fetch_add(1, Ordering::Relaxed);
            if guard.items.is_empty() && item.data.len() > QUEUE_MAX_BYTES {
                // 单帧比预算还大：不排队，直接记一次丢弃。
                self.dropped.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
        guard.bytes = guard.bytes.saturating_add(item.data.len());
        guard.items.push_back(item);
        self.cv.notify_one();
    }

    fn pop(&self) -> Option<FrameItem> {
        let mut guard = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        loop {
            if let Some(item) = guard.items.pop_front() {
                guard.bytes = guard.bytes.saturating_sub(item.data.len());
                return Some(item);
            }
            if self.closed.load(Ordering::Relaxed) {
                return None;
            }
            let (next, _) = self
                .cv
                .wait_timeout(guard, Duration::from_millis(50))
                .unwrap_or_else(|p| p.into_inner());
            guard = next;
        }
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Relaxed);
        self.cv.notify_all();
    }
}

// =====** 计数 =====**

#[derive(Default)]
struct Counters {
    bytes_received: AtomicU64,
    frames_received: AtomicU64,
    ingest_errors: AtomicU64,
    input_sent: AtomicU64,
    input_errors: AtomicU64,
    last_error: Mutex<Option<String>>,
}

impl Counters {
    fn set_error(&self, message: impl Into<String>) {
        if let Ok(mut slot) = self.last_error.lock() {
            *slot = Some(message.into());
        }
    }

    fn error(&self) -> Option<String> {
        self.last_error.lock().ok().and_then(|slot| slot.clone())
    }
}

// =====** 会话实例 =====**

/// 当前生效的视频配置。
///
/// `config_id` 是本地合成的版本号：对端 S8-2 的 CONFIG 没有版本字段，
/// 而本地重组器要求 Meta 与 Frame 的 `config_id` 一致，且 Dart 靠版本号
/// 判断是否需要重建解码器。因此只在宽高/帧率真正变化时递增。
struct ConfigState {
    width: u32,
    height: u32,
    fps: u32,
    config_id: u32,
    /// 是否已经向 `video_service` 投递过 Meta。
    ingested: bool,
}

impl ConfigState {
    fn new(width: u32, height: u32, fps: u32, config_id: u32) -> Self {
        Self {
            width,
            height,
            fps,
            config_id,
            ingested: true,
        }
    }
}

/// 本地配置版本号发生器（进程内单调递增）。
static CONFIG_SEQ: AtomicU32 = AtomicU32::new(0);

fn next_config_id() -> u32 {
    CONFIG_SEQ.fetch_add(1, Ordering::Relaxed).wrapping_add(1)
}

struct DirectSession {
    stream: Arc<TcpStream>,
    host_text: String,
    port: u16,
    stop: Arc<AtomicBool>,
    queue: Arc<FrameQueue>,
    counters: Arc<Counters>,
    config: Arc<Mutex<ConfigState>>,
    threads: Vec<JoinHandle<()>>,
    write_lock: Mutex<()>,
}

fn slot() -> &'static Mutex<Option<DirectSession>> {
    static SLOT: OnceLock<Mutex<Option<DirectSession>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

fn lock_slot() -> std::sync::MutexGuard<'static, Option<DirectSession>> {
    slot().lock().unwrap_or_else(|p| p.into_inner())
}

// =====** 线格式 =====**

/// 读一个完整协议包，返回 (kind, payload)。
fn read_packet(stream: &mut TcpStream) -> Result<(u8, Vec<u8>), String> {
    let mut header = [0u8; 5];
    stream
        .read_exact(&mut header)
        .map_err(|error| match error.kind() {
            ErrorKind::WouldBlock | ErrorKind::TimedOut => RETRY.to_owned(),
            ErrorKind::UnexpectedEof => coded(E_IO, "被控端关闭了连接"),
            _ => coded(E_IO, format!("读取包头失败: {error}")),
        })?;
    let kind = header[0];
    let len = u32::from_le_bytes([header[1], header[2], header[3], header[4]]) as usize;
    if len > MAX_PAYLOAD {
        return Err(coded(
            E_PROTOCOL,
            format!("载荷长度 {len} 超过上限 {MAX_PAYLOAD}"),
        ));
    }
    let mut payload = vec![0u8; len];
    stream
        .read_exact(&mut payload)
        .map_err(|error| coded(E_IO, format!("读取载荷失败: {error}")))?;
    Ok((kind, payload))
}

/// 解析 CONFIG：`width u32 | height u32 | fps u32`（LE）。
fn parse_config(payload: &[u8]) -> Result<(u32, u32, u32), String> {
    if payload.len() < 12 {
        return Err(coded(
            E_PROTOCOL,
            format!("CONFIG 载荷 {} 字节，少于 12", payload.len()),
        ));
    }
    let width = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
    let height = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
    let fps = u32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]);
    if width == 0 || height == 0 {
        return Err(coded(E_PROTOCOL, "CONFIG 宽高为 0"));
    }
    Ok((width, height, fps))
}

/// 构造 Meta 消息并送入既有重组管线。
fn ingest_config(width: u32, height: u32, fps: u32, config_id: u32) {
    let message = VideoMessage::Meta(VideoStreamConfig {
        stream_id: VIDEO_STREAM_MAIN,
        config_id,
        width: width.min(u16::MAX as u32) as u16,
        height: height.min(u16::MAX as u32) as u16,
        fps: fps.min(u16::MAX as u32) as u16,
        codec: VideoCodec::H264,
        color_space: VideoColorSpace::Bt709,
        color_range: VideoColorRange::Limited,
        parameter_sets_in_band: true,
        sps: Vec::new(),
        pps: Vec::new(),
    });
    let _ = crate::video_service::ingest_message(message);
}

/// 把一整帧 Annex-B 合成为 `Frame + Fragment` 消息并送入重组管线。
///
/// 对端按整帧发送；本地重组器仍要求 Frame/Fragment 结构，所以这里做一层
/// 「整帧 → 分片」适配，不改对端协议、也不改重组器线格式。
///
/// 返回：单帧超过 `VIDEO_MAX_FRAME_BYTES` 时返回 `E_PROTOCOL`；
/// 其它情况由重组器内部记账（坏帧只丢该帧，不 panic）。
fn ingest_frame(
    config_id: u32,
    frame_id: u32,
    pts_us: u64,
    keyframe: bool,
    data: &[u8],
) -> Result<(), String> {
    if data.is_empty() {
        return Ok(());
    }
    if data.len() > VIDEO_MAX_FRAME_BYTES {
        return Err(coded(
            E_PROTOCOL,
            format!("单帧 {} 字节超过上限", data.len()),
        ));
    }
    let frag_count = data.len().div_ceil(VIDEO_FRAGMENT_BYTES) as u16;
    let _ = crate::video_service::ingest_message(VideoMessage::Frame(VideoFrameHeader {
        stream_id: VIDEO_STREAM_MAIN,
        config_id,
        frame_id,
        pts_us,
        keyframe,
        total_len: data.len() as u32,
        frag_count,
    }));
    for (index, chunk) in data.chunks(VIDEO_FRAGMENT_BYTES).enumerate() {
        let _ = crate::video_service::ingest_message(VideoMessage::Fragment {
            stream_id: VIDEO_STREAM_MAIN,
            frame_id,
            frag_index: index as u16,
            frag_count,
            data: chunk.to_vec(),
        });
    }
    Ok(())
}

// =====** 生命周期 =====**

/// 建立直连会话：TCP 连接 → 等 CONFIG → 启动有界接收/重组线程。
///
/// 单会话模型：已有连接会先断开，避免两套帧号/队列串台。
///
/// # Arguments
/// * `host` - 被控端 IP 或域名
/// * `port` - 被控端 TCP 端口
///
/// # Errors
/// 返回 `CODE: 人话`（`E_ARGS/E_RESOLVE/E_CONNECT_REFUSED/E_CONNECT_TIMEOUT/
/// E_CONFIG_TIMEOUT/E_PROTOCOL/E_IO/E_BYE`）。
pub fn connect(host: String, port: u16) -> Result<DirectSessionDto, String> {
    let host = host.trim().to_owned();
    if host.is_empty() {
        return Err(coded(E_ARGS, "请填写被控端 IP"));
    }
    if port == 0 {
        return Err(coded(E_ARGS, "端口不能为 0"));
    }

    disconnect();

    let target = resolve(&host, port)?;
    let stream = connect_stream(&target)?;
    stream
        .set_nodelay(true)
        .map_err(|error| coded(E_IO, format!("设置 TCP_NODELAY 失败: {error}")))?;
    stream
        .set_read_timeout(Some(CONFIG_POLL_TIMEOUT))
        .map_err(|error| coded(E_IO, format!("设置读超时失败: {error}")))?;

    // 建会话后等 CONFIG：对端要先起采集才发，最长 8 s。
    let mut reader = stream
        .try_clone()
        .map_err(|error| coded(E_IO, format!("复制 socket 失败: {error}")))?;
    let (width, height, fps) = wait_for_config(&mut reader)?;

    // 进入长期接收阶段：改回阻塞读，避免超时把半包截断导致协议错位。
    // 断开由 `disconnect()` 的 shutdown 唤醒阻塞读。
    stream
        .set_read_timeout(None)
        .map_err(|error| coded(E_IO, format!("清除读超时失败: {error}")))?;
    stream
        .set_write_timeout(Some(WRITE_TIMEOUT))
        .map_err(|error| coded(E_IO, format!("设置写超时失败: {error}")))?;

    let _ = crate::video_service::reset();
    let config_id = next_config_id();
    ingest_config(width, height, fps, config_id);

    let counters = Arc::new(Counters::default());
    let queue = Arc::new(FrameQueue::new());
    let stop = Arc::new(AtomicBool::new(false));
    let config = Arc::new(Mutex::new(ConfigState::new(width, height, fps, config_id)));
    let stream = Arc::new(stream);

    // 读线程：解析协议包 → 帧入有界队列；CONFIG/BYE 就地处理。
    let read_stream = Arc::clone(&stream);
    let read_queue = Arc::clone(&queue);
    let read_counters = Arc::clone(&counters);
    let read_config = Arc::clone(&config);
    let read_stop = Arc::clone(&stop);
    let read_handle = thread::spawn(move || {
        let mut local = match read_stream.try_clone() {
            Ok(clone) => clone,
            Err(error) => {
                read_counters.set_error(coded(E_IO, format!("复制 socket 失败: {error}")));
                read_queue.close();
                return;
            }
        };
        loop {
            if read_stop.load(Ordering::Relaxed) {
                break;
            }
            let (kind, payload) = match read_packet(&mut local) {
                Ok(packet) => packet,
                Err(error) => {
                    if error == RETRY {
                        continue;
                    }
                    if !read_stop.load(Ordering::Relaxed) {
                        read_counters.set_error(error);
                    }
                    break;
                }
            };
            match kind {
                KIND_VIDEO => {
                    if payload.len() < 9 {
                        read_counters
                            .ingest_errors
                            .fetch_add(1, Ordering::Relaxed);
                        continue;
                    }
                    let pts_ns = u64::from_le_bytes([
                        payload[0], payload[1], payload[2], payload[3], payload[4], payload[5],
                        payload[6], payload[7],
                    ]);
                    let flags = payload[8];
                    let data = payload[9..].to_vec();
                    let frame_config_id = {
                        let guard = read_config.lock().unwrap_or_else(|p| p.into_inner());
                        guard.config_id
                    };
                    read_counters
                        .bytes_received
                        .fetch_add(data.len() as u64, Ordering::Relaxed);
                    read_counters
                        .frames_received
                        .fetch_add(1, Ordering::Relaxed);
                    read_queue.push(FrameItem {
                        config_id: frame_config_id,
                        pts_us: pts_ns / 1000,
                        keyframe: flags & 0x01 != 0,
                        data,
                    });
                }
                KIND_CONFIG => match parse_config(&payload) {
                    Ok((width, height, fps)) => {
                        // 同尺寸的重复 CONFIG 直接忽略：避免无谓重配与等 IDR。
                        let changed = {
                            let mut guard =
                                read_config.lock().unwrap_or_else(|p| p.into_inner());
                            if guard.ingested
                                && guard.width == width
                                && guard.height == height
                                && guard.fps == fps
                            {
                                None
                            } else {
                                guard.width = width;
                                guard.height = height;
                                guard.fps = fps;
                                guard.config_id = next_config_id();
                                guard.ingested = true;
                                Some(guard.config_id)
                            }
                        };
                        if let Some(config_id) = changed {
                            ingest_config(width, height, fps, config_id);
                        }
                    }
                    Err(error) => read_counters.set_error(error),
                },
                KIND_BYE => {
                    read_counters.set_error(coded(E_BYE, "被控端主动断开"));
                    break;
                }
                _ => {}
            }
        }
        read_queue.close();
    });

    // 重组线程：帧 → Frame/Fragment → video_service → Dart 轮询。
    let work_queue = Arc::clone(&queue);
    let work_counters = Arc::clone(&counters);
    let work_handle = thread::spawn(move || {
        let mut frame_id = 0u32;
        while let Some(item) = work_queue.pop() {
            frame_id = frame_id.wrapping_add(1);
            if let Err(error) = ingest_frame(
                item.config_id,
                frame_id,
                item.pts_us,
                item.keyframe,
                &item.data,
            ) {
                work_counters.ingest_errors.fetch_add(1, Ordering::Relaxed);
                work_counters.set_error(error);
            }
        }
    });

    let session = DirectSession {
        stream,
        host_text: host,
        port,
        stop,
        queue,
        counters,
        config,
        threads: vec![read_handle, work_handle],
        write_lock: Mutex::new(()),
    };
    let dto = snapshot(&session);
    *lock_slot() = Some(session);
    Ok(dto)
}

fn resolve(host: &str, port: u16) -> Result<SocketAddr, String> {
    format!("{host}:{port}")
        .to_socket_addrs()
        .map_err(|error| coded(E_RESOLVE, format!("地址解析失败: {error}")))?
        .next()
        .ok_or_else(|| coded(E_RESOLVE, "地址解析结果为空"))
}

fn connect_stream(target: &SocketAddr) -> Result<TcpStream, String> {
    match TcpStream::connect_timeout(target, CONNECT_TIMEOUT) {
        Ok(stream) => Ok(stream),
        Err(error) => {
            let code = match error.kind() {
                ErrorKind::ConnectionRefused => E_CONNECT_REFUSED,
                ErrorKind::TimedOut => E_CONNECT_TIMEOUT,
                _ => E_IO,
            };
            Err(coded(code, format!("连接 {target} 失败: {error}")))
        }
    }
}

/// 阻塞等第一个 CONFIG；期间允许跳过其它包（对端可能先发心跳/日志类包）。
fn wait_for_config(reader: &mut TcpStream) -> Result<(u32, u32, u32), String> {
    let deadline = Instant::now() + CONFIG_TIMEOUT;
    while Instant::now() < deadline {
        match read_packet(reader) {
            Ok((KIND_CONFIG, payload)) => return parse_config(&payload),
            Ok((KIND_BYE, _)) => return Err(coded(E_BYE, "被控端在 CONFIG 前断开")),
            Ok(_) => continue,
            // 读超时继续等；其它错误立即报。
            Err(error) if error == RETRY => continue,
            Err(error) => return Err(error),
        }
    }
    Err(coded(
        E_CONFIG_TIMEOUT,
        "已连接但 8 秒内没收到 CONFIG（被控端可能正忙或采集未启动）",
    ))
}

/// 断开并回收线程；重复调用安全。
pub fn disconnect() {
    let mut guard = lock_slot();
    let Some(session) = guard.take() else {
        return;
    };
    session.stop.store(true, Ordering::Relaxed);
    // 主动发 BYE（kind=4，空载荷），对端据此清理会话。
    {
        let _write = session.write_lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut writer: &TcpStream = session.stream.as_ref();
        let _ = writer.write_all(&[KIND_BYE, 0, 0, 0, 0]);
    }
    session.queue.close();
    let _ = session.stream.shutdown(std::net::Shutdown::Both);
    for handle in session.threads {
        let _ = handle.join();
    }
}

/// 读取当前会话快照；未连接时返回默认值。
pub fn status() -> DirectSessionDto {
    let guard = lock_slot();
    match guard.as_ref() {
        Some(session) => snapshot(session),
        None => DirectSessionDto::default(),
    }
}

fn snapshot(session: &DirectSession) -> DirectSessionDto {
    let config = session.config.lock().unwrap_or_else(|p| p.into_inner());
    DirectSessionDto {
        connected: true,
        host: session.host_text.clone(),
        port: session.port,
        configured: config.ingested,
        width: config.width,
        height: config.height,
        fps: config.fps,
        bytes_received: session.counters.bytes_received.load(Ordering::Relaxed),
        frames_received: session.counters.frames_received.load(Ordering::Relaxed),
        frames_dropped: session.queue.dropped.load(Ordering::Relaxed),
        ingest_errors: session.counters.ingest_errors.load(Ordering::Relaxed),
        input_sent: session.counters.input_sent.load(Ordering::Relaxed),
        input_errors: session.counters.input_errors.load(Ordering::Relaxed),
        last_error: session.counters.error(),
    }
}

// =====** 输入上行 =====**

/// 把一批输入事件按 `kind=INPUT` 写出。
///
/// 触摸已由 Dart 侧转成 Pointer 语义（move/button），这里只做编码与写 socket；
/// 坐标必须是远端物理像素（由 Dart 按 CONFIG 尺寸换算）。
///
/// # Errors
/// 未连接、事件非法或写入失败时返回 `CODE: 人话`。
pub fn send_input(events: Vec<InputEventDto>) -> Result<u64, String> {
    let guard = lock_slot();
    let session = guard.as_ref().ok_or_else(|| coded(E_IO, "直连会话未建立"))?;
    let mut sent = 0u64;
    for dto in &events {
        let event = crate::input_service::to_event(dto)?;
        let payload = rdc_input::encode_event(&event);
        if payload.len() > MAX_PAYLOAD {
            session
                .counters
                .input_errors
                .fetch_add(1, Ordering::Relaxed);
            return Err(coded(E_PROTOCOL, "输入事件超长"));
        }
        let mut packet = Vec::with_capacity(5 + payload.len());
        packet.push(KIND_INPUT);
        packet.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        packet.extend_from_slice(&payload);
        let _guard = session.write_lock.lock().unwrap_or_else(|p| p.into_inner());
        let mut writer: &TcpStream = session.stream.as_ref();
        match writer.write_all(&packet) {
            Ok(()) => {
                sent += 1;
                session.counters.input_sent.fetch_add(1, Ordering::Relaxed);
            }
            Err(error) => {
                session
                    .counters
                    .input_errors
                    .fetch_add(1, Ordering::Relaxed);
                session
                    .counters
                    .set_error(coded(E_IO, format!("写入输入包失败: {error}")));
                return Err(coded(E_IO, format!("写入输入包失败: {error}")));
            }
        }
    }
    Ok(sent)
}

// =====** 测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 内存预算优先：超过 16 MiB 时丢最旧，剩余总量不超预算。
    #[test]
    fn frame_queue_respects_memory_budget() {
        let queue = FrameQueue::new();
        let chunk = vec![0u8; 4 * 1024 * 1024];
        for _ in 0..8 {
            queue.push(FrameItem {
                config_id: 1,
                pts_us: 0,
                keyframe: false,
                data: chunk.clone(),
            });
        }
        assert!(queue.dropped.load(Ordering::Relaxed) >= 4);
        // 先关闭再排空：`pop` 在空队列上会阻塞等新帧（消费者语义）。
        queue.close();
        let mut total = 0usize;
        while let Some(item) = queue.pop() {
            total += item.data.len();
        }
        assert_eq!(total, QUEUE_MAX_BYTES);
    }

    /// 条数上限同样生效。
    #[test]
    fn frame_queue_respects_item_limit() {
        let queue = FrameQueue::new();
        for index in 0..(QUEUE_MAX_ITEMS + 10) {
            queue.push(FrameItem {
                config_id: 1,
                pts_us: index as u64,
                keyframe: false,
                data: vec![0u8; 16],
            });
        }
        assert_eq!(queue.dropped.load(Ordering::Relaxed), 10);
        queue.close();
        let mut count = 0;
        while let Some(_) = queue.pop() {
            count += 1;
        }
        assert_eq!(count, QUEUE_MAX_ITEMS);
    }

    /// 关闭队列后消费者立即退出。
    #[test]
    fn closing_queue_wakes_consumer() {
        let queue = Arc::new(FrameQueue::new());
        let cloned = Arc::clone(&queue);
        let consumer = thread::spawn(move || cloned.pop());
        thread::sleep(Duration::from_millis(20));
        queue.close();
        assert!(consumer.join().unwrap().is_none());
    }

    /// 协议包长度上限与 CONFIG 解析的错误码。
    #[test]
    fn protocol_helpers_report_codes() {
        assert!(parse_config(&[0u8; 8]).unwrap_err().starts_with(E_PROTOCOL));
        let config = [1280u32, 800, 60];
        let mut bytes = Vec::new();
        for value in config {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        assert_eq!(parse_config(&bytes).unwrap(), (1280, 800, 60));
    }

    /// 未连接时发送输入必须明确报错，不得静默丢弃。
    #[test]
    fn send_input_requires_session() {
        disconnect();
        assert!(send_input(Vec::new()).is_err());
    }

    /// 合成整帧时，分片数与重组器常量一致。
    #[test]
    fn frame_chunking_matches_protocol() {
        let data = vec![7u8; VIDEO_FRAGMENT_BYTES * 3 + 10];
        assert_eq!(data.len().div_ceil(VIDEO_FRAGMENT_BYTES), 4);
        assert_eq!(VIDEO_STREAM_MAIN, 0);
    }
}
