//! rdc-cli / S3a 采集与编码端到端实测
//!
//! 层级：L4 验证入口（无 UI，仅 Windows）
//! 用途：可选的全屏动态图案窗口作采集夹具 → 以 1080p60 连续采集 10 秒 →
//!       硬件 H.264 编码 → MP4 落盘 → MF 解码回读校验 → 打印/落盘实测报告
//! 调用方：`rdc-cli s3a`（加 `--fixture` 启用夹具）子命令
//!
//! 线程模型（为什么这样分）：
//! - 主线程：只创建窗口、泵窗口消息、跑看门狗。任何条件下都不得长时间阻塞，
//!   否则 Windows 会在 5 秒后把窗口标记为「未响应」并弹框（S3a 实测踩到过）。
//! - worker 线程：探测、采集编排、编码收尾、解码校验、报告落盘。
//! - 采集线程 / 编码线程：由 rdc-capture 与 worker 各自派生，等待一律带超时。
//!
//! 夹具窗口是**可选的一次性测试件**：默认不创建（正常采集流程不出现），
//! 仅在 `--fixture` 时创建，退出时由 [`FixtureWindow`] 的 Drop 自动销毁。

use std::error::Error;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, sync_channel, Receiver, SyncSender, TryRecvError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use rdc_capture::{
    create_source, CaptureConfig, CaptureSnapshot, ChromaFormat, DisplayInfo, Frame, FrameSink,
};
use rdc_codec::{
    create_encoder_selected, extract_sps_pps, probe_encoders, CodecKind, EncodeSnapshot, Mp4Writer,
    VideoConfig,
};

// =====** 类型别名与常量 =====**

/// 实测错误类型（保持与 demo 一致的跨线程传播要求）。
type S3aResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

/// 实测时长（秒）。
const DURATION_SECS: u64 = 10;
/// 目标帧率。
const TARGET_FPS: u32 = 60;
/// 目标码率（bps）。
const TARGET_BITRATE: u32 = 8_000_000;
/// 采集 → 编码通道容量（帧）；通道满即丢帧，避免编码反压采集。
const CHANNEL_CAP: usize = 4;
/// 验收门槛：丢帧率 < 1%。
const DROP_RATE_LIMIT: f64 = 0.01;
/// 验收门槛：硬件编码 p99 ≤ 8ms。
const ENCODE_P99_LIMIT_US: u64 = 8_000;
/// 看门狗告警阈值：任务无进展超过该时长即写日志（窗口仍由主线程泵消息）。
const WATCHDOG_WARN: Duration = Duration::from_millis(2_000);
/// 看门狗硬阈值：超过即放弃等待并退出，绝不允许「未响应」超过 5 秒。
const WATCHDOG_ABORT: Duration = Duration::from_millis(5_000);
/// 单次线程 join 的上限：超时记日志并放弃等待，由进程退出回收线程。
const JOIN_TIMEOUT: Duration = Duration::from_millis(3_000);
/// 采集期心跳间隔（worker 侧），用于看门狗判断「有没有进展」。
const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(20);

// =====** FrameSink 实现 =====**

/// 把采集帧送入有界通道；通道满时静默丢帧并计数。
struct ChannelSink {
    /// 有界发送端。
    tx: SyncSender<Frame>,
    /// 因通道满丢弃的帧数。
    dropped: Arc<AtomicU64>,
    /// 逐帧原始日志（`logs/`，可缺省）。
    log: Option<Arc<FrameLog>>,
}

impl FrameSink for ChannelSink {
    /// 尝试投递一帧。
    ///
    /// # Arguments
    /// * `frame` - 采集帧；投递失败时在此 drop，缓冲归还内存池。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败；丢弃通过 `dropped` 原子计数上报。
    fn on_frame(&self, frame: Frame) {
        // 先取元数据快照：`frame` 会在 try_send 里被移动。
        let frame_id = frame.meta.frame_id;
        let pts_ns = frame.meta.pts_ns;
        let interval_us = frame.interval_us;
        let capture_cost_us = frame.capture_cost_us;
        let dirty = frame.dirty_rects.len();
        let moves = frame.move_rects.len();
        let accepted = self.tx.try_send(frame).is_ok();
        if !accepted {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        if let Some(log) = self.log.as_ref() {
            log.row(&format!(
                "{frame_id},{pts_ns},{interval_us},{capture_cost_us},{dirty},{moves},{}",
                if accepted { "ok" } else { "channel_full" }
            ));
        }
    }
}

// =====** 原始数据日志 =====**

/// 逐帧原始数据日志（CSV，落 `logs/`；该目录已入 `.gitignore`）。
struct FrameLog {
    /// 串行化写入器：采集线程与编码线程都可能写同一目录的不同文件。
    inner: std::sync::Mutex<std::io::BufWriter<std::fs::File>>,
}

impl FrameLog {
    /// 创建日志文件并写入表头。
    ///
    /// # Arguments
    /// * `path` - 目标文件路径。
    /// * `header` - CSV 表头。
    ///
    /// # Returns
    /// 可共享的写入句柄。
    ///
    /// # Errors
    /// 目录或文件创建失败时返回 IO 错误（由调用方决定是否降级）。
    fn create(path: &Path, header: &str) -> std::io::Result<Arc<Self>> {
        let file = std::fs::File::create(path)?;
        let mut writer = std::io::BufWriter::new(file);
        writeln!(writer, "{header}")?;
        Ok(Arc::new(Self {
            inner: std::sync::Mutex::new(writer),
        }))
    }

    /// 追加一行。
    ///
    /// # Arguments
    /// * `line` - 不含换行的 CSV 行。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 写失败被忽略：日志是诊断数据，不得影响实测主流程。
    fn row(&self, line: &str) {
        if let Ok(mut writer) = self.inner.lock() {
            let _ = writeln!(writer, "{line}");
        }
    }

    /// 冲刷缓冲（收尾时调用，保证日志完整）。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 冲刷失败被忽略。
    fn flush(&self) {
        if let Ok(mut writer) = self.inner.lock() {
            let _ = writer.flush();
        }
    }
}

/// 创建本次实测的日志目录 `logs/s3a-<epoch 秒>`。
///
/// # Returns
/// 目录路径；创建失败时返回路径但后续写日志会自动降级为无输出。
///
/// # Errors
/// 不会失败。
fn create_log_dir() -> PathBuf {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let dir = PathBuf::from("logs").join(format!("s3a-{secs}"));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

// =====** 编码线程结果 =====**

/// 编码线程产出。
struct EncodeOutcome {
    /// 编码后的帧数（成功写入 MP4 的样本数）。
    encoded_frames: u64,
    /// 池化缓冲上的通道丢帧数（由采集层统计，这里复制一份便于报告）。
    channel_dropped: u64,
    /// 编码器统计快照。
    snapshot: EncodeSnapshot,
    /// 实际后端名称。
    backend: String,
    /// 实际选中的后端是否为硬件（来自 `create_encoder_selected`，不取探测列表推断）。
    hardware: bool,
}

// =====** 主线程：消息泵与看门狗 =====**

/// 进程级退出请求（Ctrl+C、关窗、看门狗）。
static STOP_REQUESTED: AtomicBool = AtomicBool::new(false);

/// 任务进度心跳：worker 每个阶段和每帧都记录一次活动时刻。
struct Progress {
    /// 计时起点。
    start: Instant,
    /// 最近一次活动时刻（自起点起的毫秒）。
    last_ms: AtomicU64,
}

impl Progress {
    /// 创建心跳。
    ///
    /// # Returns
    /// 起点为当前时刻的心跳对象。
    ///
    /// # Errors
    /// 不会失败。
    fn new() -> Self {
        Self {
            start: Instant::now(),
            last_ms: AtomicU64::new(0),
        }
    }

    /// 记录一次活动。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    fn touch(&self) {
        self.last_ms
            .store(self.start.elapsed().as_millis() as u64, Ordering::Relaxed);
    }

    /// 距上次活动的时长。
    ///
    /// # Returns
    /// 无进展时长；从未活动时为自起点起的全部时长。
    ///
    /// # Errors
    /// 不会失败。
    fn idle(&self) -> Duration {
        let now = self.start.elapsed().as_millis() as u64;
        Duration::from_millis(now.saturating_sub(self.last_ms.load(Ordering::Relaxed)))
    }
}

/// 主线程等待结果。
enum PumpOutcome {
    /// worker 已结束，附带其结果。
    Worker(Result<(), String>),
    /// worker 卡住超过硬阈值（附卡住时长）。
    Stalled(Duration),
    /// 收到退出请求。
    Aborted,
}

/// 执行 S3a 端到端实测。
///
/// # Arguments
/// * `fixture_enabled` - 是否创建全屏夹具窗口（`--fixture` 时为 true）。
///
/// # Returns
/// 全部步骤完成且达标返回 `Ok(())`；失败或未达门槛返回带描述的错误。
///
/// # Errors
/// 任务卡住、收到退出请求、采集/编码/校验失败或未达验收门槛时返回错误。
pub fn run(fixture_enabled: bool) -> S3aResult<()> {
    let started = Instant::now();
    STOP_REQUESTED.store(false, Ordering::SeqCst);
    println!(
        "== RDC S3a 采集与编码实测（{} s，1080p{}）==",
        DURATION_SECS, TARGET_FPS
    );
    let log_dir = create_log_dir();
    println!("日志目录             : {}", log_dir.display());

    // 夹具是可选的／一次性测试件：默认不创建，正常采集流程里不出现。
    let fixture = if fixture_enabled {
        let window = FixtureWindow::create()?;
        // 先让桌面产生稳定更新，再开始计时采集。
        window.pump(Duration::from_millis(250));
        println!("采集夹具             : 已创建（一次性测试件，退出时自动销毁）");
        Some(window)
    } else {
        println!("采集夹具             : 未启用（需桌面自行产生画面变化）");
        None
    };
    install_ctrl_handler();

    // worker 承担全部耗时工作；主线程只泵消息 + 看门狗，任何时刻都不阻塞。
    let progress = Arc::new(Progress::new());
    progress.touch();
    let (done_tx, done_rx) = channel::<Result<(), String>>();
    let worker_progress = Arc::clone(&progress);
    let worker_log_dir = log_dir.clone();
    let worker = thread::Builder::new()
        .name("rdc-s3a-worker".to_owned())
        .spawn(move || {
            let result = run_pipeline(&worker_progress, &worker_log_dir).map_err(|e| e.to_string());
            let _ = done_tx.send(result);
        })?;

    // 回归探针：模拟「主线程长时间不泵消息」的旧行为，用于验证未响应判定。
    // 仅诊断时设置 RDC_S3A_MAIN_BLOCK_MS（毫秒），默认不生效。
    if let Ok(raw) = std::env::var("RDC_S3A_MAIN_BLOCK_MS") {
        if let Ok(ms) = raw.parse::<u64>() {
            eprintln!("[repro] 主线程故意阻塞 {ms} ms 不泵消息（复现未响应）");
            thread::sleep(Duration::from_millis(ms));
        }
    }

    let outcome = pump_until_done(&done_rx, &progress);
    // 退出前必须销毁夹具窗口，避免留下空白窄窗。
    if let Some(window) = fixture.as_ref() {
        window.close();
    }
    drop(fixture);

    match outcome {
        PumpOutcome::Worker(result) => {
            if join_with_timeout(worker, JOIN_TIMEOUT, &progress).is_none() {
                eprintln!("[watchdog] worker 已发出结果但 3 s 内未退出，放弃等待");
            }
            if let Err(msg) = result {
                return Err(msg.into());
            }
        }
        PumpOutcome::Stalled(idle) => {
            return Err(format!("任务卡住 {idle:?} 无进展，看门狗中止（窗口已销毁）").into());
        }
        PumpOutcome::Aborted => {
            return Err("收到退出请求（Ctrl+C / 关窗），已中止实测".into());
        }
    }
    println!("总耗时 {} ms", started.elapsed().as_millis());
    Ok(())
}

/// 泵消息直到 worker 结束、卡住或收到退出请求。
///
/// # Arguments
/// * `done` - worker 结果通道。
/// * `progress` - worker 心跳。
///
/// # Returns
/// 结束原因（[`PumpOutcome`]）。
///
/// # Errors
/// 不会失败；异常通过返回值表达。
fn pump_until_done(done: &Receiver<Result<(), String>>, progress: &Progress) -> PumpOutcome {
    let mut warned_secs = 0u64;
    loop {
        pump_messages();
        match done.try_recv() {
            Ok(result) => return PumpOutcome::Worker(result),
            Err(TryRecvError::Empty) => {}
            Err(TryRecvError::Disconnected) => {
                return PumpOutcome::Worker(Err("worker 线程异常结束（未返回结果）".to_owned()))
            }
        }
        if STOP_REQUESTED.load(Ordering::SeqCst) {
            return PumpOutcome::Aborted;
        }
        let idle = progress.idle();
        if idle >= WATCHDOG_ABORT {
            eprintln!("[watchdog] worker 已 {idle:?} 无进展，超过硬阈值，放弃等待并退出");
            return PumpOutcome::Stalled(idle);
        }
        if idle >= WATCHDOG_WARN && idle.as_secs() != warned_secs {
            warned_secs = idle.as_secs();
            eprintln!("[watchdog] worker 已 {idle:?} 无进展（窗口仍可响应），继续等待");
        }
        // 8ms 节流：既保证消息及时处理，又不空转抢占采集 CPU。
        thread::sleep(Duration::from_millis(8));
    }
}

/// 非阻塞泵一轮窗口消息。
///
/// # Returns
/// 无返回值。
///
/// # Errors
/// 不会失败。
fn pump_messages() {
    #[cfg(target_os = "windows")]
    unsafe {
        use windows::Win32::UI::WindowsAndMessaging::{
            DispatchMessageW, PeekMessageW, TranslateMessage, MSG, PM_REMOVE,
        };
        let mut msg = MSG::default();
        while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

/// 带超时 join：卡住就放弃等待并记日志，由进程退出回收线程。
///
/// # Arguments
/// * `handle` - 目标线程。
/// * `timeout` - 最长等待时长。
/// * `progress` - 心跳（等待期间维持 worker 活跃标记）。
///
/// # Returns
/// 线程结束返回其结果；超时返回 `None`。
///
/// # Errors
/// 不会失败；线程 panic 也折算为 `None`（已记日志）。
fn join_with_timeout<T>(
    handle: thread::JoinHandle<T>,
    timeout: Duration,
    progress: &Progress,
) -> Option<T> {
    let deadline = Instant::now() + timeout;
    while !handle.is_finished() {
        if Instant::now() >= deadline {
            return None;
        }
        progress.touch();
        thread::sleep(Duration::from_millis(10));
    }
    match handle.join() {
        Ok(value) => Some(value),
        Err(_) => {
            eprintln!("[watchdog] 目标线程 panic，结果不可用");
            None
        }
    }
}

/// 注册 Ctrl+C / 关控制台的处理：只置位退出请求，不做清理。
///
/// # Returns
/// 无返回值。
///
/// # Errors
/// 注册失败只记日志，不影响主流程。
fn install_ctrl_handler() {
    #[cfg(target_os = "windows")]
    unsafe {
        use windows::Win32::System::Console::{SetConsoleCtrlHandler, PHANDLER_ROUTINE};
        // 处理函数只置原子位，避免在系统回调里做任何可能阻塞的操作。
        unsafe extern "system" fn on_ctrl(_kind: u32) -> windows::Win32::Foundation::BOOL {
            STOP_REQUESTED.store(true, Ordering::SeqCst);
            windows::Win32::Foundation::BOOL(1)
        }
        let handler: PHANDLER_ROUTINE = Some(on_ctrl);
        if SetConsoleCtrlHandler(handler, true).is_err() {
            eprintln!("[s3a] 注册 Ctrl+C 处理失败，Ctrl+C 将走系统默认终止");
        }
    }
}

// =====** worker：实测流水线 =====**

/// 在 worker 线程执行完整实测流程。
///
/// # Arguments
/// * `progress` - 心跳句柄，供主线程看门狗判断是否卡住。
///
/// # Returns
/// 达标返回 `Ok(())`，否则返回带描述的错误。
///
/// # Errors
/// 无硬件编码器、采集/编码失败、解码校验失败或未达门槛时返回错误。
fn run_pipeline(progress: &Progress, log_dir: &Path) -> S3aResult<()> {
    // 逐帧原始数据：采集侧由 ChannelSink 写，编码侧由编码线程写（失败即降级为无日志）。
    let capture_log = FrameLog::create(
        &log_dir.join("capture-frames.csv"),
        "frame_id,pts_ns,interval_us,capture_cost_us,dirty_rects,move_rects,status",
    )
    .ok();
    let encode_log = FrameLog::create(
        &log_dir.join("encode-frames.csv"),
        "index,pts_ns,bytes,keyframe,cost_us",
    )
    .ok();

    // 1) 编码器能力探测：先证明「有没有硬件」，再决定是否继续。
    progress.touch();
    let probe = probe_encoders();
    print_probe(&probe);
    if !probe.hardware_available {
        return Err("未检测到硬件 H.264 编码器；按 S3a 约束不做软编回退".into());
    }

    // 2) 采集源与显示器。
    progress.touch();
    let mut source = create_source()?;
    let displays = source.enumerate();
    if displays.is_empty() {
        return Err("未枚举到任何显示器".into());
    }
    let display = pick_display(&displays);
    println!(
        "显示器 #{} {}x{}@{}Hz{}",
        display.id,
        display.width,
        display.height,
        display.refresh_hz,
        if display.is_primary { "（主）" } else { "" }
    );
    if display.width != 1920 || display.height != 1080 {
        println!(
            "注意：当前显示器为 {}x{}，不是标准的 1920x1080；验收口径按实际分辨率记录",
            display.width, display.height
        );
    }

    // 3) 输出路径。
    let out_dir = PathBuf::from("target").join("s3a");
    std::fs::create_dir_all(&out_dir)?;
    let mp4_path = out_dir.join("s3a-1080p60.mp4");
    let report_path = out_dir.join("s3a-report.json");

    // 4) 采集 → 编码流水线。
    let (tx, rx) = sync_channel::<Frame>(CHANNEL_CAP);
    let channel_dropped = Arc::new(AtomicU64::new(0));
    source.set_sink(Arc::new(ChannelSink {
        tx,
        dropped: Arc::clone(&channel_dropped),
        log: capture_log,
    }));
    let config = CaptureConfig {
        display_id: display.id,
        target_fps: TARGET_FPS,
        include_cursor: false,
        chroma: ChromaFormat::Bgra,
        bit_depth: 8,
    };
    source.start(config)?;
    let capture_backend = source.backend_name();
    println!("采集后端: {capture_backend}");

    let (width, height) = (display.width, display.height);
    let mp4_path_for_thread = mp4_path.clone();
    let encode_thread = thread::Builder::new()
        .name("rdc-s3a-encode".to_owned())
        .spawn(move || {
            encode_loop(
                rx,
                &mp4_path_for_thread,
                width,
                height,
                channel_dropped,
                encode_log,
            )
        })?;

    // 5) 采集期：worker 只做计时与心跳；窗口消息由主线程负责。
    let record_start = Instant::now();
    while record_start.elapsed() < Duration::from_secs(DURATION_SECS) {
        if STOP_REQUESTED.load(Ordering::SeqCst) {
            break;
        }
        progress.touch();
        thread::sleep(HEARTBEAT_INTERVAL);
    }
    let elapsed = record_start.elapsed();
    progress.touch();

    // 6) 收尾：先停采集线程，再释放采集源（它持有帧通道发送端），最后等编码线程。
    source.stop();
    let capture = source.stats();
    drop(source);
    progress.touch();
    let outcome = join_with_timeout(encode_thread, JOIN_TIMEOUT, progress)
        .ok_or("编码线程在超时内未结束，已放弃等待")??;

    // 7) 解码回读验证。
    progress.touch();
    let verify = rdc_codec::verify_mp4(&mp4_path)?;
    let file_bytes = std::fs::metadata(&mp4_path).map(|m| m.len()).unwrap_or(0);

    // 8) 报告。
    progress.touch();
    let report = build_report(
        &capture,
        &outcome,
        &verify,
        file_bytes,
        elapsed,
        capture_backend,
        &mp4_path,
        width,
        height,
        outcome.hardware,
    );
    println!("{}", report.text);
    std::fs::write(&report_path, &report.json)?;
    // 报告副本进日志目录，便于与逐帧原始数据对照回查。
    let _ = std::fs::copy(&report_path, log_dir.join("report.json"));
    println!("MP4: {}", mp4_path.display());
    println!("报告: {}", report_path.display());

    if report.passed {
        Ok(())
    } else {
        Err(format!(
            "验收未通过：采集掉帧率 {:.3}%（门槛 <{:.1}%），编码 p99 {} us（门槛 ≤{} us）",
            capture.drop_rate * 100.0,
            DROP_RATE_LIMIT * 100.0,
            outcome.snapshot.cost_p99_us,
            ENCODE_P99_LIMIT_US
        )
        .into())
    }
}

// =====** 编码线程 =====**

/// 编码循环：逐帧转 RGBA → 编码 → 写 MP4。
///
/// # Arguments
/// * `rx` - 采集帧接收端。
/// * `mp4_path` - 输出文件路径。
/// * `width` / `height` - 帧尺寸。
/// * `channel_dropped` - 通道丢帧计数（共享原子量）。
///
/// # Returns
/// 编码统计与后端信息。
///
/// # Errors
/// 编码器创建/配置、MP4 创建或写样本失败时返回错误。
fn encode_loop(
    rx: Receiver<Frame>,
    mp4_path: &std::path::Path,
    width: u32,
    height: u32,
    channel_dropped: Arc<AtomicU64>,
    log: Option<Arc<FrameLog>>,
) -> S3aResult<EncodeOutcome> {
    // 用带选择结果入口创建编码器：`hardware` 必须来自实际选中的后端，
    // 而不是 `probe_encoders()` 的「列表里有没有硬件」。
    let selection = create_encoder_selected(CodecKind::H264)
        .map_err(|e| format!("创建编码器失败: {e}"))?;
    let hardware = selection.hardware;
    let mut encoder = selection.encoder;
    encoder
        .configure(VideoConfig {
            codec: CodecKind::H264,
            width,
            height,
            fps: TARGET_FPS,
            bitrate_bps: TARGET_BITRATE,
            chroma: rdc_codec::VideoChroma::Yuv420,
            bit_depth: 8,
            low_latency: true,
        })
        .map_err(|e| format!("配置编码器失败: {e}"))?;

    let mut writer: Option<Mp4Writer> = None;
    // 文件时间戳从 0 开始（QPC 绝对时间对容器无意义，且便于帧率校验）。
    let mut pts_base = 0u64;
    let nominal_100ns = 10_000_000i64 / i64::from(TARGET_FPS);
    // 延后一包落盘：拿到下一包时间戳后才能给出不重叠的样本时长，
    // 否则按时长固定 1/60 s 会与稀疏帧的实际间隔冲突，MP4 sink 会丢样本。
    let mut pending: Option<(Vec<u8>, i64)> = None;
    let mut encoded = 0u64;
    // 逐包序号（写入 encode-frames.csv）。
    let mut packet_index = 0u64;

    while let Ok(mut frame) = rx.recv() {
        // BGRA → RGBA 只交换 R/B 通道，原地做即可，不发生整帧拷贝。
        swap_rb(&mut frame.buffer.as_mut_slice()[..(width as usize) * (height as usize) * 4]);
        let pts_ns = frame.meta.pts_ns;
        let push_start = Instant::now();
        encoder
            .push(frame.buffer, pts_ns, false)
            .map_err(|e| format!("提交编码失败: {e}"))?;
        let push_cost_us = push_start.elapsed().as_micros() as u64;
        while let Some(packet) = encoder.poll_packet() {
            if let Some(log) = log.as_ref() {
                log.row(&format!(
                    "{packet_index},{},{},{},{}",
                    packet.pts_ns,
                    packet.data.len(),
                    u8::from(packet.is_keyframe),
                    push_cost_us
                ));
            }
            packet_index += 1;
            // 首个包才创建写入器：此时编码器已产出（或可从 IDR 中提取）SPS/PPS，
            // 直通封装的输入类型必须携带序列头。
            if writer.is_none() {
                let mut header = encoder.sequence_header().to_vec();
                if header.is_empty() {
                    header = extract_sps_pps(&packet.data);
                }
                pts_base = packet.pts_ns;
                writer = Some(
                    Mp4Writer::create(
                        mp4_path,
                        width,
                        height,
                        TARGET_FPS,
                        TARGET_BITRATE,
                        &header,
                    )
                    .map_err(|e| format!("创建 MP4 写入器失败: {e}"))?,
                );
            }
            let rel = packet.pts_ns.saturating_sub(pts_base) as i64 / 100;
            if let Some((data, prev)) = pending.take() {
                let duration = (rel - prev).max(1);
                writer
                    .as_mut()
                    .expect("writer 已在首个包时创建")
                    .write(&data, prev, duration)
                    .map_err(|e| format!("写 MP4 失败: {e}"))?;
                encoded += 1;
            }
            pending = Some((packet.data, rel));
        }
    }

    let Some(mut writer) = writer else {
        return Err("采集期间没有产出任何可写入的码流包".into());
    };

    // 冲刷尾部：异步 MFT 会滞留 1~2 帧。
    let tail = encoder.flush().map_err(|e| format!("冲刷编码器失败: {e}"))?;
    let mut tail: Vec<rdc_codec::EncodedPacket> = tail;
    while let Some(packet) = encoder.poll_packet() {
        tail.push(packet);
    }
    for packet in tail {
        let rel = packet.pts_ns.saturating_sub(pts_base) as i64 / 100;
        if let Some((data, prev)) = pending.take() {
            let duration = (rel - prev).max(1);
            writer.write(&data, prev, duration)?;
            encoded += 1;
        }
        pending = Some((packet.data, rel));
    }
    // 最后一包没有后继时间戳，按标称帧时长收尾。
    if let Some((data, prev)) = pending.take() {
        writer.write(&data, prev, nominal_100ns)?;
        encoded += 1;
    }
    writer.finish()?;
    if let Some(log) = log.as_ref() {
        log.flush();
    }

    let snapshot = encoder
        .stats_snapshot()
        .ok_or("编码器未提供统计快照")?;
    let backend = encoder.backend_name().to_owned();
    Ok(EncodeOutcome {
        encoded_frames: encoded,
        channel_dropped: channel_dropped.load(Ordering::Relaxed),
        snapshot,
        backend,
        hardware,
    })
}

/// 原地交换每个像素的 R 与 B 通道（BGRA → RGBA）。
///
/// # Arguments
/// * `buf` - 像素缓冲，长度必须是 4 的倍数。
///
/// # Returns
/// 无返回值。
///
/// # Errors
/// 不会失败。
fn swap_rb(buf: &mut [u8]) {
    for px in buf.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
}

// =====** 报告 =====**

/// 报告结构与落盘内容。
struct Report {
    /// 人类可读文本。
    text: String,
    /// JSON 文本。
    json: String,
    /// 是否满足验收门槛。
    passed: bool,
}

/// 组装实测报告。
#[allow(clippy::too_many_arguments)]
fn build_report(
    capture: &CaptureSnapshot,
    encode: &EncodeOutcome,
    verify: &rdc_codec::VerifyReport,
    file_bytes: u64,
    elapsed: Duration,
    capture_backend: &str,
    mp4_path: &std::path::Path,
    width: u32,
    height: u32,
    hardware: bool,
) -> Report {
    let elapsed_ms = elapsed.as_millis().max(1) as u64;
    let drop_rate_pct = capture.drop_rate * 100.0;
    let avg_bitrate_bps = encode.snapshot.avg_bitrate_bps;
    let decode_ok = verify.ok;
    // 分辨率必须与请求一致；帧率允许 ±1 fps 漂移（时间戳抖动）。
    let resolution_ok = verify.width == width && verify.height == height;
    let fps_ok = (verify.measured_fps() - f64::from(TARGET_FPS)).abs() <= 1.0;
    let drop_ok = capture.drop_rate < DROP_RATE_LIMIT;
    let p99_ok = encode.snapshot.cost_p99_us <= ENCODE_P99_LIMIT_US;
    let passed = drop_ok && p99_ok && decode_ok && resolution_ok && fps_ok;

    let text = format!(
        "\n== S3a 实测结果 ==\n\
         采集路径             : {capture_backend}\n\
         编码路径             : {}（硬件={hardware}）\n\
         采集时长             : {:.2} s（目标 {} s）\n\
         采集交付帧数         : {}\n\
         采集丢帧数           : {}（超时 {} 次，池耗尽 {} 次）\n\
         采集掉帧率           : {drop_rate_pct:.3}%（门槛 <{:.1}%）\n\
         实测采集帧率         : {:.2} fps\n\
         帧间隔 p50 / p99      : {} / {} us\n\
         采集耗时 p50 / p99    : {} / {} us\n\
         编码帧数             : {}\n\
         关键帧数             : {}\n\
         编码总字节           : {}\n\
         平均码率             : {:.3} Mbps（目标 {:.1} Mbps）\n\
         编码耗时 p50 / p99    : {} / {} us（门槛 ≤{} us）\n\
         编码时延 p50 / p99    : {} / {} us\n\
         通道溢出丢帧         : {}\n\
         MP4 文件大小         : {} KB\n\
         解码验证             : {}（{}x{}，声明 {}/{} fps，实测 {:.2} fps，解码 {} 帧，失败 {} 帧）\n\
         判定                 : {}",
        encode.backend,
        elapsed_ms as f64 / 1000.0,
        DURATION_SECS,
        capture.captured,
        capture.dropped,
        capture.timeouts,
        capture.pool_exhausted,
        DROP_RATE_LIMIT * 100.0,
        capture.fps,
        capture.interval_p50_us,
        capture.interval_p99_us,
        capture.cost_p50_us,
        capture.cost_p99_us,
        encode.encoded_frames,
        encode.snapshot.keyframes,
        encode.snapshot.bytes,
        avg_bitrate_bps as f64 / 1_000_000.0,
        TARGET_BITRATE as f64 / 1_000_000.0,
        encode.snapshot.cost_p50_us,
        encode.snapshot.cost_p99_us,
        ENCODE_P99_LIMIT_US,
        encode.snapshot.latency_p50_us,
        encode.snapshot.latency_p99_us,
        encode.channel_dropped,
        file_bytes / 1024,
        if verify.ok { "通过" } else { "失败" },
        verify.width,
        verify.height,
        verify.fps_num,
        verify.fps_den,
        verify.measured_fps(),
        verify.decoded_frames,
        verify.failed_frames,
        if passed { "PASS" } else { "FAIL" },
    );

    let json = format!(
        "{{\n\
         \x20 \"phase\": \"S3a\",\n\
         \x20 \"capture_backend\": \"{capture_backend}\",\n\
         \x20 \"encoder_backend\": \"{}\",\n\
         \x20 \"hardware\": {hardware},\n\
         \x20 \"duration_ms\": {elapsed_ms},\n\
         \x20 \"capture\": {{\"captured\": {}, \"dropped\": {}, \"timeouts\": {}, \"pool_exhausted\": {}, \"drop_rate\": {:.6}, \"fps\": {:.3}, \"interval_p50_us\": {}, \"interval_p99_us\": {}, \"cost_p50_us\": {}, \"cost_p99_us\": {}}},\n\
         \x20 \"encode\": {{\"frames\": {}, \"keyframes\": {}, \"bytes\": {}, \"avg_bitrate_bps\": {:.1}, \"cost_p50_us\": {}, \"cost_p99_us\": {}, \"latency_p50_us\": {}, \"latency_p99_us\": {}}},\n\
         \x20 \"pipeline_dropped\": {},\n\
         \x20 \"file\": {{\"path\": \"{}\", \"bytes\": {file_bytes}}},\n\
         \x20 \"verify\": {{\"ok\": {}, \"width\": {}, \"height\": {}, \"fps_num\": {}, \"fps_den\": {}, \"measured_fps\": {:.3}, \"decoded_frames\": {}, \"failed_frames\": {}, \"note\": \"{}\"}},\n\
         \x20 \"gate\": {{\"drop_rate_limit\": {DROP_RATE_LIMIT}, \"drop_ok\": {drop_ok}, \"encode_p99_limit_us\": {ENCODE_P99_LIMIT_US}, \"encode_p99_ok\": {p99_ok}, \"resolution_ok\": {resolution_ok}, \"fps_ok\": {fps_ok}, \"decode_ok\": {decode_ok}, \"passed\": {passed}}}\n\
         }}",
        encode.backend,
        capture.captured,
        capture.dropped,
        capture.timeouts,
        capture.pool_exhausted,
        capture.drop_rate,
        capture.fps,
        capture.interval_p50_us,
        capture.interval_p99_us,
        capture.cost_p50_us,
        capture.cost_p99_us,
        encode.encoded_frames,
        encode.snapshot.keyframes,
        encode.snapshot.bytes,
        avg_bitrate_bps,
        encode.snapshot.cost_p50_us,
        encode.snapshot.cost_p99_us,
        encode.snapshot.latency_p50_us,
        encode.snapshot.latency_p99_us,
        encode.channel_dropped,
        mp4_path.display().to_string().replace('\\', "/"),
        verify.ok,
        verify.width,
        verify.height,
        verify.fps_num,
        verify.fps_den,
        verify.measured_fps(),
        verify.decoded_frames,
        verify.failed_frames,
        verify.note,
    );
    Report { text, json, passed }
}

// =====** 探测报告打印 =====**

/// 打印编码器探测结果。
///
/// # Arguments
/// * `report` - 探测报告。
///
/// # Returns
/// 无返回值。
///
/// # Errors
/// 不会失败。
fn print_probe(report: &rdc_codec::ProbeReport) {
    println!(
        "MFT 硬件编码器可用: {}（仅枚举结果，不代表实际选中后端；实际以后续 encoder.selected 为准）",
        report.hardware_available
    );
    for cap in &report.hardware {
        println!(
            "  [HW] {} | H.264 | {}x{}@{} | {:?}/{}bit | {}-{} Mbps | 热更新={} | {}",
            cap.name,
            cap.max_width,
            cap.max_height,
            cap.max_fps,
            cap.chroma,
            cap.bit_depth,
            cap.min_bitrate_bps / 1_000_000,
            cap.max_bitrate_bps / 1_000_000,
            cap.runtime_reconfigure,
            cap.note
        );
    }
    for cap in &report.software {
        println!(
            "  [SW] {} | {}x{}@{}（仅列出，不参与编码）",
            cap.name, cap.max_width, cap.max_height, cap.max_fps
        );
    }
    if let Some(err) = &report.error {
        println!("  探测错误: {err}");
    }
}

/// 选择主显示器；没有主显示器标记时取第一个。
///
/// # Arguments
/// * `displays` - 显示器列表。
///
/// # Returns
/// 选中的显示器。
///
/// # Errors
/// 调用方已保证列表非空。
fn pick_display(displays: &[DisplayInfo]) -> DisplayInfo {
    displays
        .iter()
        .find(|d| d.is_primary)
        .cloned()
        .unwrap_or_else(|| displays[0].clone())
}

// =====** 全屏动态夹具窗口 =====**

/// 动态图案窗口：定时重绘整屏彩条 + 移动白块。
#[cfg(target_os = "windows")]
pub struct FixtureWindow {
    /// 窗口句柄。
    hwnd: HWND,
}

#[cfg(not(target_os = "windows"))]
pub struct FixtureWindow;

#[cfg(not(target_os = "windows"))]
impl FixtureWindow {
    /// 非 Windows 平台不提供夹具。
    pub fn create() -> S3aResult<Self> {
        Err("S3a 实测夹具只实现于 Windows".into())
    }

    /// 空转。
    pub fn pump(&self, _duration: Duration) {}

    /// 空转（非 Windows 无窗口）。无返回值。
    pub fn close(&self) {}
}

#[cfg(target_os = "windows")]
impl FixtureWindow {
    /// 创建全屏置顶无边框窗口并启动 16ms 定时器。
    ///
    /// # Returns
    /// 窗口句柄包装。
    ///
    /// # Errors
    /// 注册窗口类或创建窗口失败时返回错误。
    pub fn create() -> S3aResult<Self> {
        use windows::Win32::Foundation::HINSTANCE;
        use windows::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, RegisterClassW, SetTimer, ShowWindow, SW_SHOWNOACTIVATE, WNDCLASSW,
            WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
        };
        unsafe {
            let hinstance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)?;
            let class_name = windows::core::HSTRING::from("rdc-s3a-fixture");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(wndproc),
                hInstance: HINSTANCE::from(hinstance),
                lpszClassName: windows::core::PCWSTR(class_name.as_ptr()),
                ..Default::default()
            };
            let atom = RegisterClassW(&wc);
            if atom == 0 {
                // 已注册过同名窗口类时（同进程重复调用）可以继续。
                let err = windows::Win32::Foundation::GetLastError();
                if err.0 != 1410 {
                    return Err(format!("RegisterClassW 失败: {}", err.0).into());
                }
            }
            // 以主显示器分辨率为窗口尺寸（进程已设为 Per-Monitor V2，坐标即物理像素）。
            let (screen_w, screen_h) = screen_size();
            let hwnd = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                &class_name,
                &windows::core::HSTRING::from("RDC S3a 采集夹具"),
                WS_POPUP | WS_VISIBLE,
                0,
                0,
                screen_w,
                screen_h,
                None,
                None,
                HINSTANCE::from(hinstance),
                None,
            );
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let _ = SetTimer(hwnd, 1, 16, None);
            Ok(Self { hwnd })
        }
    }

    /// 泵消息 `duration` 时长。
    ///
    /// # Arguments
    /// * `duration` - 泵消息持续时间。
    ///
    /// # Returns
    /// 无返回值；收到退出请求时提前返回。
    ///
    /// # Errors
    /// 不会失败。
    pub fn pump(&self, duration: Duration) {
        let deadline = Instant::now() + duration;
        while Instant::now() < deadline {
            pump_messages();
            if STOP_REQUESTED.load(Ordering::SeqCst) {
                return;
            }
            // 1ms 节流，避免空转占满 CPU 影响采集计时。
            thread::sleep(Duration::from_millis(1));
        }
    }

    /// 主动销毁窗口（退出路径必调，避免留下空白窄窗）。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败；重复调用无害。
    pub fn close(&self) {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::DestroyWindow(self.hwnd);
        }
    }
}

#[cfg(target_os = "windows")]
impl Drop for FixtureWindow {
    /// 兜底销毁窗口（任何退出路径都不会残留窗口）。
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(target_os = "windows")]
use windows::Win32::Foundation::HWND;

/// 屏幕物理尺寸。
#[cfg(target_os = "windows")]
fn screen_size() -> (i32, i32) {
    use windows::Win32::UI::WindowsAndMessaging::{GetSystemMetrics, SM_CXSCREEN, SM_CYSCREEN};
    unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) }
}

/// 全局重绘节拍（跨消息回调使用）。
#[cfg(target_os = "windows")]
static TICK: AtomicU64 = AtomicU64::new(0);

/// 窗口消息处理：定时器触发整屏重绘。
#[cfg(target_os = "windows")]
unsafe extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: windows::Win32::Foundation::WPARAM,
    lparam: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::Graphics::Gdi::{
        BeginPaint, EndPaint, FillRect, GetStockObject, InvalidateRect, SetDCBrushColor,
        DC_BRUSH, PAINTSTRUCT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{DefWindowProcW, WM_CLOSE, WM_PAINT, WM_TIMER};
    match msg {
        // 关窗（Alt+F4 / 任务栏关闭）等价于退出请求，交给主线程收敛。
        WM_CLOSE => {
            STOP_REQUESTED.store(true, Ordering::SeqCst);
            windows::Win32::Foundation::LRESULT(0)
        }
        WM_TIMER => {
            let _ = InvalidateRect(hwnd, None, false);
            windows::Win32::Foundation::LRESULT(0)
        }
        WM_PAINT => {
            let tick = TICK.fetch_add(1, Ordering::Relaxed);
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(hwnd, &mut ps);
            let (w, h) = screen_size();
            let brush = windows::Win32::Graphics::Gdi::HBRUSH(GetStockObject(DC_BRUSH).0);
            // 16 条彩条循环换色 + 一条移动白块，保证每帧都有大面积变化。
            let stripes = 16i32;
            for i in 0..stripes {
                let phase = (tick as i32 + i) % 6;
                let color = match phase {
                    0 => 0x00E0_4020,
                    1 => 0x0020_C040,
                    2 => 0x0040_20E0,
                    3 => 0x00E0_C020,
                    4 => 0x0020_C0E0,
                    _ => 0x00C0_20E0,
                };
                let left = w * i / stripes;
                let right = w * (i + 1) / stripes;
                let rect = windows::Win32::Foundation::RECT {
                    left,
                    top: 0,
                    right,
                    bottom: h,
                };
                SetDCBrushColor(hdc, windows::Win32::Foundation::COLORREF(color));
                let _ = FillRect(hdc, &rect, brush);
            }
            let bar_w = (w / 8).max(8);
            let x = (tick as i32 * 24) % (w + bar_w) - bar_w;
            let rect = windows::Win32::Foundation::RECT {
                left: x,
                top: h / 3,
                right: x + bar_w,
                bottom: h * 2 / 3,
            };
            SetDCBrushColor(hdc, windows::Win32::Foundation::COLORREF(0x00FF_FFFF));
            let _ = FillRect(hdc, &rect, brush);
            let _ = EndPaint(hwnd, &ps);
            windows::Win32::Foundation::LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

