// ============================================================
// 文件用途：控制页本机回环预览管线（采集 → H.264 编码 → 解码 → D3D11 共享纹理）
//
// 层级：L4 桥接层的子模块（只在本进程内做真实采集与上屏，不建网络会话）
// 被谁调用：crates/rdc-bridge/src/api.rs 的 control_* 函数
// 边界：不暴露裸指针给 Dart；只把共享纹理句柄与信号事件以 i64 交出，
//       由 runner 的 control_texture.cpp 注册为 Flutter 外部纹理。
// ============================================================

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rdc_capture::{create_source, CaptureConfig, ChromaFormat, Frame, FrameSink};
use rdc_codec::{create_encoder, CodecKind, VideoConfig, VideoChroma};
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
    D3D11_VPOV_DIMENSION_TEXTURE2D, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D,
    ID3D11VideoContext, ID3D11VideoDevice, ID3D11VideoProcessor,
    ID3D11VideoProcessorEnumerator, ID3D11VideoProcessorInputView, ID3D11VideoProcessorOutputView,
};
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};
use windows::Win32::Graphics::Dxgi::IDXGIResource;
use windows::Win32::System::Threading::{CreateEventW, SetEvent};

// =====** 常量 =====**

/// 编码目标帧率（Hz）：与采集同频，限流会把 MFT 固定积压的帧数（约 13 帧）
/// 按 30fps 拉长到 ~430ms，实测画内时延从 ~260ms 升到 ~550ms，故不做编码限流。
const TARGET_FPS: u32 = 60;
/// 主码流码率（bps）：1080p60 桌面文字需要较高码率才能看清。
const TARGET_BITRATE: u32 = 12_000_000;
/// 采集通道容量：只有 2 帧，满了立即丢最旧帧，保证低延迟。
const CHANNEL_CAP: usize = 2;
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

// =====** 对外快照 =====**

/// 控制页预览状态快照。
#[derive(Debug, Clone, Default)]
pub struct ControlSnapshot {
    /// 是否运行。
    pub running: bool,
    /// 采集后端名称。
    pub backend: String,
    /// 编码器名称。
    pub encoder: String,
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
    readback_count: AtomicU64,
    error: Mutex<Option<String>>,
    backend: Mutex<String>,
    encoder: Mutex<String>,
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

    /// 写入字符串型字段。
    fn set_text(slot: &Mutex<String>, value: impl Into<String>) {
        if let Ok(mut guard) = slot.lock() {
            *guard = value.into();
        }
    }

    /// 读取字符串型字段。
    fn text(slot: &Mutex<String>) -> String {
        slot.lock().map(|guard| guard.clone()).unwrap_or_default()
    }
}

// =====** 采集接收 =====**

/// 采集帧接收端：容量极小，满了丢帧而不是阻塞采集线程。
struct ChannelSink {
    tx: SyncSender<Frame>,
    dropped: Arc<AtomicU64>,
}

impl FrameSink for ChannelSink {
    fn on_frame(&self, frame: Frame) {
        if self.tx.try_send(frame).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
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
    let mut slot = state().lock().map_err(|_| "控制状态锁损坏".to_owned())?;
    if let Some(running) = slot.as_ref() {
        if !running.stop.load(Ordering::Relaxed) {
            return Err("控制预览已在运行".to_owned());
        }
        // 上一实例已请求停止但尚未 join：先回收，避免句柄泄漏。
        if let Some(running) = slot.take() {
            let _ = running.join.map(|join| join.join());
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
            Ok((source, rx, parts)) => {
                let _ = setup_tx.send(Ok(parts.info.clone()));
                run_pipeline(source, rx, parts, stats_for_thread, stop_for_thread);
            }
            Err(error) => {
                let _ = setup_tx.send(Err(error));
            }
        })
        .map_err(|error| format!("启动管线线程失败: {error}"))?;

    let info = match setup_rx.recv_timeout(Duration::from_secs(8)) {
        Ok(Ok(info)) => info,
        Ok(Err(error)) => {
            let _ = join.join();
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
    let mut slot = state().lock().map_err(|_| "控制状态锁损坏".to_owned())?;
    let Some(mut running) = slot.take() else {
        return Ok(ControlSnapshot::default());
    };
    running.stop.store(true, Ordering::SeqCst);
    // 采集线程最多 1 秒内退出；join 失败不视为业务错误。
    if let Some(join) = running.join.take() {
        let _ = join.join();
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
    let slot = state().lock().map_err(|_| "控制状态锁损坏".to_owned())?;
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
    decoder: String,
    decode_path: String,
}

/// 管线所需的全部原生资源（只在管线线程内创建、使用与释放）。
struct PipelineParts {
    info: SetupInfo,
    _device: ID3D11Device,
    context: ID3D11DeviceContext,
    textures: [ID3D11Texture2D; 2],
    events: [HANDLE; 2],
    converter: VideoConverter,
    encoder: Box<dyn rdc_codec::VideoEncoder>,
    decoder: Box<dyn VideoDecoder>,
}

/// 在管线线程内创建采集源、D3D11 资源、编码器与解码器，并启动采集。
///
/// # Returns
/// 采集源、接收端与已初始化的管线资源。
///
/// # Errors
/// 任一步失败时返回错误文本；此时已创建的资源随值析构自动回收。
fn setup_pipeline(
    stats: &Arc<Stats>,
) -> Result<(Box<dyn rdc_capture::CaptureSource>, Receiver<Frame>, PipelineParts), String> {
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
            let _ = multithread.SetMultithreadProtected(true);
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

    // 5) 编码器 / 解码器。
    let mut encoder =
        create_encoder(CodecKind::H264).map_err(|error| format!("创建编码器失败: {error}"))?;
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
    eprintln!(
        "[control] decoder={} ({})；输出类型待首帧确定",
        decoder_info.backend, decoder_info.name
    );
    Stats::set_text(&stats.decoder, decoder_name.clone());
    Stats::set_text(&stats.decode_path, decode_path.clone());
    let encoder_name = rdc_codec::probe_encoders()
        .hardware
        .first()
        .map(|capability| capability.name.clone())
        .unwrap_or_else(|| "software-mft".to_owned());
    Stats::set_text(&stats.encoder, encoder_name.clone());

    // 6) 采集通道与启动。
    let (tx, rx) = sync_channel::<Frame>(CHANNEL_CAP);
    source.set_sink(Arc::new(ChannelSink {
        tx,
        dropped: Arc::clone(&stats.dropped),
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
    };
    Ok((source, rx, parts))
}

// =====** 管线线程 =====**

/// 单线程管线：采集 → 编码 → 解码 → 共享纹理 → 通知 Flutter。
fn run_pipeline(
    mut source: Box<dyn rdc_capture::CaptureSource>,
    rx: Receiver<Frame>,
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
        mut encoder,
        mut decoder,
    } = parts;
    let start = Instant::now();
    let mut index = 0usize;
    let mut displayed_window = 0u64;
    let mut bytes_window = 0u64;
    let mut window_start = Instant::now();
    let mut last_readback = Instant::now() - Duration::from_millis(READBACK_INTERVAL_MS);
    let mut latency_samples: Vec<u64> = Vec::with_capacity(64);
    let mut frame_views: HashMap<usize, ID3D11VideoProcessorInputView> = HashMap::new();
    let mut log_file = open_log();
    // 每帧采集时刻（按编码顺序入队，解码输出按顺序出队）。
    let mut pending_capture_ms: std::collections::VecDeque<u32> = std::collections::VecDeque::new();
    // 编码器输出时刻诊断：capture → packet，用来区分编码积压与解码积压。
    let mut pending_packet_ms: std::collections::VecDeque<u32> = std::collections::VecDeque::new();
    let mut enc_latency_us: u64 = 0;
    // 一次送入可能解出 0..N 帧，复用缓冲避免每包分配。
    let mut decoded_frames: Vec<DecodedFrame> = Vec::new();
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let frame = match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(frame) => frame,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            // 采集线程已退出（源停止时发送端被丢弃）。
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };

        let elapsed_ms = start.elapsed().as_millis() as u32 % 1_000_000;
        let mut bgra = frame;
        let (width, height) = (bgra.width, bgra.height);
        if width != converter.width || height != converter.height {
            stats.set_error(format!(
                "采集尺寸变化 {width}x{height}，当前纹理 {}x{}",
                converter.width, converter.height
            ));
            continue;
        }
        stats.captured.fetch_add(1, Ordering::Relaxed);

        // BGRA → RGBA 原地交换，并把时间码画进左上角（编码前）。
        let len = width as usize * height as usize * 4;
        if bgra.buffer.as_slice().len() < len {
            stats.set_error("采集帧缓冲小于整帧长度".to_owned());
            continue;
        }
        let slice = &mut bgra.buffer.as_mut_slice()[..len];
        swap_rb(slice);
        draw_barcode(slice, width as usize, elapsed_ms);
        let pts_ns = bgra.meta.pts_ns;
        let force_key = stats.captured.load(Ordering::Relaxed) == 1;
        if let Err(error) = encoder.push(bgra.buffer, pts_ns, force_key) {
            stats.set_error(format!("编码入队失败: {error}"));
            continue;
        }
        pending_capture_ms.push_back(elapsed_ms);
        pending_packet_ms.push_back(elapsed_ms);

        // 取编码输出并喂给解码器。
        while let Some(packet) = encoder.poll_packet() {
            stats.encoded.fetch_add(1, Ordering::Relaxed);
            let packet_bytes = packet.data.len() as u64;
            stats
                .encoded_bytes
                .fetch_add(packet_bytes, Ordering::Relaxed);
            bytes_window += packet_bytes;
            if let Some(capture_ms) = pending_packet_ms.pop_front() {
                let now_ms = start.elapsed().as_millis() as u64 % 1_000_000;
                let diff = now_ms.wrapping_sub(capture_ms as u64) % 1_000_000;
                if diff < 1_000 {
                    enc_latency_us = diff * 1000;
                }
            }
            if pending_packet_ms.len() > 32 {
                pending_packet_ms.clear();
            }
            decoded_frames.clear();
            let feed_result = decoder.feed_each(
                &packet.data,
                packet.pts_ns / 1000,
                &mut |frame| {
                    decoded_frames.push(frame);
                    Ok(())
                },
            );
            match feed_result {
                Ok(()) => {
                    for decoded in decoded_frames.drain(..) {
                        stats.decoded.fetch_add(1, Ordering::Relaxed);
                        // 编解码管线时延（不含 Flutter 合成），用于与画内回读互相校验。
                        if let Some(capture_ms) = pending_capture_ms.pop_front() {
                            let now_ms =
                                start.elapsed().as_millis() as u64 % 1_000_000;
                            let diff =
                                now_ms.wrapping_sub(capture_ms as u64) % 1_000_000;
                            if diff < 1_000 {
                                stats
                                    .pipeline_latency_us
                                    .store(diff * 1000, Ordering::Relaxed);
                            }
                        }
                        if pending_capture_ms.len() > 32 {
                            pending_capture_ms.clear();
                        }
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
                        if let Err(error) = result {
                            stats.set_error(error);
                            continue;
                        }
                        unsafe {
                            let _ = SetEvent(events[index]);
                        }
                        index ^= 1;
                        stats.displayed.fetch_add(1, Ordering::Relaxed);
                        displayed_window += 1;
                    }
                }
                Err(error) => stats.set_error(format!("解码失败: {error}")),
            }
        }

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
                let _ = writeln!(
                    file,
                    "{},{:.2},{},{},{},{},{},{},{},{},{},{}",
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
                );
            }
            displayed_window = 0;
            bytes_window = 0;
            window_start = Instant::now();
        }

        // 时间码回读：每 500ms 一次，只拷左上角小区域，避免逐帧整帧回读。
        if last_readback.elapsed() >= Duration::from_millis(READBACK_INTERVAL_MS) {
            last_readback = Instant::now();
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
        }
    }

    // 先停采集（它会丢弃发送端，从而让下一次 recv 返回 Disconnected）。
    source.stop();
    drop(source);
    drop(encoder);
    drop(decoder);
    // 纹理随 device 释放；事件句柄必须显式关闭。
    unsafe {
        let _ = CloseHandle(events[0]);
        let _ = CloseHandle(events[1]);
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
    for slot in 0..2 {
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
        handles[slot] = handle.0 as i64;
        textures.push(texture);
    }
    let array: [ID3D11Texture2D; 2] = textures
        .try_into()
        .map_err(|_| "共享纹理数量错误".to_owned())?;
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
        let outputs: [ID3D11VideoProcessorOutputView; 2] = output_views
            .try_into()
            .map_err(|_| "输出视图数量错误".to_owned())?;

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
            let _ = std::mem::ManuallyDrop::take(&mut stream.pInputSurface);
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
        context.Unmap(&converter.readback, 0);
    }
    if aligned {
        Some(value)
    } else {
        None
    }
}

// =====** 日志落盘 =====**

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
        let _ = writeln!(
            file,
            "elapsed_ms,fps,captured,encoded,decoded,dropped,latency_last_us,latency_p50_us,latency_p99_us,pipeline_latency_us,enc_latency_us,decode_path"
        );
    }
    Some(file)
}

// =====** 像素处理 =====**

/// BGRA ↔ RGBA 原地交换（编码器要求 RGBA 输入）。
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
