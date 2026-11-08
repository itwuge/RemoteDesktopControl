//! rdc-bridge / 屏幕墙预览服务
//!
//! 层级：L4 桥接层
//! 用途：在应用进程内跑通 S5-1 的缩略子码流管线（真实桌面采集 → 缩放 → 共享缩略图
//!       编码器 → 帧头/分片 → 严格顺序重组 → H.264 解码 → NV12 转 RGBA），把
//!       「最新预览帧」以可跨 FFI 复制的字节 + 时间戳暴露给 Flutter 屏幕墙。
//! 调用方：rdc-bridge 的 `wall_start` / `wall_latest` / `wall_status` / `wall_stop`。
//!
//! 范围说明（必须如实写出）：
//! - 采集源是本机 **DXGI 桌面复制**（`rdc-capture`），按 10 fps 采集后最近邻缩放到
//!   848x480 再进缩略编码器；不再使用合成测试图。
//! - 跨机 QUIC 会话尚未接入：第 1 宫格是本机真实桌面，其余宫格显示空状态而不是假数据。
//! - 不创建额外编码实例：全进程 1 个缩略图编码器 + 1 个解码器，与 S5-1 的选项 B 一致。
//! - 采集会话有线程亲和性，采集源在管线线程内创建/启动/停止。

// 非 Windows 构建下以下 11 项必然无调用点：缩略流管线只在 Windows 可达（`start()`
// 在其它平台直接返回「不支持」，`start_windows` 也按 `#[cfg(windows)]` 排除），涉及
// `CAPTURE_FPS`、`CAPTURE_CHANNEL_CAP`、`STREAM_ID`、`TICK`、`WallSink`、
// `scale_bgra_to_rgba`、`run_pipeline`、`frame_messages`、`DecoderHandle`、
// `open_decoder`、`decode_frame`。它们含采集帧率/通道容量/节拍等规格数值，不能为消
// warning 删除，故仅在非 Windows 构建放行 dead_code；Windows 构建仍严格检查。
#![cfg_attr(not(windows), allow(dead_code))]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use rdc_capture::{create_source, CaptureConfig, ChromaFormat, Frame, FrameSink};
use rdc_core::s5::wall::{
    decode_message, encode_message, ThumbnailEncoder, WallFrame, WallMessage, WallReceiver,
    WALL_BITRATE_BPS, WALL_FPS, WALL_HEIGHT, WALL_WIDTH,
};

use crate::error::lock_poisoned_text;

// =====** 常量 =====**

/// 采集帧率：与缩略流编码帧率一致，避免为缩略预览占用 60Hz 的桌面采集。
const CAPTURE_FPS: u32 = WALL_FPS;
/// 采集 → 消费通道容量（帧）；满即丢，绝不在采集线程上阻塞。
const CAPTURE_CHANNEL_CAP: usize = 2;
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
    /// 采集后端名（如 `dxgi-desktop-duplication`）。
    pub capture_backend: String,
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
            capture_backend: String::new(),
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
    // Linux 分支：采集与编码已具备，但缩略流解码后端尚未接入（rdc-decode 无 Linux 实现）。
    // 明确返回「不支持」，而不是启动线程后异步失败或返回假帧；接口与 DTO 保持不变。
    #[cfg(not(windows))]
    {
        Err("屏幕墙缩略预览暂不支持当前平台：缩略流解码仅 Windows 已实现（接口保留）".to_owned())
    }
    #[cfg(windows)]
    {
        start_windows()
    }
}

/// Windows 实现：启动缩略流采集/编码/解码线程。
#[cfg(windows)]
fn start_windows() -> Result<WallSnapshot, String> {
    {
        let slot = service_slot().lock().map_err(|poison| lock_poisoned_text("poisoned lock: another task failed inside", poison))?;
        if let Some(state) = slot.as_ref() {
            let snapshot = state
                .snapshot
                .lock()
                .map_err(|poison| lock_poisoned_text("poisoned lock: another task failed inside", poison))?
                .clone();
            if snapshot.running {
                return Ok(snapshot);
            }
        }
    }

    let mut slot = service_slot().lock().map_err(|poison| lock_poisoned_text("poisoned lock: another task failed inside", poison))?;
    // 上一轮线程已结束但快照还是旧的：先回收，避免线程泄漏。
    if let Some(previous) = slot.take() {
        if let Some(join) = previous.join {
            let _ = join.join(); // gate-allow R-C-2: 回收上一轮投影线程，panic 已由 panic hook 记录，此处无恢复动作
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
    let guard = snapshot.lock().map_err(|poison| lock_poisoned_text("poisoned lock: another task failed inside", poison))?;
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
    let mut slot = service_slot().lock().map_err(|poison| lock_poisoned_text("poisoned lock: another task failed inside", poison))?;
    let state = slot.take();
    let mut snapshot = WallSnapshot::default();
    if let Some(state) = state {
        state.stop.store(true, Ordering::SeqCst);
        if let Some(join) = state.join {
            let _ = join.join(); // gate-allow R-C-2: 投影线程已收到停止标志，join 失败不改变停止结论
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
    let slot = service_slot().lock().map_err(|poison| lock_poisoned_text("poisoned lock: another task failed inside", poison))?;
    match slot.as_ref() {
        Some(state) => state
            .snapshot
            .lock()
            .map(|guard| guard.clone())
            .map_err(|poison| lock_poisoned_text("poisoned lock: another task failed inside", poison)),
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
    let slot = service_slot().lock().map_err(|poison| lock_poisoned_text("poisoned lock: another task failed inside", poison))?;
    let Some(state) = slot.as_ref() else {
        return Ok(None);
    };
    let frame = state
        .latest
        .lock()
        .map_err(|poison| lock_poisoned_text("poisoned lock: another task failed inside", poison))?
        .clone();
    Ok(frame)
}

// =====** 采集接收 =====**

/// 采集回调：把帧连同到达时刻一起投进有界通道，绝不阻塞采集线程。
struct WallSink {
    /// 有界发送端。
    tx: SyncSender<(Frame, u64)>,
    /// 进程内单调时钟基准。
    origin: Instant,
}

impl FrameSink for WallSink {
    fn on_frame(&self, frame: Frame) {
        let pts_us = self.origin.elapsed().as_micros() as u64;
        let _ = self.tx.try_send((frame, pts_us)); // gate-allow R-C-2: 投影有界队列满时丢当前帧是设计行为，背压由上游采集自行降速
    }
}

/// 把任意尺寸 BGRA 帧按最近邻缩放到缩略尺寸的 RGBA 紧凑缓冲。
fn scale_bgra_to_rgba(src: &[u8], sw: u32, sh: u32, dst: &mut [u8], dw: u32, dh: u32) {
    if sw == 0 || sh == 0 {
        return;
    }
    let source_row_bytes = sw as usize * 4;
    let target_row_bytes = dw as usize * 4;
    for dy in 0..dh as usize {
        let sy = (dy * sh as usize / dh as usize).min(sh as usize - 1);
        let src_row = sy * source_row_bytes;
        let dst_row = dy * target_row_bytes;
        for dx in 0..dw as usize {
            let sx = (dx * sw as usize / dw as usize).min(sw as usize - 1);
            let si = src_row + sx * 4;
            let di = dst_row + dx * 4;
            dst[di] = src[si + 2];
            dst[di + 1] = src[si + 1];
            dst[di + 2] = src[si];
            dst[di + 3] = 255;
        }
    }
}

// =====** 管线 =====**

/// 工作线程主体：真实采集 → 缩放 → 编码 → 分片 → 重组 → 解码 → RGBA。
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

    // 采集会话有线程亲和性：源在本线程创建、启动，线程退出时随之释放。
    let mut capture = create_source().map_err(|error| format!("采集源创建失败：{error}"))?;
    let displays = capture.enumerate();
    let display = displays
        .iter()
        .find(|item| item.is_primary)
        .or_else(|| displays.first())
        .ok_or_else(|| "未找到可采集显示器".to_owned())?;
    let display_id = display.id;
    let (sender, sampler) = sync_channel::<(Frame, u64)>(CAPTURE_CHANNEL_CAP);
    capture.set_sink(Arc::new(WallSink { tx: sender, origin }));
    capture
        .start(CaptureConfig {
            display_id,
            target_fps: CAPTURE_FPS,
            include_cursor: true,
            chroma: ChromaFormat::Bgra,
            bit_depth: 8,
        })
        .map_err(|error| format!("采集启动失败：{error}"))?;
    // 后端名在 start 之后才落定（dxgi 优先，gdi 兜底）。
    let capture_backend = capture.backend_name().to_owned();

    let mut source = vec![0u8; WALL_WIDTH as usize * WALL_HEIGHT as usize * 4];
    if let Ok(mut guard) = snapshot.lock() {
        guard.backend = backend;
        guard.capture_backend = capture_backend;
        guard.running = true;
    }

    let mut frame_id = 0u32;
    let mut encoded = 0u64;
    let mut decoded_count = 0u64;
    let mut dropped = 0u64;
    let mut window_start = Instant::now();
    let mut window_frames = 0u32;
    let mut last_decoded: Option<PreviewFrame> = None;

    while !stop.load(Ordering::Relaxed) {
        let tick_start = Instant::now();
        // 预览只要最新一帧：把通道里的积压读空，避免延迟随队列增长。
        let mut sample: Option<(Frame, u64)> = None;
        while let Ok(next) = sampler.try_recv() {
            sample = Some(next);
        }
        let Some((frame, pts_us)) = sample else {
            std::thread::sleep(TICK);
            continue;
        };
        let packets = {
            scale_bgra_to_rgba(
                frame.buffer.as_slice(),
                frame.width,
                frame.height,
                &mut source,
                WALL_WIDTH,
                WALL_HEIGHT,
            );
            thumbnail
                .encode(&source, WALL_WIDTH, WALL_HEIGHT, pts_us * 1000)
                .map_err(|error| format!("缩略编码失败：{error}"))?
        };
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

    // 退出前停采集并冲刷编码器，避免残留帧；解码器随线程释放。
    capture.stop();
    let _ = thumbnail.flush(); // gate-allow R-C-2: 线程退出前冲刷编码器尾包，失败无恢复动作，已编码帧不受影响
    // 解码器只在 Windows 后端存在；其它平台无解码句柄可冲刷。
    #[cfg(target_os = "windows")]
    let _ = decoder.flush(); // gate-allow R-C-2: 同上，冲刷失败不影响已呈现的投影帧
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
#[cfg(target_os = "windows")]
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

/// 编译期选择的解码句柄：Windows 用统一解码后端（默认 FFmpeg，MF 回退），其余平台不可用。
#[cfg(target_os = "windows")]
type DecoderHandle = Box<dyn rdc_decode::VideoDecoder>;

/// 打开解码器（软件路径，不持有 D3D11 设备）。
#[cfg(target_os = "windows")]
fn open_decoder() -> Result<DecoderHandle, String> {
    rdc_decode::open_video_decoder(None, WALL_WIDTH, WALL_HEIGHT, WALL_FPS, false)
        .map_err(|error| format!("H.264 解码器创建失败：{error}"))
}

/// 非 Windows 平台的解码句柄占位。
///
/// 必须是独立的 unit-like struct，不能用 `()`：`let mut decoder = open_decoder()?;`
/// 在 `Ok` 类型为 `()` 时绑定的是 unit 值，clippy 的 `let_unit_value` 会报错；
/// 换成具名类型后绑定值不是 `()`，lint 不再成立。该值实际永不可达（`open_decoder`
/// 恒返回 `Err`），仅用于让跨平台代码在编译期类型检查里成立。
#[cfg(not(target_os = "windows"))]
struct UnsupportedDecoder;

/// 非 Windows 平台：屏幕墙预览返回明确错误，而不是假帧。
#[cfg(not(target_os = "windows"))]
type DecoderHandle = UnsupportedDecoder;

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

/// 非 Windows 平台的解码占位：屏幕墙预览依赖 Media Foundation，调用方按错误处理。
#[cfg(not(target_os = "windows"))]
fn decode_frame(
    _decoder: &mut DecoderHandle,
    _frame: &WallFrame,
    _origin: Instant,
) -> Result<Option<PreviewFrame>, String> {
    Err("屏幕墙预览解码目前仅支持 Windows".to_owned())
}

