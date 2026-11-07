//! rdc-bridge / 屏幕墙预览服务
//!
//! 层级：L4 桥接层
//! 用途：在应用进程内跑通 S5-1 的缩略子码流管线（合成采集源 → 共享缩略图编码器 →
//!       帧头/分片 → 严格顺序重组 → H.264 解码 → NV12 转 RGBA），把「最新预览帧」
//!       以可跨 FFI 复制的字节 + 时间戳暴露给 Flutter 屏幕墙。
//! 调用方：rdc-bridge 的 `wall_start` / `wall_latest` / `wall_status` / `wall_stop`。
//!
//! 范围说明（必须如实写出）：
//! - 采集源是**进程内合成画面**（与 `demo-wall` 同源），真实屏幕采集与跨机 QUIC 会话
//!   尚未接入；页面标注「本机回环预览」并记入 `docs/决策待办.md`。
//! - 单路流（stream 1）已跑通；多路预览复用同一编码器，接口按「N 个会话」设计，
//!   但当前只返回 1 路真实帧，其余宫格显示空状态而不是假数据。
//! - 不创建额外编码实例：全进程 1 个缩略图编码器 + 1 个解码器，与 S5-1 的选项 B 一致。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rdc_core::s5::wall::{
    decode_message, encode_message, ThumbnailEncoder, WallFrame, WallMessage, WallReceiver,
    WALL_BITRATE_BPS, WALL_FPS, WALL_HEIGHT, WALL_WIDTH,
};

// =====** 常量 =====**

/// 合成采集源宽（比 480p 大，模拟主码流分辨率，再缩放给缩略编码器）。
const SOURCE_WIDTH: u32 = 1280;
/// 合成采集源高。
const SOURCE_HEIGHT: u32 = 720;
/// 预览输出宽（解码后按 2:1 抽样，降低 FFI 拷贝量）。
const PREVIEW_WIDTH: u32 = WALL_WIDTH / 2;
/// 预览输出高。
const PREVIEW_HEIGHT: u32 = WALL_HEIGHT / 2;
/// 预览流编号。
const STREAM_ID: u32 = 1;
/// 轮询/更新周期：略小于 100 ms，保证 10 fps 编码节拍。
const TICK: Duration = Duration::from_millis(90);

// =====** 快照 =====**

/// 单帧预览（RGBA，可直接交给 Flutter 上屏）。
#[derive(Debug, Clone)]
pub struct PreviewFrame {
    /// 流编号。
    pub stream_id: u32,
    /// 帧号（单调递增）。
    pub frame_id: u32,
    /// host 侧时间戳（微秒，本进程单调时钟）。
    pub pts_us: u64,
    /// 预览宽。
    pub width: u32,
    /// 预览高。
    pub height: u32,
    /// 端到端延迟（微秒）：解码完成时刻 − 帧时间戳。
    pub latency_us: u64,
    /// RGBA 像素（紧密排列）。
    pub bytes: Vec<u8>,
}

/// 服务状态快照（不含像素）。
#[derive(Debug, Clone)]
pub struct WallSnapshot {
    /// 是否运行中。
    pub running: bool,
    /// 缩略编码器后端名。
    pub backend: String,
    /// 实际帧率（1 秒滑动窗口）。
    pub fps: f64,
    /// 已编码帧数。
    pub encoded_frames: u64,
    /// 已解码帧数。
    pub decoded_frames: u64,
    /// 重组丢弃帧数（协议异常计数）。
    pub dropped_frames: u64,
    /// 最近一帧的预览宽。
    pub width: u32,
    /// 最近一帧的预览高。
    pub height: u32,
    /// 最近一帧的端到端延迟（微秒）。
    pub latency_us: u64,
    /// 最近一帧时间戳（微秒）。
    pub latest_pts_us: u64,
    /// 最近一次错误（用于页面横幅）。
    pub last_error: Option<String>,
}

impl Default for WallSnapshot {
    fn default() -> Self {
        Self {
            running: false,
            backend: String::new(),
            fps: 0.0,
            encoded_frames: 0,
            decoded_frames: 0,
            dropped_frames: 0,
            width: PREVIEW_WIDTH,
            height: PREVIEW_HEIGHT,
            latency_us: 0,
            latest_pts_us: 0,
            last_error: None,
        }
    }
}

// =====** 全局状态 =====**

/// 服务控制句柄与共享快照。
struct ServiceState {
    /// 停止信号。
    stop: Arc<AtomicBool>,
    /// 工作线程。
    join: Option<JoinHandle<()>>,
    /// 状态快照。
    snapshot: Arc<Mutex<WallSnapshot>>,
    /// 最近一帧像素。
    latest: Arc<Mutex<Option<PreviewFrame>>>,
}

/// 进程内唯一服务（桥接层生命周期与进程一致）。
static SERVICE: OnceLock<Mutex<Option<ServiceState>>> = OnceLock::new();

/// 取全局锁；锁只保护 Option 的替换，持有时长上限 10 µs。
fn service_slot() -> &'static Mutex<Option<ServiceState>> {
    SERVICE.get_or_init(|| Mutex::new(None))
}

// =====** 对外操作 =====**

/// 启动屏幕墙预览。
///
/// # Returns
/// 启动后的状态快照；已运行时不重复启动。
///
/// # Errors
/// 编码器或解码器创建失败时返回错误文本，且不留下运行中的线程。
pub fn start() -> Result<WallSnapshot, String> {
    {
        let slot = service_slot().lock().map_err(|error| error.to_string())?;
        if let Some(state) = slot.as_ref() {
            let snapshot = state
                .snapshot
                .lock()
                .map_err(|error| error.to_string())?
                .clone();
            if snapshot.running {
                return Ok(snapshot);
            }
        }
    }

    let mut slot = service_slot().lock().map_err(|error| error.to_string())?;
    // 上一轮线程已结束但快照还是旧的：先回收，避免线程泄漏。
    if let Some(previous) = slot.take() {
        if let Some(join) = previous.join {
            let _ = join.join();
        }
    }

    let stop = Arc::new(AtomicBool::new(false));
    let snapshot = Arc::new(Mutex::new(WallSnapshot {
        running: true,
        ..WallSnapshot::default()
    }));
    let latest = Arc::new(Mutex::new(None));
    let snapshot_for_thread = Arc::clone(&snapshot);
    let snapshot_for_error = Arc::clone(&snapshot);
    let latest_for_thread = Arc::clone(&latest);
    let stop_for_thread = Arc::clone(&stop);
    let join = std::thread::Builder::new()
        .name("rdc-wall-preview".to_owned())
        .spawn(move || {
            if let Err(error) = run_pipeline(
                stop_for_thread,
                Arc::clone(&snapshot_for_thread),
                latest_for_thread,
            ) {
                if let Ok(mut guard) = snapshot_for_error.lock() {
                    guard.running = false;
                    guard.last_error = Some(error);
                }
            }
        })
        .map_err(|error| format!("创建屏幕墙线程失败：{error}"))?;

    *slot = Some(ServiceState {
        stop,
        join: Some(join),
        snapshot: Arc::clone(&snapshot),
        latest,
    });
    let guard = snapshot.lock().map_err(|error| error.to_string())?;
    Ok(guard.clone())
}

/// 停止预览并等待线程退出。
///
/// # Returns
/// 停止后的状态快照。
///
/// # Errors
/// 内部锁中毒时返回错误文本。
pub fn stop() -> Result<WallSnapshot, String> {
    let mut slot = service_slot().lock().map_err(|error| error.to_string())?;
    let state = slot.take();
    let mut snapshot = WallSnapshot::default();
    if let Some(state) = state {
        state.stop.store(true, Ordering::SeqCst);
        if let Some(join) = state.join {
            let _ = join.join();
        }
        if let Ok(guard) = state.snapshot.lock() {
            snapshot = guard.clone();
        }
        snapshot.running = false;
        if let Ok(mut guard) = state.snapshot.lock() {
            guard.running = false;
        }
    }
    Ok(snapshot)
}

/// 读取当前状态。
///
/// # Returns
/// 状态克隆；服务未启动时为默认快照（`running = false`）。
///
/// # Errors
/// 内部锁中毒时返回错误文本。
pub fn status() -> Result<WallSnapshot, String> {
    let slot = service_slot().lock().map_err(|error| error.to_string())?;
    match slot.as_ref() {
        Some(state) => state
            .snapshot
            .lock()
            .map(|guard| guard.clone())
            .map_err(|error| error.to_string()),
        None => Ok(WallSnapshot::default()),
    }
}

/// 读取最新预览帧。
///
/// # Returns
/// 无新帧时返回 `None`；否则返回 RGBA 像素与时间戳的克隆。
///
/// # Errors
/// 内部锁中毒时返回错误文本。
pub fn latest() -> Result<Option<PreviewFrame>, String> {
    let slot = service_slot().lock().map_err(|error| error.to_string())?;
    let Some(state) = slot.as_ref() else {
        return Ok(None);
    };
    let frame = state
        .latest
        .lock()
        .map_err(|error| error.to_string())?
        .clone();
    Ok(frame)
}

// =====** 管线 =====**

/// 工作线程主体：编码 → 分片 → 重组 → 解码 → RGBA。
fn run_pipeline(
    stop: Arc<AtomicBool>,
    snapshot: Arc<Mutex<WallSnapshot>>,
    latest: Arc<Mutex<Option<PreviewFrame>>>,
) -> Result<(), String> {
    let origin = Instant::now();
    let mut thumbnail =
        ThumbnailEncoder::new(WALL_WIDTH, WALL_HEIGHT, WALL_FPS, WALL_BITRATE_BPS)
            .map_err(|error| format!("缩略图编码器创建失败：{error}"))?;
    let backend = thumbnail.backend_name().to_owned();
    let mut decoder = open_decoder()?;
    let mut receiver = WallReceiver::new(STREAM_ID);
    let mut source = vec![0u8; SOURCE_WIDTH as usize * SOURCE_HEIGHT as usize * 4];
    let rows = pattern_rows(SOURCE_WIDTH);
    if let Ok(mut guard) = snapshot.lock() {
        guard.backend = backend;
    }

    let mut frame_id = 0u32;
    let mut tick = 0u32;
    let mut encoded = 0u64;
    let mut decoded_count = 0u64;
    let mut dropped = 0u64;
    let mut window_start = Instant::now();
    let mut window_frames = 0u32;
    let mut last_decoded: Option<PreviewFrame> = None;

    while !stop.load(Ordering::Relaxed) {
        let tick_start = Instant::now();
        paint_frame(&mut source, SOURCE_WIDTH, SOURCE_HEIGHT, tick, &rows);
        tick = tick.wrapping_add(1);
        let pts_us = origin.elapsed().as_micros() as u64;
        let packets = thumbnail
            .encode(&source, SOURCE_WIDTH, SOURCE_HEIGHT, pts_us * 1000)
            .map_err(|error| format!("缩略编码失败：{error}"))?;
        for packet in packets {
            frame_id = frame_id.wrapping_add(1);
            encoded += 1;
            // 走真实的帧头 + 1352 字节分片消息格式，再由接收端严格重组。
            let wall_frame = WallFrame {
                stream_id: STREAM_ID,
                frame_id,
                pts_us,
                width: WALL_WIDTH as u16,
                height: WALL_HEIGHT as u16,
                data: packet.data,
            };
            for message in frame_messages(&wall_frame) {
                let bytes = encode_message(&message)
                    .map_err(|error| format!("屏幕墙消息编码失败：{error}"))?;
                let decoded = decode_message(&bytes)
                    .map_err(|error| format!("屏幕墙消息解码失败：{error}"))?;
                match receiver.apply(decoded) {
                    Ok(Some(frame)) => {
                        if let Some(preview) = decode_frame(&mut decoder, &frame, origin)? {
                            decoded_count += 1;
                            if let Ok(mut guard) = latest.lock() {
                                *guard = Some(preview.clone());
                            }
                            last_decoded = Some(preview);
                        }
                    }
                    Ok(None) => {}
                    Err(_) => dropped += 1,
                }
            }
        }

        if last_decoded.is_some() {
            window_frames += 1;
        }
        if window_start.elapsed() >= Duration::from_secs(1) {
            let fps = f64::from(window_frames) / window_start.elapsed().as_secs_f64();
            window_start = Instant::now();
            window_frames = 0;
            if let Ok(mut guard) = snapshot.lock() {
                guard.fps = fps;
                guard.encoded_frames = encoded;
                guard.decoded_frames = decoded_count;
                guard.dropped_frames = dropped;
                guard.running = true;
                if let Some(preview) = last_decoded.as_ref() {
                    guard.width = preview.width;
                    guard.height = preview.height;
                    guard.latency_us = preview.latency_us;
                    guard.latest_pts_us = preview.pts_us;
                }
                // 有帧到达即清错误，避免旧错误一直挂在页面横幅上。
                if encoded > 0 {
                    guard.last_error = None;
                }
            }
        }

        let elapsed = tick_start.elapsed();
        if elapsed < TICK {
            std::thread::sleep(TICK - elapsed);
        }
    }

    // 退出前冲刷编码器，避免残留帧；解码器随线程释放。
    let _ = thumbnail.flush();
    let _ = decoder.flush();
    Ok(())
}

/// 把一帧拆成「帧头 + 若干分片」消息。
fn frame_messages(frame: &WallFrame) -> Vec<WallMessage> {
    use rdc_core::s5::wall::WALL_FRAGMENT_BYTES;

    let chunks: Vec<&[u8]> = frame.data.chunks(WALL_FRAGMENT_BYTES).collect();
    let mut messages = Vec::with_capacity(chunks.len() + 1);
    messages.push(WallMessage::Header {
        stream_id: frame.stream_id,
        frame_id: frame.frame_id,
        pts_us: frame.pts_us,
        width: frame.width,
        height: frame.height,
        total_len: frame.data.len() as u32,
    });
    for (index, chunk) in chunks.iter().enumerate() {
        messages.push(WallMessage::Fragment {
            stream_id: frame.stream_id,
            frame_id: frame.frame_id,
            frag_index: index as u16,
            frag_count: chunks.len() as u16,
            data: chunk.to_vec(),
        });
    }
    messages
}

/// 解码一帧并转为 RGBA 预览。
fn decode_frame(
    decoder: &mut DecoderHandle,
    frame: &WallFrame,
    origin: Instant,
) -> Result<Option<PreviewFrame>, String> {
    let mut outputs = Vec::new();
    decoder
        .feed_each(&frame.data, frame.pts_us, &mut |decoded| {
            outputs.push(decoded);
            Ok(())
        })
        .map_err(|error| format!("缩略帧解码失败：{error}"))?;
    let Some(output) = outputs.pop() else {
        return Ok(None);
    };
    let (width, height, bytes) = nv12_to_rgba_half(&output);
    let latency_us = origin.elapsed().as_micros() as u64;
    Ok(Some(PreviewFrame {
        stream_id: frame.stream_id,
        frame_id: frame.frame_id,
        pts_us: frame.pts_us,
        width,
        height,
        // 延迟用同一单调时钟：解码完成时刻 − 帧时间戳（同进程，无需时钟同步）。
        latency_us: latency_us.saturating_sub(frame.pts_us),
        bytes,
    }))
}

/// 编译期选择的解码句柄：Windows 用 Media Foundation，其余平台不可用。
#[cfg(target_os = "windows")]
type DecoderHandle = rdc_decode::MfH264Decoder;

/// 打开解码器（软件路径，不持有 D3D11 设备）。
#[cfg(target_os = "windows")]
fn open_decoder() -> Result<DecoderHandle, String> {
    rdc_decode::MfH264Decoder::open(WALL_WIDTH, WALL_HEIGHT, WALL_FPS, false)
        .map_err(|error| format!("H.264 解码器创建失败：{error}"))
}

/// 非 Windows 平台：屏幕墙预览返回明确错误，而不是假帧。
#[cfg(not(target_os = "windows"))]
type DecoderHandle = ();

/// 非 Windows 平台不支持解码。
#[cfg(not(target_os = "windows"))]
fn open_decoder() -> Result<DecoderHandle, String> {
    Err("屏幕墙预览解码目前仅支持 Windows".to_owned())
}

/// NV12 → RGBA，按 2:1 抽样（输出约 424x240）。
#[cfg(target_os = "windows")]
fn nv12_to_rgba_half(frame: &rdc_decode::DecodedFrame) -> (u32, u32, Vec<u8>) {
    let out_w = PREVIEW_WIDTH;
    let out_h = PREVIEW_HEIGHT;
    let stride = frame.stride as usize;
    let src_w = frame.width as usize;
    let src_h = frame.height as usize;
    let uv_offset = stride * src_h;
    let mut rgba = vec![0u8; out_w as usize * out_h as usize * 4];
    if frame.data.len() < uv_offset + stride * (src_h / 2) {
        return (out_w, out_h, rgba);
    }
    for oy in 0..out_h as usize {
        let sy = (oy * 2).min(src_h.saturating_sub(1));
        for ox in 0..out_w as usize {
            let sx = (ox * 2).min(src_w.saturating_sub(1));
            let y = frame.data[sy * stride + sx] as f32;
            let uv_index = uv_offset + (sy / 2) * stride + (sx / 2) * 2;
            let u = frame.data[uv_index] as f32 - 128.0;
            let v = frame.data[uv_index + 1] as f32 - 128.0;
            let r = (y + 1.5748 * v).clamp(0.0, 255.0);
            let g = (y - 0.1873 * u - 0.4681 * v).clamp(0.0, 255.0);
            let b = (y + 1.7902 * u).clamp(0.0, 255.0);
            let index = (oy * out_w as usize + ox) * 4;
            rgba[index] = r as u8;
            rgba[index + 1] = g as u8;
            rgba[index + 2] = b as u8;
            rgba[index + 3] = 255;
        }
    }
    (out_w, out_h, rgba)
}

/// 非 Windows 平台的占位转换（不会被执行，调用前已返回错误）。
#[cfg(not(target_os = "windows"))]
fn nv12_to_rgba_half(_frame: &()) -> (u32, u32, Vec<u8>) {
    (PREVIEW_WIDTH, PREVIEW_HEIGHT, Vec::new())
}

// =====** 合成画面 =====**

/// 生成两条预制行（深浅棋盘格）。
fn pattern_rows(width: u32) -> [Vec<u8>; 2] {
    let dark = [28u8, 36, 52, 255];
    let light = [60u8, 120, 200, 255];
    let mut rows = [vec![0u8; width as usize * 4], vec![0u8; width as usize * 4]];
    for (index, color) in [dark, light].into_iter().enumerate() {
        for px in rows[index].chunks_exact_mut(4) {
            px.copy_from_slice(&color);
        }
    }
    rows
}

/// 画一帧动态图案：整行拷贝 + 一条移动亮带。
fn paint_frame(buf: &mut [u8], width: u32, height: u32, tick: u32, rows: &[Vec<u8>; 2]) {
    let stride = width as usize * 4;
    for y in 0..height as usize {
        let band = (((y as u32 / 64) + tick) % 2) as usize;
        buf[y * stride..(y + 1) * stride].copy_from_slice(&rows[band]);
    }
    let bar = 96usize;
    let span = (width as usize - bar).max(1);
    let x0 = (tick as usize * 24) % span;
    for y in 0..height as usize {
        let offset = y * stride + x0 * 4;
        buf[offset..offset + bar * 4].fill(0xFF);
    }
}
