// ============================================================
// 文件用途：控制页本机回环预览管线（采集 → H.264 编码 → 解码 → D3D11 共享纹理）
//
// 层级：L4 桥接层的子模块（只在本进程内做真实采集与上屏，不建网络会话）
// 被谁调用：crates/rdc-bridge/src/api.rs 的 control_* 函数
// 边界：不暴露裸指针给 Dart；只把共享纹理句柄与信号事件以 i64 交出，
//       由 runner 的 control_texture.cpp 注册为 Flutter 外部纹理。
// ============================================================

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rdc_capture::{create_source, CaptureConfig, ChromaFormat, Frame, FrameSink};
use rdc_codec::{create_encoder_selected, CodecKind, EncodedPacket, VideoChroma, VideoConfig};
use rdc_decode::{open_video_decoder, DecodedFrame, VideoDecoder};
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
use windows::Win32::Foundation::{CloseHandle, HANDLE, HMODULE};
use windows::Win32::Graphics::Direct3D10::ID3D10Multithread;
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE, D3D11_BOX,
    D3D11_CPU_ACCESS_READ, D3D11_CPU_ACCESS_WRITE, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_RESOURCE_MISC_SHARED, D3D11_SDK_VERSION,
    D3D11_TEX2D_VPIV, D3D11_TEX2D_VPOV, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    D3D11_USAGE_STAGING, D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE, D3D11_VIDEO_PROCESSOR_CONTENT_DESC,
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC, D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0,
    D3D11_VIDEO_PROCESSOR_STREAM, D3D11_VIDEO_USAGE_PLAYBACK_NORMAL, D3D11_VPIV_DIMENSION_TEXTURE2D,
    D3D11_VPOV_DIMENSION_TEXTURE2D, ID3D11Device, ID3D11DeviceContext, ID3D11Resource,
    ID3D11Texture2D,
    ID3D11VideoContext, ID3D11VideoDevice, ID3D11VideoProcessor,
    ID3D11VideoProcessorEnumerator, ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::IDXGIResource;
use windows::Win32::System::Threading::{CreateEventW, SetEvent};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

use crate::error::lock_poisoned_text;

// =====** 常量 =====**

/// 编码目标帧率（Hz）：与采集同频，限流会把 MFT 固定积压的帧数（约 13 帧）
/// 按 30fps 拉长到 ~430ms，实测画内时延从 ~260ms 升到 ~550ms，故不做编码限流。
const TARGET_FPS: u32 = 60;
/// 主码流码率（bps）：1080p60 桌面文字需要较高码率才能看清。
const TARGET_BITRATE: u32 = 12_000_000;
/// 采集通道容量：只有 2 帧，满了立即丢最旧帧，保证低延迟。
const CHANNEL_CAP: usize = 2;
/// 码流包队列容量（编码线程 → 呈现线程）：同样只留 2 个，满了由编码线程反压等待。
const PACKET_CAP: usize = 2;
/// 时间码回读周期（毫秒）。
const READBACK_INTERVAL_MS: u64 = 500;
/// 时间码编码位数（值域 0..1_000_000 毫秒，取 20 位）。
const BARCODE_BITS: u32 = 20;
/// 时间码单元边长（像素）。
const BARCODE_CELL: usize = 18;
/// 时间码单元步距（像素）。
const BARCODE_PITCH: usize = 24;
/// 时间码起点 X（像素）。
const BARCODE_X0: usize = 16;
/// 时间码起点 Y（像素）。
const BARCODE_Y0: usize = 16;
/// 编码在飞帧上限（**有界**，绝对不允许无界）。
///
/// 为什么是 3：MF H.264 硬件 MFT 是异步的，自身有 1~2 帧固有流水延迟——
/// 提交第 N 帧后，它的输出常在第 N+1~N+2 次驱动时才可取回。上限取 3 正好覆盖
/// 这段延迟；再多只增加排队时延，不增加吞吐（S7-9 实测）。
/// 反面教材：S7-8 的「无界提前提交」把 MFT 输入队列灌满，导致 `push` 阻塞、
/// 全程 0 包（`s78-final`）。
const MAX_IN_FLIGHT: usize = 3;
/// 编码看门狗阈值（毫秒）：连续这么久无新包 → 重建编码会话一次；
/// 重建后再一个阈值仍无包 → 报 `RDC-ENC-001`。
///
/// 取 600 ms：覆盖 MFT 1~2 帧固有延迟（60 fps 下约 33 ms）的十倍以上，
/// 不会把正常的首帧/重配抖动误判成失速，又能在用户察觉黑屏前给出结论。
const ENCODE_WATCHDOG_MS: u64 = 600;

// =====** 对外快照 =====**

/// 控制页预览状态快照。
#[derive(Debug, Clone, Default)]
pub struct ControlSnapshot {
    /// 是否运行。
    pub running: bool,
    /// 采集后端名称。
    pub backend: String,
    /// 编码器名称（configure 之后的真实后端/驱动名）。
    pub encoder: String,
    /// 编码器选择理由（探测结果与跳过项；回退不静默）。
    pub encoder_fallback_reason: Option<String>,
    /// 解码器名称。
    pub decoder: String,
    /// 纹理通道：`dxgi-shared-texture` 为正解；`rgba-snapshot` 为降级。
    pub texture_path: String,
    /// 解码输出路径：`dxgi-texture`（GPU）或 `system-memory`（CPU 转换）。
    pub decode_path: String,
    /// 画面宽。
    pub width: u32,
    /// 画面高。
    pub height: u32,
    /// 实测帧率（1 秒窗口）。
    pub fps: f64,
    /// 采集帧数。
    pub captured_frames: u64,
    /// 编码包数。
    pub encoded_frames: u64,
    /// 解码帧数。
    pub decoded_frames: u64,
    /// 采集通道丢帧数。
    pub dropped_frames: u64,
    /// 编码输出累计字节数（真实编码包大小累加）。
    pub encoded_bytes: u64,
    /// 实测码率（千比特/秒，1 秒窗口）。
    pub bitrate_kbps: u64,
    /// 最近端到端延迟（微秒，画内时间码回读）。
    pub latency_last_us: u64,
    /// 端到端延迟 p50（微秒）。
    pub latency_p50_us: u64,
    /// 端到端延迟 p99（微秒）。
    pub latency_p99_us: u64,
    /// 时间码回读次数（每秒约 2 次，用于证明没有逐帧回读）。
    pub readback_count: u64,
    /// 共享纹理句柄 A（DXGI legacy shared handle）。
    pub share_handle_a: i64,
    /// 共享纹理句柄 B。
    pub share_handle_b: i64,
    /// 新帧信号事件 A（Windows HANDLE）。
    pub event_a: i64,
    /// 新帧信号事件 B。
    pub event_b: i64,
    /// 最近错误。
    pub last_error: Option<String>,
    /// 编码会话重建次数（看门狗）。
    pub encoder_resets: u64,
    /// 当前在飞帧数（已提交、未出包；有界 ≤ 3）。
    pub encoder_in_flight: u64,
}

// =====** 统计 =====**

/// 线程内共享的计数与最近错误。
#[derive(Default)]
struct Stats {
    captured: AtomicU64,
    encoded: AtomicU64,
    decoded: AtomicU64,
    displayed: AtomicU64,
    dropped: Arc<AtomicU64>,
    fps_milli: AtomicU64,
    encoded_bytes: AtomicU64,
    bitrate_kbps: AtomicU64,
    latency_last_us: AtomicU64,
    latency_p50_us: AtomicU64,
    latency_p99_us: AtomicU64,
    pipeline_latency_us: AtomicU64,
    /// 解码器内部积压帧数（已送输入 − 已取输出）：4C 用来看 MF 是否还会积压。
    decoder_in_flight: AtomicU64,
    readback_count: AtomicU64,
    /// 最近一帧采集耗时（微秒）：GPU 直出时是拷贝提交时间，CPU 路径含 staging 回读。
    capture_us: AtomicU64,
    /// 采集帧到手 → 编码入队耗时（微秒，含时间码覆盖/色彩交换）。
    encode_us: AtomicU64,
    /// 取编码输出耗时（微秒，本帧累计）。
    poll_us: AtomicU64,
    /// 解码喂入耗时（微秒，本帧累计）。
    decode_us: AtomicU64,
    /// 上屏（VideoProcessor Blit + SetEvent）耗时（微秒，本帧累计）。
    present_us: AtomicU64,
    /// 编码会话重建次数（看门狗）。
    encoder_resets: AtomicU64,
    /// 当前在飞帧数（已提交、未出包）。
    encoder_in_flight: AtomicU64,
    /// 在飞记录超过上限时丢最旧的次数（有界保护的计数，不是采集丢帧）。
    encoder_dropped: AtomicU64,
    error: Mutex<Option<String>>,
    backend: Mutex<String>,
    encoder: Mutex<String>,
    /// 编码器选择理由（探测链与跳过项）。
    encoder_reason: Mutex<String>,
    decoder: Mutex<String>,
    decode_path: Mutex<String>,
}

impl Stats {
    /// 记录一条错误（只保留第一条，避免刷屏）。
    fn set_error(&self, message: impl Into<String>) {
        if let Ok(mut slot) = self.error.lock() {
            if slot.is_none() {
                *slot = Some(message.into());
            }
        }
    }

    /// 覆盖写入错误：看门狗必须能覆盖早期旧错误，保证 RDC-ENC-001 在 HUD 可见。
    fn force_error(&self, message: impl Into<String>) {
        if let Ok(mut slot) = self.error.lock() {
            *slot = Some(message.into());
        }
    }

    /// 清空错误（编码恢复后，HUD 不应继续显示旧错误）。
    fn clear_error(&self) {
        if let Ok(mut slot) = self.error.lock() {
            *slot = None;
        }
    }

    /// 写入字符串型字段。
    fn set_text(slot: &Mutex<String>, value: impl Into<String>) {
        if let Ok(mut guard) = slot.lock() {
            *guard = value.into();
        }
    }

    /// 读取字符串型字段。
    fn text(slot: &Mutex<String>) -> String {
        match slot.lock() {
            Ok(guard) => guard.clone(),
            Err(error) => {
                tracing::warn!(error = %error, "[control] 读取评审字段时锁已中毒，按空值返回");
                String::new()
            }
        }
    }
}

// =====** 采集帧直投队列 =====**

/// 采集线程 → 编码线程的直投队列：深度固定，满时**丢最旧帧**并计数。
///
/// 为什么不用 `mpsc::sync_channel`：`try_send` 满时只能丢*新*帧，等于把最新画面挡在门外；
/// 低延迟场景里该丢的是旧帧。代价是丢帧计数要自己维护（`dropped`）。
///
/// 相位锁（S7-8 根因）就断在这里：采集线程 `on_frame` 只做一次入队、立即返回采集循环；
/// 编码线程独立阻塞在这个队列上，帧一落地就被拿走去提交，不再受主循环节拍牵连。
struct FrameQueue {
    /// 队列本体（只存帧，深度 ≤ `depth`）。
    inner: Mutex<VecDeque<QueuedFrame>>,
    /// 有新帧 / 关闭时唤醒编码线程。
    ready: Condvar,
    /// 满时丢最旧帧的计数（即 `stats.dropped`）。
    dropped: Arc<AtomicU64>,
    /// 队列深度。
    depth: usize,
}

/// 队列里的帧：帧本体 + 入队瞬间（微秒）。
struct QueuedFrame {
    frame: Frame,
    /// 采集线程把帧放进队列的 QPC（微秒）。
    enqueue_qpc: u64,
}

/// 取帧结果。
enum QueuePop {
    /// 取到一帧（附入队时刻）。
    Frame(QueuedFrame),
    /// 超时（采集可能暂时无变化）。
    Timeout,
    /// 队列已关闭（采集侧已停止）。
    Closed,
}

impl FrameQueue {
    /// 创建直投队列。
    fn new(depth: usize, dropped: Arc<AtomicU64>) -> Self {
        Self {
            inner: Mutex::new(VecDeque::with_capacity(depth + 1)),
            ready: Condvar::new(),
            dropped,
            depth,
        }
    }

    /// 入队一帧：满了先丢队首（最旧），再放新帧，永不阻塞采集线程。
    fn push(&self, frame: Frame) {
        let Ok(mut guard) = self.inner.lock() else {
            return;
        };
        while guard.len() >= self.depth {
            guard.pop_front();
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        guard.push_back(QueuedFrame {
            enqueue_qpc: qpc_us(),
            frame,
        });
        self.ready.notify_one();
    }

    /// 阻塞取帧（带超时），供编码线程独占消费。
    fn pop_timeout(&self, timeout: Duration) -> QueuePop {
        let Ok(mut guard) = self.inner.lock() else {
            return QueuePop::Closed;
        };
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(frame) = guard.pop_front() {
                return QueuePop::Frame(frame);
            }
            let now = Instant::now();
            if now >= deadline {
                return QueuePop::Timeout;
            }
            let Ok((next, _)) = self.ready.wait_timeout(guard, deadline - now) else {
                return QueuePop::Closed;
            };
            guard = next;
        }
    }

    /// 关闭队列：唤醒等待者，让编码线程尽快看到停止标志。
    fn close(&self) {
        let Ok(mut guard) = self.inner.lock() else {
            return;
        };
        guard.clear();
        self.ready.notify_all();
    }
}

/// `FrameSink` 适配：采集线程每产出一帧就直投入队（不阻塞、不丢最新帧）。
struct FrameQueueSink {
    queue: Arc<FrameQueue>,
}

impl FrameSink for FrameQueueSink {
    fn on_frame(&self, frame: Frame) {
        self.queue.push(frame);
    }
}

// =====** 运行态 =====**

/// 一次运行实例的句柄与线程。
struct Running {
    stop: Arc<AtomicBool>,
    stats: Arc<Stats>,
    width: u32,
    height: u32,
    share_handles: [i64; 2],
    events: [i64; 2],
    join: Option<JoinHandle<()>>,
}

/// 全局单实例槽位。
fn state() -> &'static Mutex<Option<Running>> {
    static STATE: OnceLock<Mutex<Option<Running>>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(None))
}

// =====** 启动 / 停止 / 状态 =====**

/// 启动本机回环控制预览。
///
/// # Returns
/// 启动后的状态快照（含共享纹理句柄与事件句柄）。
///
/// # Errors
/// 采集源/编码器/解码器/D3D11 资源创建失败时返回错误文本。
pub fn start() -> Result<ControlSnapshot, String> {
    let mut slot = state().lock().map_err(|poison| lock_poisoned_text("控制状态锁损坏", poison))?;
    if let Some(running) = slot.as_ref() {
        if !running.stop.load(Ordering::Relaxed) {
            return Err("控制预览已在运行".to_owned());
        }
        // 上一实例已请求停止但尚未 join：先回收，避免句柄泄漏。
        if let Some(running) = slot.take() {
            let _ = running.join.map(|join| join.join()); // gate-allow R-C-2: 回收旧线程，panic 已由 panic hook 记录，此处无恢复动作
        }
    }

    let stats = Arc::new(Stats::default());
    let (setup_tx, setup_rx) = sync_channel::<Result<SetupInfo, String>>(1);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_thread = Arc::clone(&stop);
    let stats_for_thread = Arc::clone(&stats);

    // 采集源、D3D11 设备与解码 MFT 都必须在**同一个线程**里创建与使用
    // （DXGI Desktop Duplication 与 MFT 都有线程亲和性），因此整套资源在管线线程内初始化，
    // 初始化结果通过 setup 通道回传给 start()。
    let join = thread::Builder::new()
        .name("rdc-control-preview".to_owned())
        .spawn(move || match setup_pipeline(&stats_for_thread) {
            Ok((source, queue, parts)) => {
                let _ = setup_tx.send(Ok(parts.info.clone())); // gate-allow R-C-2: 启动结果回传通道接收端可能已超时退出，发送失败不影响管线线程继续
                run_pipeline(source, queue, parts, stats_for_thread, stop_for_thread);
            }
            Err(error) => {
                let _ = setup_tx.send(Err(error)); // gate-allow R-C-2: 同上，接收端已超时时由 start() 返回超时错误
            }
        })
        .map_err(|error| format!("启动管线线程失败: {error}"))?;

    let info = match setup_rx.recv_timeout(Duration::from_secs(8)) {
        Ok(Ok(info)) => info,
        Ok(Err(error)) => {
            let _ = join.join(); // gate-allow R-C-2: 回收已报错的管线线程，具体错误已由 setup 通道取回并返回
            return Err(error);
        }
        Err(_) => {
            stop.store(true, Ordering::SeqCst);
            return Err("控制预览启动超时（8 秒）".to_owned());
        }
    };

    let snapshot = ControlSnapshot {
        running: true,
        backend: info.backend.clone(),
        encoder: info.encoder.clone(),
        encoder_fallback_reason: Some(info.encoder_reason.clone()),
        decoder: info.decoder.clone(),
        texture_path: "dxgi-shared-texture".to_owned(),
        decode_path: info.decode_path.clone(),
        width: info.width,
        height: info.height,
        share_handle_a: info.share_handles[0],
        share_handle_b: info.share_handles[1],
        event_a: info.events[0],
        event_b: info.events[1],
        ..ControlSnapshot::default()
    };

    *slot = Some(Running {
        stop,
        stats,
        width: info.width,
        height: info.height,
        share_handles: info.share_handles,
        events: info.events,
        join: Some(join),
    });
    Ok(snapshot)
}

/// 停止预览并等待线程退出。
///
/// # Returns
/// 停止后的状态快照（running 为 false）。
///
/// # Errors
/// 状态锁异常时返回错误文本。
pub fn stop() -> Result<ControlSnapshot, String> {
    let mut slot = state().lock().map_err(|poison| lock_poisoned_text("控制状态锁损坏", poison))?;
    let Some(mut running) = slot.take() else {
        return Ok(ControlSnapshot::default());
    };
    running.stop.store(true, Ordering::SeqCst);
    // 采集线程最多 1 秒内退出；join 失败不视为业务错误。
    if let Some(join) = running.join.take() {
        let _ = join.join(); // gate-allow R-C-2: 文档约定 join 失败不视为业务错误，线程退出即达目的
    }
    let stats = Arc::clone(&running.stats);
    let snapshot = build_snapshot(&stats, false, running.width, running.height, &running);
    Ok(snapshot)
}

/// 读取当前状态。
///
/// # Returns
/// 运行中返回实时数据；未运行返回零值快照。
///
/// # Errors
/// 状态锁异常时返回错误文本。
pub fn status() -> Result<ControlSnapshot, String> {
    let slot = state().lock().map_err(|poison| lock_poisoned_text("控制状态锁损坏", poison))?;
    let Some(running) = slot.as_ref() else {
        return Ok(ControlSnapshot::default());
    };
    Ok(build_snapshot(
        &running.stats,
        true,
        running.width,
        running.height,
        running,
    ))
}

/// 组装快照。
fn build_snapshot(stats: &Stats, running: bool, width: u32, height: u32, running_state: &Running) -> ControlSnapshot {
    ControlSnapshot {
        running,
        backend: Stats::text(&stats.backend),
        encoder: Stats::text(&stats.encoder),
        encoder_fallback_reason: {
            let reason = Stats::text(&stats.encoder_reason);
            (!reason.is_empty()).then_some(reason)
        },
        decoder: Stats::text(&stats.decoder),
        texture_path: "dxgi-shared-texture".to_owned(),
        decode_path: Stats::text(&stats.decode_path),
        width,
        height,
        fps: stats.fps_milli.load(Ordering::Relaxed) as f64 / 1000.0,
        captured_frames: stats.captured.load(Ordering::Relaxed),
        encoded_frames: stats.encoded.load(Ordering::Relaxed),
        decoded_frames: stats.decoded.load(Ordering::Relaxed),
        dropped_frames: stats.dropped.load(Ordering::Relaxed),
        encoded_bytes: stats.encoded_bytes.load(Ordering::Relaxed),
        bitrate_kbps: stats.bitrate_kbps.load(Ordering::Relaxed),
        latency_last_us: stats.latency_last_us.load(Ordering::Relaxed),
        latency_p50_us: stats.latency_p50_us.load(Ordering::Relaxed),
        latency_p99_us: stats.latency_p99_us.load(Ordering::Relaxed),
        readback_count: stats.readback_count.load(Ordering::Relaxed),
        share_handle_a: running_state.share_handles[0],
        share_handle_b: running_state.share_handles[1],
        event_a: running_state.events[0],
        event_b: running_state.events[1],
        last_error: stats.error.lock().ok().and_then(|guard| guard.clone()),
        encoder_resets: stats.encoder_resets.load(Ordering::Relaxed),
        encoder_in_flight: stats.encoder_in_flight.load(Ordering::Relaxed),
    }
}

// =====** 初始化 =====**

/// 初始化结果（跨线程回传给 `start()`）。
#[derive(Clone)]
struct SetupInfo {
    width: u32,
    height: u32,
    share_handles: [i64; 2],
    events: [i64; 2],
    backend: String,
    encoder: String,
    encoder_reason: String,
    decoder: String,
    decode_path: String,
}

/// GPU 时间码覆盖器。
///
/// 为什么需要它：纹理直出路径没有 CPU 像素可写，但画内时间码必须在编码前
/// 进入画面。做法是把时间码画进一张 512×64 的小纹理（约 128KB 上传），
/// 再用 `CopySubresourceRegion` 覆盖采集帧左上角的条带；整帧永不回读 CPU。
struct BarcodeOverlay {
    /// 覆盖用纹理（`DEFAULT`，每帧 `UpdateSubresource` 更新）；持有引用保证资源有效。
    _texture: ID3D11Texture2D,
    /// `ID3D11Resource` 视图（`UpdateSubresource`/拷贝源）。
    resource: ID3D11Resource,
    /// CPU 侧像素缓冲（`width × height BGRA`）。
    pixels: Vec<u8>,
    /// 行跨度（字节）。
    stride: usize,
    /// 覆盖纹理宽。
    width: usize,
    /// 覆盖纹理高。
    height: usize,
}

impl BarcodeOverlay {
    /// 覆盖纹理尺寸：覆盖时间码黑底带（x 8..504，y 8..40）即可。
    const WIDTH: usize = 512;
    /// 覆盖纹理高。
    const HEIGHT: usize = 64;

    /// 创建覆盖纹理。
    ///
    /// # Arguments
    /// * `device` - 管线 D3D11 设备（与采集/编码器同一设备）。
    ///
    /// # Returns
    /// 覆盖器。
    ///
    /// # Errors
    /// 纹理创建失败时返回错误文本。
    fn new(device: &ID3D11Device) -> Result<Self, String> {
        let desc = D3D11_TEXTURE2D_DESC {
            Width: Self::WIDTH as u32,
            Height: Self::HEIGHT as u32,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: 0,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut texture: Option<ID3D11Texture2D> = None;
        unsafe {
            device
                .CreateTexture2D(&desc, None, Some(&mut texture))
                .map_err(|error| format!("创建时间码覆盖纹理失败: {error}"))?;
        }
        let texture = texture.ok_or_else(|| "没有时间码覆盖纹理".to_owned())?;
        let resource: ID3D11Resource = texture
            .cast()
            .map_err(|error| format!("cast ID3D11Resource: {error}"))?;
        Ok(Self {
            _texture: texture,
            resource,
            pixels: vec![0u8; Self::WIDTH * Self::HEIGHT * 4],
            stride: Self::WIDTH * 4,
            width: Self::WIDTH,
            height: Self::HEIGHT,
        })
    }

    /// 把时间码画进覆盖纹理并覆盖到帧纹理左上角。
    ///
    /// # Arguments
    /// * `context` - 管线立即上下文（与采集线程同一上下文，已开启多线程保护）。
    /// * `frame_texture` - 采集帧纹理（`ID3D11Texture2D*`）。
    /// * `array_slice` - 帧纹理数组层号。
    /// * `elapsed_ms` - 已编码时间码值（毫秒）。
    ///
    /// # Returns
    /// 成功返回 `Ok(())`。
    ///
    /// # Errors
    /// 纹理句柄无效或类型不符时返回错误文本。
    fn draw(
        &mut self,
        context: &ID3D11DeviceContext,
        frame_texture: usize,
        array_slice: u32,
        elapsed_ms: u32,
    ) -> Result<(), String> {
        self.pixels.fill(0);
        draw_barcode(&mut self.pixels, self.width, elapsed_ms);
        // 只拷贝时间码黑底带（draw_barcode 已把该矩形填满，见函数内 band_* 计算）。
        let band_left = (BARCODE_X0 - 8) as u32;
        let band_top = (BARCODE_Y0 - 8) as u32;
        let band_right = (BARCODE_X0 + BARCODE_BITS as usize * BARCODE_PITCH + 8) as u32;
        let band_bottom = (BARCODE_Y0 + BARCODE_CELL + 8) as u32;
        debug_assert!(band_right <= self.width as u32 && band_bottom <= self.height as u32);
        unsafe {
            context.UpdateSubresource(
                &self.resource,
                0,
                None,
                self.pixels.as_ptr() as *const std::ffi::c_void,
                self.stride as u32,
                0,
            );
            let raw = frame_texture as *mut std::ffi::c_void;
            let dst = ID3D11Texture2D::from_raw_borrowed(&raw)
                .ok_or_else(|| "时间码覆盖：帧纹理句柄为空".to_owned())?;
            let dst_resource: ID3D11Resource = dst
                .cast()
                .map_err(|error| format!("cast ID3D11Resource: {error}"))?;
            let bx = D3D11_BOX {
                left: band_left,
                top: band_top,
                front: 0,
                right: band_right,
                bottom: band_bottom,
                back: 1,
            };
            context.CopySubresourceRegion(
                &dst_resource,
                array_slice,
                0,
                0,
                0,
                &self.resource,
                0,
                Some(&bx),
            );
        }
        Ok(())
    }
}

struct PipelineParts {
    /// 管线所需的全部原生资源（只在管线线程内创建、使用与释放）。
    info: SetupInfo,
    _device: ID3D11Device,
    context: ID3D11DeviceContext,
    textures: [ID3D11Texture2D; 2],
    events: [HANDLE; 2],
    converter: VideoConverter,
    encoder: Box<dyn rdc_codec::VideoEncoder>,
    decoder: Box<dyn VideoDecoder>,
    /// GPU 时间码覆盖器（纹理直出路径使用）。
    overlay: BarcodeOverlay,
}

/// 采集源、接收端与已初始化管线资源的组合。
type PipelineSetup = (
    Box<dyn rdc_capture::CaptureSource>,
    Arc<FrameQueue>,
    PipelineParts,
);

/// 在管线线程内创建采集源、D3D11 资源、编码器与解码器，并启动采集。
///
/// # Returns
/// 采集源、接收端与已初始化管线资源的组合。
///
/// # Errors
/// 任一步失败时返回错误文本；此时已创建的资源随值析构自动回收。
fn setup_pipeline(stats: &Arc<Stats>) -> Result<PipelineSetup, String> {
    // 1) 采集源与显示器（用真实主显示器尺寸创建纹理）。
    let mut source = create_source().map_err(|error| format!("创建采集源失败: {error}"))?;
    let displays = source.enumerate();
    let display = displays
        .iter()
        .find(|display| display.is_primary)
        .or_else(|| displays.first())
        .cloned()
        .ok_or_else(|| "未枚举到任何显示器".to_owned())?;
    let width = display.width;
    let height = display.height;
    let backend = source.backend_name().to_owned();
    Stats::set_text(&stats.backend, backend.clone());

    // 2) D3D11 设备（必须带 VIDEO_SUPPORT，硬件解码 MFT 才能绑定）。
    let mut device: Option<ID3D11Device> = None;
    let mut context: Option<ID3D11DeviceContext> = None;
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            None,
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
        .map_err(|error| format!("D3D11CreateDevice: {error}"))?;
    }
    let device = device.ok_or_else(|| "没有 D3D11 设备".to_owned())?;
    let context = context.ok_or_else(|| "没有 D3D11 上下文".to_owned())?;
    if let Ok(multithread) = device.cast::<ID3D10Multithread>() {
        unsafe {
            let _ = multithread.SetMultithreadProtected(true); // gate-allow R-C-2: D3D11 线程保护为可选优化，失败时按单线程约束使用设备
        }
    }

    // 3) 两张共享 BGRA 纹理（双缓冲）：Flutter 每次取「刚写完」的那张。
    let (textures, handles) = create_shared_textures(&device, width, height)?;
    let events = [
        create_event().map_err(|error| format!("CreateEvent A: {error}"))?,
        create_event().map_err(|error| format!("CreateEvent B: {error}"))?,
    ];

    // 4) 视频处理器：解码 NV12 → 共享 BGRA，全程 GPU。
    let converter = VideoConverter::new(&device, &context, &textures, width, height)?;
    // 时间码覆盖纹理（纹理直出路径使用；CPU 路径不碰它）。
    let overlay = BarcodeOverlay::new(&device)?;

    // 5) 编码器 / 解码器。
    // 选择走能力探测（AMF → NVENC → oneVPL → MFT → 软编），并把「最终后端 + 理由」带出来，
    // 由日志与状态接口共同呈现，不静默降级（backend.rs 的契约）。
    let selection = create_encoder_selected(CodecKind::H264)
        .map_err(|error| format!("创建编码器失败: {error}"))?;
    let encoder_reason = selection.reason;
    let mut encoder = selection.encoder;
    // 纹理直出（S7-3）：只有厂商后端（AMF）接受 D3D11 直入；MFT 只能吃系统内存。
    // 采集侧与编码器必须共用同一设备，否则纹理跨适配器/跨设备不可用。
    //
    // 默认关闭：RX580 / 驱动 31.0.21912.14 实测，AMF 的 `CreateSurfaceFromDX11Native`
    // 在本进程共享设备上每帧引入 30~140ms 的驱动侧同步（隔离 spike 只有 1.8ms），
    // 整条管线从 60fps 掉到 5~10fps。保留实现，用 `RDC_CAPTURE_GPU=1` 显式启用并复测。
    let want_gpu = std::env::var("RDC_CAPTURE_GPU")
        .map(|v| v != "0")
        .unwrap_or(false);
    if !want_gpu {
        tracing::info!("[control] 纹理直出未启用（默认 host BGRA；RDC_CAPTURE_GPU=1 可启用实验路径）");
    }
    let mut gpu_pipeline = false;
    if want_gpu
        && encoder.attach_d3d11(
            device.as_raw() as usize,
            context.as_raw() as usize,
            true,
        )
    {
        if source.set_gpu_device(device.as_raw() as usize, context.as_raw() as usize) {
            gpu_pipeline = true;
        } else {
            // 编码器已切到纹理模式但采集给不出纹理：显式退回 host 模式，不留半开状态。
            encoder.attach_d3d11(0, 0, false);
            tracing::warn!("[control] 采集侧拒绝纹理直出，编码器回退 host BGRA（非静默）");
        }
    }
    encoder
        .configure(VideoConfig {
            codec: CodecKind::H264,
            width,
            height,
            fps: TARGET_FPS,
            bitrate_bps: TARGET_BITRATE,
            chroma: VideoChroma::Yuv420,
            bit_depth: 8,
            low_latency: true,
        })
        .map_err(|error| format!("配置编码器失败: {error}"))?;
    // configure 之后 `backend_name()` 才是真实选中值（MFT 会带驱动友好名）。
    let encoder_name = encoder.backend_name().to_owned();
    tracing::info!(
        "[control] capture={} encoder={} reason={}",
        if gpu_pipeline {
            "dxgi-texture-direct"
        } else {
            "cpu-bgra"
        },
        encoder_name,
        encoder_reason
    );
    // 解码后端按构建 feature 与 `RDC_DECODER` 选择：默认 FFmpeg（d3d11va 直出纹理），
    // 打开失败自动回退 MF，最终值反映在 `DecoderInfo` 里。
    let decoder = open_video_decoder(Some(&device), width, height, TARGET_FPS, true)
        .map_err(|error| format!("打开解码器失败: {error}"))?;
    let decoder_info = decoder.info();
    let decoder_name = format!("{}/{}", decoder_info.backend, decoder_info.name);
    let decode_path = format!(
        "{}/{}",
        decoder_info.backend,
        decoder_info.output.as_str()
    );
    tracing::info!(
        "[control] decoder={} ({})；输出类型待首帧确定",
        decoder_info.backend, decoder_info.name
    );
    Stats::set_text(&stats.decoder, decoder_name.clone());
    Stats::set_text(&stats.decode_path, decode_path.clone());
    Stats::set_text(&stats.encoder, encoder_name.clone());
    Stats::set_text(&stats.encoder_reason, encoder_reason.clone());

    // 6) 采集直投队列与启动：采集线程产出帧后直接入队，由编码线程独占消费。
    let queue = Arc::new(FrameQueue::new(CHANNEL_CAP, Arc::clone(&stats.dropped)));
    source.set_sink(Arc::new(FrameQueueSink {
        queue: Arc::clone(&queue),
    }));
    source
        .start(CaptureConfig {
            display_id: display.id,
            target_fps: TARGET_FPS,
            include_cursor: true,
            chroma: ChromaFormat::Bgra,
            bit_depth: 8,
        })
        .map_err(|error| format!("启动采集失败: {error}"))?;

    let info = SetupInfo {
        width,
        height,
        share_handles: handles,
        events: [events[0].0 as i64, events[1].0 as i64],
        backend,
        encoder: encoder_name,
        encoder_reason,
        decoder: decoder_name,
        decode_path,
    };
    let parts = PipelineParts {
        info,
        _device: device,
        context,
        textures,
        events,
        converter,
        encoder,
        decoder,
        overlay,
    };
    Ok((source, queue, parts))
}

// =====** 管线线程 =====**

/// 单帧在管线内的关键时间戳（同一帧一路带下去，抖动归因才可加和）。
#[derive(Clone, Copy, Default)]
struct FrameTiming {
    /// 采集瞬间 QPC（微秒；与 [`qpc_us`] 同源）。
    capture_qpc: u64,
    /// 采集段耗时（微秒）。
    capture_us: u64,
    /// 捕获瞬间 → 主线程取到帧的排队时长（微秒）。
    rx_queue_us: u64,
    /// 采集帧到手 → 编码入队耗时（微秒）。
    push_us: u64,
    /// 提交完成瞬间 QPC（微秒）。
    submit_qpc: u64,
    /// 提交 → 出包耗时（微秒，编码器内部时延）。
    enc_wait_us: u64,
    /// 编码线程等采集帧的阻塞时长（微秒；区分「采集没出帧」与「线程忙」）。
    rx_wait_us: u64,
    /// 编码线程从取到帧到提交完成的忙时时长（微秒）。
    submit_busy_us: u64,
    /// 捕获瞬间 → 入队（采集回读 + 发布）耗时（微秒），`rx_queue_us` 的固定底座。
    publish_us: u64,
    /// 颜色交换 + 时间码 + `encoder.push` 全程（微秒），用于区分「我们的准备」与「上传调用」。
    submit_call_us: u64,
}

/// 取包结果（把出包与它的时间戳绑在一起，供解码侧与明细日志使用）。
struct EncodeOut {
    packet: EncodedPacket,
    timing: FrameTiming,
}

/// 编码线程独占的一组资源（与主线程共用同一 D3D11 设备，已启用多线程保护）。
struct EncodeStage {
    /// 立即上下文（与主线程呈现侧是同一 COM 对象的两个引用）。
    context: ID3D11DeviceContext,
    /// 时间码覆盖器（纹理直出路径使用）。
    overlay: BarcodeOverlay,
    /// 编码器。
    encoder: Box<dyn rdc_codec::VideoEncoder>,
}

// 安全性说明：windows crate 的 COM 接口是 `!Send`（内部是裸指针）。此处的移动只发生在
// 「创建线程 → 编码线程」之间，没有并发创建/析构；设备在 setup 时已
// `SetMultithreadProtected(true)`，编码线程与主线程对同一立即上下文的调用由 D3D11 运行时串行化。
unsafe impl Send for EncodeStage {}

/// 编码线程：采集直投队列 → 提交编码 → 取包 → 包队列。主循环只从包队列取。
///
/// 相位锁根因（S7-7 F2 第 1 条，本轮唯一的改动依据）：
/// 1. 采集在**每次 vsync 后 0.2 ms**就把新帧交给下游（DDA 以新桌面帧为信号），生产节拍恒为 16.7 ms；
/// 2. 旧结构里，主循环一周期要串行做完「等帧 → 提交（push 1.7 ms）→ 等包（enc_wait 8.7 ms）
///    → 解码/呈现（0.5 ms）」，忙段合计 ≈11~13 ms；
/// 3. 于是新帧落地时主循环几乎总在忙段里，帧只能在队里等到本轮结束才被取走，
///    实测 `rx_queue` p50 13.48 / p99 34.93 ≈ 1~2 个刷新周期，与 (2) 的忙段长度吻合。
///
/// 也就是说：**消费周期与生产周期同为 60 Hz，两者锁相**，不是某一项耗时超标。
///
/// 本线程把「提交 + 等包」从主循环里拿走：它一周期仍是 ≈11 ms，但**阻塞在采集队列上**，
/// 帧一落地就被取走；主循环只剩解码/呈现（≈0.5 ms/帧），两侧都留出余量，
/// 队列里不再积压一个刷新周期。
///
/// S7-9（本轮）：本线程的单轮串行工时 W（p50 14.0 / 尾部 ≈15.9）仍贴着 16.7 ms 帧周期，
/// 13.1% 的帧因此漏一拍。抓手是把「准备」与「取包」拆成两级流水：第 N 帧在飞时，
/// 先完成第 N+1 帧的 CPU 准备段（时间码覆盖 / 色彩交换），再回到取包循环，
/// 准备段与 `enc_wait` 重叠，W 里不再含准备段。
///
/// **红线**：提前提交必须有界（`in_flight ≤ 1`）。S7-8 实测过「取包循环里队列有帧就提前
/// 回去取帧」（无界提前提交）：AMF 输入队列被灌满后 `push` 阻塞、**全程 0 包**（`s78-final`）。
///
/// 已准备、待提交的一帧（S7-9：准备段与提交段分离）。
enum PreparedFrame {
    /// 纹理直出路径：`overlay.draw` 已完成，等待 `push_texture`。
    Gpu {
        /// 目标纹理裸句柄（`rdc_capture::GpuFrame::texture`）。
        texture: usize,
        /// 纹理数组层号。
        array_slice: u32,
        /// 帧本体（持有纹理租约，提交后延迟归还）。
        frame: Frame,
        /// 已填好采集/排队分段、待补提交分段的计时。
        timing: FrameTiming,
        /// 采集时间戳（纳秒）。
        pts_ns: u64,
        /// 是否请求关键帧。
        force_key: bool,
    },
    /// CPU BGRA 路径：`swap_rb` + 时间码已完成，等待 `push`。
    Cpu {
        /// 帧本体（缓冲已就地转换）。
        frame: Frame,
        /// 已填好采集/排队分段、待补提交分段的计时。
        timing: FrameTiming,
        /// 采集时间戳（纳秒）。
        pts_ns: u64,
        /// 是否请求关键帧。
        force_key: bool,
    },
}

impl PreparedFrame {
    /// 取准备阶段的分段计时（提交时补 `submit_qpc` / `submit_call_us`）。
    fn timing(&self) -> FrameTiming {
        match self {
            Self::Gpu { timing, .. } | Self::Cpu { timing, .. } => *timing,
        }
    }

    /// 本帧是否必须编成 IDR（首帧；或看门狗重建会话后的第一帧）。
    fn force_key(&self) -> bool {
        match self {
            Self::Gpu { force_key, .. } | Self::Cpu { force_key, .. } => *force_key,
        }
    }
}

/// 准备一帧：时间码覆盖（纹理路径）或色彩交换 + 时间码（CPU 路径）。**不含提交**。
///
/// 返回 `Ok(None)` 表示该帧被丢弃（尺寸变化，错误已写入 `stats`）。
#[allow(clippy::too_many_arguments)]
fn prepare_frame(
    queued: QueuedFrame,
    rx_wait_us: u64,
    overlay: &mut BarcodeOverlay,
    context: &ID3D11DeviceContext,
    stats: &Stats,
    width: u32,
    height: u32,
    elapsed_ms: u32,
) -> Result<Option<PreparedFrame>, String> {
    let QueuedFrame {
        mut frame,
        enqueue_qpc,
    } = queued;
    if frame.width != width || frame.height != height {
        return Err(format!(
            "采集尺寸变化 {}x{}，当前纹理 {width}x{height}",
            frame.width, frame.height
        ));
    }
    // 捕获瞬间 → 编码线程取到帧 的排队时长（与采集同源 QPC，可直接相减）。
    let rx_queue_us = qpc_us().saturating_sub(frame.meta.qpc_us);
    // 其中：捕获瞬间 → 入队（采集回读 + 发布），剩下的是真正在队列里等的时间。
    let publish_us = enqueue_qpc.saturating_sub(frame.meta.qpc_us);
    stats.captured.fetch_add(1, Ordering::Relaxed);
    stats
        .capture_us
        .store(u64::from(frame.capture_cost_us), Ordering::Relaxed);
    let capture_us = u64::from(frame.capture_cost_us);
    let capture_qpc = frame.meta.qpc_us;
    let pts_ns = frame.meta.pts_ns;
    let force_key = stats.captured.load(Ordering::Relaxed) == 1;
    let mut timing = FrameTiming {
        capture_qpc,
        capture_us,
        rx_queue_us,
        rx_wait_us,
        publish_us,
        ..FrameTiming::default()
    };
    // 段计时：采集帧到手 → 准备完成（含时间码覆盖/色彩交换），不含提交调用。
    let t_prep = Instant::now();
    if let Some((texture, array_slice)) = frame.gpu.as_ref().map(|g| (g.texture, g.array_slice)) {
        // 纹理直出：时间码用 GPU 小纹理覆盖，像素从不回读 CPU。
        overlay.draw(context, texture, array_slice, elapsed_ms)?;
        timing.push_us = t_prep.elapsed().as_micros() as u64;
        return Ok(Some(PreparedFrame::Gpu {
            texture,
            array_slice,
            frame,
            timing,
            pts_ns,
            force_key,
        }));
    }
    let len = frame.width as usize * frame.height as usize * 4;
    if frame.buffer.as_slice().len() < len {
        return Err("采集帧缓冲小于整帧长度".to_owned());
    }
    {
        let pixels = &mut frame.buffer.as_mut_slice()[..len];
        // 编解码层的像素契约是 RGBA8（rdc-protocol/rdc-codec、nv12.rs、AMF shim
        // 的 push_rgba 都按 RGBA8 收），所以这里必须做 B↔R 交换。
        // 块 4 B 实测：删掉交换后 push 段 4.66 → 4.34~6.93 ms（噪声内，无收益），
        // 且 AMF shim 的 AMF_SURFACE_BGRA 标签与层契约不一致，故保留交换。
        swap_rb(pixels);
        draw_barcode(pixels, frame.width as usize, elapsed_ms);
    }
    timing.push_us = t_prep.elapsed().as_micros() as u64;
    Ok(Some(PreparedFrame::Cpu {
        frame,
        timing,
        pts_ns,
        force_key,
    }))
}

#[allow(clippy::too_many_arguments)]
fn run_encode_stage(
    stage: EncodeStage,
    queue: Arc<FrameQueue>,
    packets: SyncSender<EncodeOut>,
    stats: Arc<Stats>,
    stop: Arc<AtomicBool>,
    width: u32,
    height: u32,
    start: Instant,
) {
    let EncodeStage {
        context,
        mut overlay,
        mut encoder,
    } = stage;
    // 有界在飞：已提交、尚未出包的帧时序（只用于分段计时）。
    // 上限 `MAX_IN_FLIGHT = 3`（见常量注释）：覆盖异步 MFT 的 1~2 帧固有延迟；
    // 绝不允许退回「无界提前提交」。
    let mut in_flight: VecDeque<FrameTiming> = VecDeque::with_capacity(MAX_IN_FLIGHT + 1);
    // 已准备、等待提交的下一帧（准备段与该帧在飞期间的耗时重叠）。
    let mut prepared: Option<PreparedFrame> = None;
    // 纹理直出路径下延迟归还的帧（租约保护：编码器可能异步持有纹理）。
    let mut held_gpu_frames: VecDeque<Frame> = VecDeque::with_capacity(4);
    // 看门狗状态：最后一次「看到包」的时刻 + 本次失速已重建的次数。
    let mut last_output = Instant::now();
    let mut resets_for_stall: u32 = 0;
    let mut stall_reported = false;
    // 重建会话后必须用 IDR 重启码流，解码侧才能恢复。
    let mut force_key_next = true;

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let mut poll_us: u64 = 0;
        let mut progressed = false;

        // ① 取包段：主动驱动异步 MFT，把已就绪的输出**全部**取出（不等）。
        // 只 `pop_front` 本地队列会让「等包 → 不提交 → 更不出包」自锁（J4 根因）。
        loop {
            let t_poll = Instant::now();
            let packet = encoder.poll_packet();
            poll_us += t_poll.elapsed().as_micros() as u64;
            let Some(packet) = packet else {
                break;
            };
            progressed = true;
            last_output = Instant::now();
            resets_for_stall = 0;
            // 只在「此前确实报过看门狗错误」时清错，避免误清解码/采集侧的真实错误。
            if stall_reported {
                stats.clear_error();
                stall_reported = false;
            }
            // 每包按提交顺序 FIFO 配一个时序；记录缺失时用默认值（统计失真但不致命）。
            let mut timing = in_flight.pop_front().unwrap_or_default(); // gate-allow R-C-2: 只影响单包时序统计，热路径不逐帧打日志
            timing.enc_wait_us = qpc_us().saturating_sub(timing.submit_qpc);
            stats.encoded.fetch_add(1, Ordering::Relaxed);
            stats
                .encoded_bytes
                .fetch_add(packet.data.len() as u64, Ordering::Relaxed);
            // push 段在编码线程里补全，按出包时刻落地。
            stats.encode_us.store(timing.push_us, Ordering::Relaxed);
            // 包队列深度 2：满了就等（背压回传到采集队列，由它按「丢最旧」处理）。
            let mut out = EncodeOut { packet, timing };
            loop {
                match packets.try_send(out) {
                    Ok(()) => break,
                    Err(TrySendError::Full(back)) => {
                        if stop.load(Ordering::Relaxed) {
                            return;
                        }
                        std::thread::sleep(Duration::from_millis(1));
                        out = back;
                    }
                    Err(TrySendError::Disconnected(_)) => return,
                }
            }
        }

        // ② 提交段：在飞记录达到上限时丢最旧并计数（已提交帧无法撤回），始终不阻塞。
        if let Some(frame) = prepared.take() {
            if in_flight.len() >= MAX_IN_FLIGHT {
                in_flight.pop_front();
                stats.encoder_dropped.fetch_add(1, Ordering::Relaxed);
            }
            let t_submit = Instant::now();
            let mut timing = frame.timing();
            let force_key = force_key_next || frame.force_key();
            let submitted = match frame {
                PreparedFrame::Gpu {
                    texture,
                    array_slice,
                    frame,
                    pts_ns,
                    ..
                } => encoder
                    .push_texture(texture, array_slice, pts_ns, force_key)
                    .map_err(|error| format!("编码入队失败（纹理）: {error}"))
                    .map(|()| {
                        // 租约延迟 3 帧归还：AMF 可能在 SubmitInput 返回后仍持有纹理直到出包。
                        held_gpu_frames.push_back(frame);
                        if held_gpu_frames.len() > 3 {
                            held_gpu_frames.pop_front();
                        }
                    }),
                PreparedFrame::Cpu {
                    frame, pts_ns, ..
                } => encoder
                    .push(frame.buffer, pts_ns, force_key)
                    .map_err(|error| format!("编码入队失败: {error}")),
            };
            match submitted {
                Ok(()) => {
                    timing.submit_qpc = qpc_us();
                    timing.submit_call_us = t_submit.elapsed().as_micros() as u64;
                    timing.submit_busy_us = timing.push_us + timing.submit_call_us;
                    in_flight.push_back(timing);
                    force_key_next = false;
                    progressed = true;
                }
                Err(message) => stats.set_error(message),
            }
        }
        // ③ 准备段：没有待提交帧时才取一帧做准备（与在飞帧的 enc_wait 重叠）。
        // 有帧在飞时用 1 ms 超时：队列暂时空也不能把取包拖后。
        if prepared.is_none() {
            let t_rx = Instant::now();
            let pop_timeout_ms = if in_flight.is_empty() { 20 } else { 1 };
            match queue.pop_timeout(Duration::from_millis(pop_timeout_ms)) {
                QueuePop::Frame(queued) => {
                    let rx_wait_us = t_rx.elapsed().as_micros() as u64;
                    let elapsed_ms = start.elapsed().as_millis() as u32 % 1_000_000;
                    match prepare_frame(
                        queued,
                        rx_wait_us,
                        &mut overlay,
                        &context,
                        &stats,
                        width,
                        height,
                        elapsed_ms,
                    ) {
                        Ok(prepared_now) => {
                            prepared = prepared_now;
                            progressed = true;
                        }
                        Err(message) => stats.set_error(message),
                    }
                }
                QueuePop::Timeout => {}
                QueuePop::Closed => break,
            }
        }

        // ④ 看门狗：有帧在飞却连续 ENCODE_WATCHDOG_MS 无包 → 重建一次；
        //    重建后再一个阈值仍无包 → 报稳定错误码，写日志不静默。
        if !in_flight.is_empty() && last_output.elapsed() >= Duration::from_millis(ENCODE_WATCHDOG_MS) {
            if resets_for_stall == 0 {
                match encoder.reset_session() {
                    Ok(()) => {
                        stats.encoder_resets.fetch_add(1, Ordering::Relaxed);
                        resets_for_stall = 1;
                        force_key_next = true;
                        // 旧会话的在飞时序与待提交帧都失效（纹理租约随 frame drop 归还）。
                        in_flight.clear();
                        prepared = None;
                        stats.encoder_in_flight.store(0, Ordering::Relaxed);
                        last_output = Instant::now();
                        tracing::warn!(
                            "[enc] 看门狗：{ENCODE_WATCHDOG_MS} ms 无包，已重建编码会话一次"
                        );
                    }
                    Err(error) => {
                        resets_for_stall = 1;
                        tracing::error!(
                            "[enc] 看门狗：重建编码会话失败: {error}"
                        );
                        stats.force_error(format!(
                            "RDC-ENC-001 编码器无输出（重建失败: {error}）"
                        ));
                    }
                }
            } else if !stall_reported {
                stall_reported = true;
                tracing::error!(
                    "[enc] 看门狗：重建 {resets_for_stall} 次后仍无包输出，报 RDC-ENC-001"
                );
                stats.force_error(format!(
                    "RDC-ENC-001 编码器无输出（已重建 {resets_for_stall} 次）"
                ));
            } else {
                // 已报过：按周期刷新计时，避免每轮重复写日志。
                last_output = Instant::now();
            }
        }

        stats.poll_us.store(poll_us, Ordering::Relaxed);
        stats
            .encoder_in_flight
            .store(in_flight.len() as u64, Ordering::Relaxed);
        // 全轮没有取到包、没有提交、也没有取到帧：短暂让出 CPU，避免空转。
        if !progressed {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// 管线主线：采集直投 → [`run_encode_stage`] → 这里只做取包 → 解码 → 共享纹理 → 通知 Flutter。
fn run_pipeline(
    mut source: Box<dyn rdc_capture::CaptureSource>,
    queue: Arc<FrameQueue>,
    parts: PipelineParts,
    stats: Arc<Stats>,
    stop: Arc<AtomicBool>,
) {
    let PipelineParts {
        info: _info,
        _device,
        context,
        textures,
        events,
        converter,
        encoder,
        mut decoder,
        overlay,
    } = parts;
    let start = Instant::now();
    // 编码线程：独占 encoder/overlay/上下文；主循环不再同时承担消费与提交（S7-8 议题）。
    let (packet_tx, packet_rx) = sync_channel::<EncodeOut>(PACKET_CAP);
    let encode_join = {
        let queue = Arc::clone(&queue);
        let stats = Arc::clone(&stats);
        let stop = Arc::clone(&stop);
        let context = context.clone();
        let (width, height) = (converter.width, converter.height);
        thread::Builder::new()
            .name("rdc-control-encode".to_owned())
            .spawn(move || {
                run_encode_stage(
                    EncodeStage {
                        context,
                        overlay,
                        encoder,
                    },
                    queue,
                    packet_tx,
                    stats,
                    stop,
                    width,
                    height,
                    start,
                );
            })
    };
    if let Err(error) = &encode_join {
        stats.set_error(format!("启动编码线程失败: {error}"));
    }
    let mut index = 0usize;
    let mut displayed_window = 0u64;
    let mut bytes_window = 0u64;
    let mut window_start = Instant::now();
    let mut last_readback = Instant::now() - Duration::from_millis(READBACK_INTERVAL_MS);
    let mut latency_samples: Vec<u64> = Vec::with_capacity(64);
    let mut frame_views: HashMap<usize, ID3D11VideoProcessorInputView> = HashMap::new();
    let mut log_file = open_log();
    // 解码侧时间戳（按包入队、按解出出队）；capacity: 32，超出即清空（见下方 len>32 判断）。
    let mut pending_decode: VecDeque<FrameTiming> = VecDeque::with_capacity(32);
    let mut enc_latency_us: u64 = 0;
    // 一次送入可能解出 0..N 帧，复用缓冲避免每包分配。
    let mut decoded_frames: Vec<DecodedFrame> = Vec::new();
    // 抖动归因（S7-5 块 1）：`RDC_FRAME_TRACE=1` 时逐解出帧落一行明细，默认关闭。
    let mut trace = open_frame_trace();
    let mut trace_seq: u64 = 0;
    let mut last_drops: u64 = 0;
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        // 主循环只等编码线程的包：本线程不再提交、不再 poll 编码器。
        let out = match packet_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(out) => Some(out),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => None,
            // 编码线程已退出（停止或出错）。
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let mut decode_us: u64 = 0;
        let mut present_us: u64 = 0;
        if let Some(out) = out {
            let packet = out.packet;
            bytes_window += packet.data.len() as u64;
            let now_qpc = qpc_us();
            let diff = now_qpc.saturating_sub(out.timing.capture_qpc) / 1000;
            if diff < 1_000 {
                enc_latency_us = diff * 1000;
            }
            pending_decode.push_back(out.timing);
            decoded_frames.clear();
            let t_decode = Instant::now();
            let feed_result = decoder.feed_each(
                &packet.data,
                packet.pts_ns / 1000,
                &mut |frame| {
                    decoded_frames.push(frame);
                    Ok(())
                },
            );
            decode_us += t_decode.elapsed().as_micros() as u64;
            match feed_result {
                Ok(()) => {
                    for decoded in decoded_frames.drain(..) {
                        stats.decoded.fetch_add(1, Ordering::Relaxed);
                        // 编解码管线时延（不含 Flutter 合成），用于与画内回读互相校验。
                        let timing = pending_decode.pop_front();
                        let decode_qpc = qpc_us();
                        if let Some(t) = timing {
                            let diff = decode_qpc.saturating_sub(t.capture_qpc) / 1000;
                            if diff < 1_000 {
                                stats
                                    .pipeline_latency_us
                                    .store(diff * 1000, Ordering::Relaxed);
                            }
                        }
                        if pending_decode.len() > 32 {
                            pending_decode.clear();
                        }
                        let t_present = Instant::now();
                        let result = if let Some(texture) = decoded.texture.as_ref() {
                            present_gpu(
                                &context,
                                &converter,
                                index,
                                &mut frame_views,
                                texture.texture.clone(),
                                texture.array_slice,
                            )
                        } else {
                            present_cpu(
                                &context,
                                &converter,
                                &textures[index],
                                &decoded,
                            )
                        };
                        present_us += t_present.elapsed().as_micros() as u64;
                        let present_one_us = t_present.elapsed().as_micros() as u64;
                        if let Err(error) = result {
                            stats.set_error(error);
                            continue;
                        }
                        // 抖动归因明细（同一帧的分段加和，残差 = AMF 内部/系统调度）。
                        if let (Some(file), Some(t)) = (trace.as_mut(), timing) {
                            use std::io::Write;
                            let pipeline_us = decode_qpc.saturating_sub(t.capture_qpc);
                            let drops_now = stats.dropped.load(Ordering::Relaxed);
                            let drops_delta = drops_now.saturating_sub(last_drops);
                            last_drops = drops_now;
                            let sum = t.rx_queue_us
                                + t.capture_us
                                + t.push_us
                                + t.enc_wait_us
                                + decode_us;
                            let residual_us = pipeline_us.saturating_sub(sum);
                            let _ = writeln!( // gate-allow R-C-2: RDC_FRAME_TRACE 诊断 CSV，写失败不得中断预览
                                file,
                                "{trace_seq},{},{pipeline_us},{},{},{},{},{decode_us},{present_one_us},{residual_us},{drops_delta},{},{},{},{}",
                                start.elapsed().as_millis(),
                                t.capture_us,
                                t.rx_queue_us,
                                t.push_us,
                                t.enc_wait_us,
                                t.rx_wait_us,
                                t.submit_busy_us,
                                t.publish_us,
                                t.submit_call_us,
                            );
                            trace_seq += 1;
                        }
                        unsafe {
                            let _ = SetEvent(events[index]); // gate-allow R-C-2: 呈现同步事件设置失败无恢复动作，下一帧继续尝试
                        }
                        index ^= 1;
                        stats.displayed.fetch_add(1, Ordering::Relaxed);
                        displayed_window += 1;
                    }
                }
                Err(error) => stats.set_error(format!("解码失败: {error}")),
            }
        }
        stats.decode_us.store(decode_us, Ordering::Relaxed);
        stats.present_us.store(present_us, Ordering::Relaxed);
        stats
            .decoder_in_flight
            .store(decoder.in_flight(), Ordering::Relaxed);

        // 帧率窗口（1 秒）。
        if window_start.elapsed() >= Duration::from_secs(1) {
            let millis = window_start.elapsed().as_millis().max(1) as u64;
            let fps_milli = displayed_window * 1_000_000 / millis;
            stats.fps_milli.store(fps_milli, Ordering::Relaxed);
            // bits/ms 即 kbit/s：窗口内编码字节数 × 8 ÷ 窗口毫秒数。
            let kbps = bytes_window * 8 / millis;
            stats.bitrate_kbps.store(kbps, Ordering::Relaxed);
            // 输出类型要等首帧解出后才能确定（启动时是 unknown），每窗口刷新一次，
            // 使控制页与 CSV 都反映真实路径（如 ffmpeg/dxgi-texture）。
            let decode_info = decoder.info();
            Stats::set_text(&stats.decoder, format!("{}/{}", decode_info.backend, decode_info.name));
            Stats::set_text(
                &stats.decode_path,
                format!("{}/{}", decode_info.backend, decode_info.output.as_str()),
            );
            if let Some(file) = log_file.as_mut() {
                use std::io::Write;
                let _ = writeln!( // gate-allow R-C-2: 逐秒指标 CSV 属诊断输出，写失败不得中断预览
                    file,
                    "{},{:.2},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{},{}",
                    start.elapsed().as_millis(),
                    fps_milli as f64 / 1000.0,
                    stats.captured.load(Ordering::Relaxed),
                    stats.encoded.load(Ordering::Relaxed),
                    stats.decoded.load(Ordering::Relaxed),
                    stats.dropped.load(Ordering::Relaxed),
                    stats.latency_last_us.load(Ordering::Relaxed),
                    stats.latency_p50_us.load(Ordering::Relaxed),
                    stats.latency_p99_us.load(Ordering::Relaxed),
                    stats.pipeline_latency_us.load(Ordering::Relaxed),
                    enc_latency_us,
                    Stats::text(&stats.decode_path),
                    stats.capture_us.load(Ordering::Relaxed),
                    stats.encode_us.load(Ordering::Relaxed),
                    stats.poll_us.load(Ordering::Relaxed),
                    stats.decode_us.load(Ordering::Relaxed),
                    stats.present_us.load(Ordering::Relaxed),
                    stats.decoder_in_flight.load(Ordering::Relaxed),
                    stats.encoder_in_flight.load(Ordering::Relaxed),
                    stats.encoder_resets.load(Ordering::Relaxed),
                    stats.encoder_dropped.load(Ordering::Relaxed),
                );
            }
            displayed_window = 0;
            bytes_window = 0;
            window_start = Instant::now();
        }

        // 时间码回读：每 500ms 一次，只拷左上角小区域，避免逐帧整帧回读。
        let mut readback_us: u64 = 0;
        if last_readback.elapsed() >= Duration::from_millis(READBACK_INTERVAL_MS) {
            last_readback = Instant::now();
            let t_readback = Instant::now();
            let read_index = index ^ 1;
            if let Some(value) = read_barcode(&context, &converter, &textures[read_index]) {
                let now_ms = start.elapsed().as_millis() as u64 % 1_000_000;
                let latency = now_ms.wrapping_sub(value as u64) % 1_000_000;
                // 超过 600ms 的差值视为回绕/陈旧帧，不计入统计。
                if latency < 600_000 {
                    stats.latency_last_us.store(latency * 1000, Ordering::Relaxed);
                    latency_samples.push(latency * 1000);
                    if latency_samples.len() > 60 {
                        latency_samples.remove(0);
                    }
                    let mut sorted = latency_samples.clone();
                    sorted.sort_unstable();
                    let p50 = sorted[sorted.len() / 2];
                    let p99 = sorted[(sorted.len() * 99 / 100).min(sorted.len() - 1)];
                    stats.latency_p50_us.store(p50, Ordering::Relaxed);
                    stats.latency_p99_us.store(p99, Ordering::Relaxed);
                    stats.readback_count.fetch_add(1, Ordering::Relaxed);
                }
            }
            readback_us = t_readback.elapsed().as_micros() as u64;
        }
        let _ = readback_us; // gate-allow R-C-2: 非错误值，readback_us 是预留计时变量，此处显式消费以避免未使用告警
    }

    // 先停采集，再关直投队列：编码线程从 `pop_timeout` 返回后看到停止标志即退出。
    source.stop();
    drop(source);
    queue.close();
    if let Ok(join) = encode_join {
        let _ = join.join(); // gate-allow R-C-2: 编码线程已收到停止标志，join 失败不改变收尾结论
    }
    drop(decoder);
    // 纹理随 device 释放；事件句柄必须显式关闭。
    unsafe {
        let _ = CloseHandle(events[0]); // gate-allow R-C-2: 线程退出时关闭事件句柄，失败仅可能由句柄已失效引起
        let _ = CloseHandle(events[1]); // gate-allow R-C-2: 同上
    }
}

// =====** 呈现：GPU 路径 =====**

/// 硬解 NV12 纹理 → 共享 BGRA 纹理（VideoProcessorBlt）。
fn present_gpu(
    context: &ID3D11DeviceContext,
    converter: &VideoConverter,
    index: usize,
    frame_views: &mut HashMap<usize, ID3D11VideoProcessorInputView>,
    source_texture: ID3D11Texture2D,
    array_slice: u32,
) -> Result<(), String> {
    let key = source_texture.as_raw() as usize;
    if let std::collections::hash_map::Entry::Vacant(entry) = frame_views.entry(key) {
        entry.insert(converter.input_view(&source_texture, array_slice)?);
    }
    let input = frame_views
        .get(&key)
        .cloned()
        .ok_or_else(|| "输入视图缺失".to_owned())?;
    converter.blit(context, &input, index)
}

// =====** 呈现：CPU 降级路径 =====**

/// 系统内存 NV12 → BGRA → 暂存纹理上传 → 复制到共享纹理。
fn present_cpu(
    context: &ID3D11DeviceContext,
    converter: &VideoConverter,
    target: &ID3D11Texture2D,
    decoded: &DecodedFrame,
) -> Result<(), String> {
    let mut bgra = vec![0u8; decoded.width as usize * decoded.height as usize * 4];
    nv12_to_bgra(
        &decoded.data,
        decoded.stride as usize,
        decoded.width as usize,
        decoded.height as usize,
        &mut bgra,
    );
    converter.upload_and_copy(context, target, &bgra)
}

// =====** D3D11 资源 =====**

/// 创建两张共享 BGRA 纹理，返回纹理与 legacy shared handle（i64）。
fn create_shared_textures(
    device: &ID3D11Device,
    width: u32,
    height: u32,
) -> Result<([ID3D11Texture2D; 2], [i64; 2]), String> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        // ANGLE 以 EGL client buffer 打开共享句柄，需要 RENDER_TARGET + SHADER_RESOURCE。
        BindFlags: D3D11_BIND_RENDER_TARGET.0 as u32 | D3D11_BIND_SHADER_RESOURCE.0 as u32,
        CPUAccessFlags: 0,
        // legacy shared（非 NT handle）才能被 ANGLE 直接 OpenSharedResource。
        MiscFlags: D3D11_RESOURCE_MISC_SHARED.0 as u32,
    };
    let mut textures: Vec<ID3D11Texture2D> = Vec::with_capacity(2);
    let mut handles = [0i64; 2];
    for (slot, handle_slot) in handles.iter_mut().enumerate() {
        let mut texture: Option<ID3D11Texture2D> = None;
        unsafe {
            device
                .CreateTexture2D(&desc, None, Some(&mut texture))
                .map_err(|error| format!("CreateTexture2D(shared {slot}): {error}"))?;
        }
        let texture = texture.ok_or_else(|| "共享纹理为空".to_owned())?;
        let resource: IDXGIResource = texture
            .cast()
            .map_err(|error| format!("IDXGIResource: {error}"))?;
        let handle = unsafe { resource.GetSharedHandle() }
            .map_err(|error| format!("GetSharedHandle({slot}): {error}"))?;
        *handle_slot = handle.0 as i64;
        textures.push(texture);
    }
    // `Err(Vec<T>)` 携带剩余元素数量，不是 `Error` 类型：显式记录实际数量后返回原文案。
    let array: [ID3D11Texture2D; 2] = match textures.try_into() {
        Ok(array) => array,
        Err(remainder) => {
            tracing::debug!(actual = remainder.len(), expected = 2, "共享纹理数量不匹配");
            return Err("共享纹理数量错误".to_owned());
        }
    };
    Ok((array, handles))
}

/// 创建自动重置事件（初态未触发）。
fn create_event() -> windows::core::Result<HANDLE> {
    unsafe { CreateEventW(None, false, false, None) }
}

/// NV12 → BGRA 的视频处理器与上传/回读资源。
struct VideoConverter {
    /// 输入宽。
    width: u32,
    /// 输入高。
    height: u32,
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    /// 共享纹理 A/B 的输出视图。
    outputs: [ID3D11VideoProcessorOutputView; 2],
    /// CPU 上传用的暂存纹理（CPU 可写，容量等于整帧）。
    staging: ID3D11Texture2D,
    /// 时间码回读用的暂存纹理（CPU 可读，只有左上角小区域）。
    readback: ID3D11Texture2D,
    /// 回读区域尺寸。
    readback_width: u32,
    readback_height: u32,
}

impl VideoConverter {
    /// 创建视频处理器与暂存纹理。
    fn new(
        device: &ID3D11Device,
        context: &ID3D11DeviceContext,
        textures: &[ID3D11Texture2D; 2],
        width: u32,
        height: u32,
    ) -> Result<Self, String> {
        let video_device: ID3D11VideoDevice = device
            .cast()
            .map_err(|error| format!("ID3D11VideoDevice: {error}"))?;
        let video_context: ID3D11VideoContext = context
            .cast()
            .map_err(|error| format!("ID3D11VideoContext: {error}"))?;
        let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: DXGI_RATIONAL {
                Numerator: TARGET_FPS,
                Denominator: 1,
            },
            InputWidth: width,
            InputHeight: height,
            OutputFrameRate: DXGI_RATIONAL {
                Numerator: TARGET_FPS,
                Denominator: 1,
            },
            OutputWidth: width,
            OutputHeight: height,
            Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
        };
        let enumerator = unsafe { video_device.CreateVideoProcessorEnumerator(&content) }
            .map_err(|error| format!("CreateVideoProcessorEnumerator: {error}"))?;
        let processor = unsafe { video_device.CreateVideoProcessor(&enumerator, 0) }
            .map_err(|error| format!("CreateVideoProcessor: {error}"))?;
        // 源矩形 = 目标矩形 = 整帧；NV12 输入按 studio/709 解读，输出全范围 RGB。
        let rect = windows::Win32::Foundation::RECT {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        };
        unsafe {
            video_context.VideoProcessorSetStreamFrameFormat(
                &processor,
                0,
                D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            );
            video_context.VideoProcessorSetStreamSourceRect(&processor, 0, true, Some(&rect));
            video_context.VideoProcessorSetStreamDestRect(&processor, 0, true, Some(&rect));
        }

        let staging = create_staging(device, width, height, D3D11_CPU_ACCESS_WRITE)?;
        let readback_width = (BARCODE_X0 as u32 + BARCODE_BITS * BARCODE_PITCH as u32 + 16).min(width);
        let readback_height = (BARCODE_Y0 as u32 + BARCODE_CELL as u32 + 16).min(height);
        let readback = create_staging(device, readback_width, readback_height, D3D11_CPU_ACCESS_READ)?;

        // 输出视图绑定到两张共享纹理上，Blt 时按槽位选择。
        let mut output_views: Vec<ID3D11VideoProcessorOutputView> = Vec::with_capacity(2);
        for texture in textures {
            output_views.push(output_view(&video_device, &enumerator, texture)?);
        }
        // 同上：`Err(Vec<T>)` 只带剩余元素，记录数量后返回原文案。
        let outputs: [ID3D11VideoProcessorOutputView; 2] = match output_views.try_into() {
            Ok(outputs) => outputs,
            Err(remainder) => {
                tracing::debug!(actual = remainder.len(), expected = 2, "输出视图数量不匹配");
                return Err("输出视图数量错误".to_owned());
            }
        };

        Ok(Self {
            width,
            height,
            video_device,
            video_context,
            enumerator,
            processor,
            outputs,
            staging,
            readback,
            readback_width,
            readback_height,
        })
    }

    /// 为解码纹理创建输入视图。
    fn input_view(
        &self,
        texture: &ID3D11Texture2D,
        array_slice: u32,
    ) -> Result<ID3D11VideoProcessorInputView, String> {
        let desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPIV {
                    MipSlice: 0,
                    ArraySlice: array_slice,
                },
            },
        };
        let mut view: Option<ID3D11VideoProcessorInputView> = None;
        unsafe {
            self.video_device
                .CreateVideoProcessorInputView(texture, &self.enumerator, &desc, Some(&mut view))
                .map_err(|error| format!("CreateVideoProcessorInputView: {error}"))?;
        }
        view.ok_or_else(|| "输入视图为空".to_owned())
    }

    /// Blt 到指定共享纹理槽位（0=A，1=B）。
    fn blit(
        &self,
        _context: &ID3D11DeviceContext,
        input: &ID3D11VideoProcessorInputView,
        index: usize,
    ) -> Result<(), String> {
        let mut stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            OutputIndex: 0,
            InputFrameOrField: 0,
            PastFrames: 0,
            FutureFrames: 0,
            ppPastSurfaces: std::ptr::null_mut(),
            pInputSurface: std::mem::ManuallyDrop::new(Some(input.clone())),
            ppFutureSurfaces: std::ptr::null_mut(),
            ppPastSurfacesRight: std::ptr::null_mut(),
            pInputSurfaceRight: std::mem::ManuallyDrop::new(None),
            ppFutureSurfacesRight: std::ptr::null_mut(),
        };
        let output = self
            .outputs
            .get(index)
            .ok_or_else(|| format!("输出槽位越界: {index}"))?;
        unsafe {
            self.video_context
                .VideoProcessorBlt(&self.processor, output, 0, std::slice::from_ref(&stream))
                .map_err(|error| format!("VideoProcessorBlt: {error}"))?;
            let _ = std::mem::ManuallyDrop::take(&mut stream.pInputSurface); // gate-allow R-C-2: 非错误值，take 只为结束借用并释放输入纹理所有权
        }
        Ok(())
    }

    /// CPU 上传：暂存纹理写 BGRA，再 CopyResource 到共享纹理。
    fn upload_and_copy(
        &self,
        context: &ID3D11DeviceContext,
        target: &ID3D11Texture2D,
        bgra: &[u8],
    ) -> Result<(), String> {
        let mut mapped = windows::Win32::Graphics::Direct3D11::D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            context
                .Map(&self.staging, 0, windows::Win32::Graphics::Direct3D11::D3D11_MAP_WRITE, 0, Some(&mut mapped))
                .map_err(|error| format!("Map(staging): {error}"))?;
        }
        let row_bytes = self.width as usize * 4;
        let pitch = mapped.RowPitch as usize;
        unsafe {
            let dst = mapped.pData as *mut u8;
            for row in 0..self.height as usize {
                std::ptr::copy_nonoverlapping(
                    bgra.as_ptr().add(row * row_bytes),
                    dst.add(row * pitch),
                    row_bytes,
                );
            }
            context.Unmap(&self.staging, 0);
            context.CopyResource(target, &self.staging);
        }
        Ok(())
    }
}

/// 为共享纹理创建视频处理器输出视图。
fn output_view(
    video_device: &ID3D11VideoDevice,
    enumerator: &ID3D11VideoProcessorEnumerator,
    texture: &ID3D11Texture2D,
) -> Result<ID3D11VideoProcessorOutputView, String> {
    let desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
        ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
        Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
            Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
        },
    };
    let mut view: Option<ID3D11VideoProcessorOutputView> = None;
    unsafe {
        video_device
            .CreateVideoProcessorOutputView(texture, enumerator, &desc, Some(&mut view))
            .map_err(|error| format!("CreateVideoProcessorOutputView: {error}"))?;
    }
    view.ok_or_else(|| "输出视图为空".to_owned())
}

/// 创建 STAGING 纹理。
fn create_staging(
    device: &ID3D11Device,
    width: u32,
    height: u32,
    access: windows::Win32::Graphics::Direct3D11::D3D11_CPU_ACCESS_FLAG,
) -> Result<ID3D11Texture2D, String> {
    let desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_STAGING,
        BindFlags: 0,
        CPUAccessFlags: access.0 as u32,
        MiscFlags: 0,
    };
    let mut texture: Option<ID3D11Texture2D> = None;
    unsafe {
        device
            .CreateTexture2D(&desc, None, Some(&mut texture))
            .map_err(|error| format!("CreateTexture2D(staging): {error}"))?;
    }
    texture.ok_or_else(|| "暂存纹理为空".to_owned())
}

// =====** 时间码回读 =====**

/// 从共享纹理左上角拷小区域回读时间码，返回采集时刻（毫秒，模 1_000_000）。
fn read_barcode(
    context: &ID3D11DeviceContext,
    converter: &VideoConverter,
    source: &ID3D11Texture2D,
) -> Option<u32> {
    let box_region = D3D11_BOX {
        left: 0,
        top: 0,
        front: 0,
        right: converter.readback_width,
        bottom: converter.readback_height,
        back: 1,
    };
    unsafe {
        context.CopySubresourceRegion(&converter.readback, 0, 0, 0, 0, source, 0, Some(&box_region));
    }
    let mut mapped = windows::Win32::Graphics::Direct3D11::D3D11_MAPPED_SUBRESOURCE::default();
    unsafe {
        context
            .Map(&converter.readback, 0, windows::Win32::Graphics::Direct3D11::D3D11_MAP_READ, 0, Some(&mut mapped))
            .ok()?;
    }
    let pitch = mapped.RowPitch as usize;
    let mut value = 0u32;
    let mut aligned = true;
    unsafe {
        for cell in 0..=BARCODE_BITS as usize {
            let x = BARCODE_X0 + cell * BARCODE_PITCH + BARCODE_CELL / 2;
            let y = BARCODE_Y0 + BARCODE_CELL / 2;
            let offset = y * pitch + x * 4;
            let pixel = *(mapped.pData as *const u8).add(offset + 1) as u32; // G 通道
            let on = pixel > 100;
            if cell == 0 {
                aligned = on;
            } else if on {
                value |= 1 << (cell - 1);
            }
        }
        // 颜色核对（块 4 C1）：`RDC_COLOR_PROBE=1` 时打印两个色块的回读值。
        if std::env::var("RDC_COLOR_PROBE").map(|v| v != "0").unwrap_or(false) {
            let y = 46usize;
            let read = |x: usize| {
                let o = y * pitch + x * 4;
                let p = (mapped.pData as *const u8).add(o);
                (*p, *p.add(1), *p.add(2))
            };
            let (lb, lg, lr) = read(12);
            let (rb, rg, rr) = read(26);
            tracing::info!(
                "[color] left(BGR)=({lb},{lg},{lr}) right(BGR)=({rb},{rg},{rr}) expected left=(0,0,255) right=(255,0,0)"
            );
        }
        context.Unmap(&converter.readback, 0);
    }
    if aligned {
        Some(value)
    } else {
        None
    }
}

// =====** 日志落盘 =====**

/// 本机单调时钟（微秒）：与采集侧 `FrameMeta::qpc_us` 同源，可直接相减算排队时长。
fn qpc_us() -> u64 {
    static FREQ: OnceLock<i64> = OnceLock::new();
    let freq = *FREQ.get_or_init(|| {
        let mut value = 0i64;
        unsafe {
            let _ = QueryPerformanceFrequency(&mut value); // gate-allow R-C-2: 频率查询失败时 value 保持 0，qpc_us 已降级返回 0
        }
        value
    });
    let mut counter = 0i64;
    unsafe {
        let _ = QueryPerformanceCounter(&mut counter); // gate-allow R-C-2: 计数器查询失败时 counter 保持 0，qpc_us 已降级返回 0
    }
    if freq > 0 {
        (counter as u128 * 1_000_000 / freq as u128) as u64
    } else {
        0
    }
}

/// 打开逐帧明细 CSV（`RDC_FRAME_TRACE=1` 时启用，默认关闭）。
///
/// 用途：S7-5 抖动归因。逐迭代记录各段耗时与 pipeline 时延，
/// 让分析脚本能对「超过 30ms 的帧」按段求和找出主导段，而不是只看每秒中位数。
fn open_frame_trace() -> Option<std::fs::File> {
    use std::io::Write;
    if std::env::var("RDC_FRAME_TRACE").ok().as_deref() != Some("1") {
        return None;
    }
    let dir = std::env::var_os("RDC_CONTROL_LOG_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("rdc-control-logs"));
    std::fs::create_dir_all(&dir).ok()?;
    // 每次运行覆盖，避免多次运行的行混在一起。
    let mut file = std::fs::File::create(dir.join("frames-detail.csv")).ok()?;
    let _ = writeln!( // gate-allow R-C-2: 明细 CSV 表头属诊断输出，写失败时后续写入同样失败但仍不影响预览
        file,
        "seq,t_ms,pipeline_us,capture_us,rx_queue_us,push_us,enc_wait_us,decode_us,present_us,residual_us,drops_delta,rx_wait_us,busy_us,publish_us,submit_call_us"
    );
    Some(file)
}

/// 打开逐秒指标 CSV（`RDC_CONTROL_LOG_DIR` 或进程当前目录下的 `rdc-control-logs`）。
fn open_log() -> Option<std::fs::File> {
    use std::io::Write;
    let dir = std::env::var_os("RDC_CONTROL_LOG_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("rdc-control-logs"));
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("frames.csv");
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .ok()?;
    if std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0) == 0 {
        let _ = writeln!( // gate-allow R-C-2: 指标 CSV 表头属诊断输出，写失败不影响预览
            file,
            "elapsed_ms,fps,captured,encoded,decoded,dropped,latency_last_us,latency_p50_us,latency_p99_us,pipeline_latency_us,enc_latency_us,decode_path,capture_us,encode_us,poll_us,decode_us,present_us,decoder_in_flight,encoder_in_flight,encoder_resets,encoder_dropped"
        );
    }
    Some(file)
}

// =====** 像素处理 =====**

/// BGRA ↔ RGBA 原地交换（只给声明 RGBA 输入的后端用，见 `input_is_bgra`）。
///
/// # Arguments
/// * `data` - 整帧 BGRA8 缓冲（原地修改）。
fn swap_rb(data: &mut [u8]) {
    for pixel in data.chunks_exact_mut(4) {
        pixel.swap(0, 2);
    }
}

/// 把采集时刻画成左上角的二进制时间码（21 格：1 格对齐 + 20 位）。
///
/// 画面里保留一块黑底带，保证任意桌面背景下对比度足够；解码后按格心亮度回读。
fn draw_barcode(data: &mut [u8], width: usize, elapsed_ms: u32) {
    let band_left = BARCODE_X0 - 8;
    let band_top = BARCODE_Y0 - 8;
    let band_right = BARCODE_X0 + BARCODE_BITS as usize * BARCODE_PITCH + 8;
    let band_bottom = BARCODE_Y0 + BARCODE_CELL + 8;
    for y in band_top..band_bottom {
        for x in band_left..band_right {
            if x >= width {
                continue;
            }
            let offset = y * width * 4 + x * 4;
            if offset + 3 < data.len() {
                data[offset] = 0;
                data[offset + 1] = 0;
                data[offset + 2] = 0;
                data[offset + 3] = 255;
            }
        }
    }
    for cell in 0..=BARCODE_BITS as usize {
        let on = if cell == 0 {
            true
        } else {
            (elapsed_ms >> (cell - 1)) & 1 == 1
        };
        let x0 = BARCODE_X0 + cell * BARCODE_PITCH;
        for y in BARCODE_Y0..BARCODE_Y0 + BARCODE_CELL {
            for x in x0..x0 + BARCODE_CELL {
                if x >= width {
                    continue;
                }
                let offset = y * width * 4 + x * 4;
                if offset + 3 < data.len() {
                    let value = if on { 255 } else { 0 };
                    data[offset] = value;
                    data[offset + 1] = value;
                    data[offset + 2] = value;
                    data[offset + 3] = 255;
                }
            }
        }
    }
    // 颜色核对色块（块 4 C1）：左纯红、右纯蓝，画在回读区下缘（y 44..50），
    // 不占 barcode 采样行。缓冲是 BGRA 字节序 → 红=(0,0,255)、蓝=(255,0,0)。
    // 若编码链路里还残留 B↔R 交换，回读时会看到左右互换。
    for (px, rgb) in [(12usize, (0u8, 0u8, 255u8)), (26usize, (255u8, 0u8, 0u8))] {
        for y in 44..50 {
            for x in px..px + 8 {
                let offset = y * width * 4 + x * 4;
                if offset + 3 < data.len() {
                    data[offset] = rgb.0;
                    data[offset + 1] = rgb.1;
                    data[offset + 2] = rgb.2;
                    data[offset + 3] = 255;
                }
            }
        }
    }
}

/// NV12 → BGRA（BT.709 limited → full range），用于系统内存解码降级路径。
fn nv12_to_bgra(data: &[u8], stride: usize, width: usize, height: usize, out: &mut [u8]) {
    let y_plane = data;
    let uv_plane = &data[stride * height..];
    for row in 0..height {
        let y_row = row * stride;
        let uv_row = (row / 2) * stride;
        for col in (0..width).step_by(2) {
            let uv_offset = uv_row + col / 2 * 2;
            if uv_offset + 1 >= uv_plane.len() {
                continue;
            }
            let cb = uv_plane[uv_offset] as f32 - 128.0;
            let cr = uv_plane[uv_offset + 1] as f32 - 128.0;
            for sub in 0..2 {
                let x = col + sub;
                if x >= width {
                    break;
                }
                let y = (y_plane[y_row + x] as f32 - 16.0) * (255.0 / 219.0);
                let r = y + 1.5748 * cr;
                let g = y - 0.1873 * cb - 0.4681 * cr;
                let b = y + 1.7902 * cb;
                let offset = (row * width + x) * 4;
                out[offset] = clamp_u8(b);
                out[offset + 1] = clamp_u8(g);
                out[offset + 2] = clamp_u8(r);
                out[offset + 3] = 255;
            }
        }
    }
}

/// 浮点截断到 0..255。
fn clamp_u8(value: f32) -> u8 {
    value.clamp(0.0, 255.0) as u8
}
