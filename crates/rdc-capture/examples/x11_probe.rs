//! rdc-capture / Linux X11 采集实测探针（示例程序，不进入生产路径）
//!
//! 用途：在真实 X11 会话里跑固定时长的采集，输出帧率、丢帧、时间戳单调性与帧尺寸，
//!       供 `logs/TESTPLAN-s8-linux.md` 的 C1–C4/C7 判据取证。
//! 运行：`DISPLAY=:0 cargo run -p rdc-capture --example x11_probe -- --secs 10 --fps 60`
//!
//! 说明：CPU 占用由外部脚本读取 `/proc/<pid>/stat`（探针启动时打印 pid）。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rdc_capture::{create_source, CaptureConfig, ChromaFormat, Frame, FrameSink};
use tracing_subscriber::EnvFilter;

// =====** 探针统计 =====**

/// 只做计数的帧消费者（`on_frame` 必须 1ms 内返回）。
struct ProbeSink {
    /// 收到的帧数。
    frames: AtomicU64,
    /// 上一帧 pts（纳秒）。
    last_pts_ns: AtomicU64,
    /// 时间戳严格单调破坏次数。
    monotonic_violations: AtomicU64,
    /// 首帧的宽/高。
    width: AtomicU64,
    height: AtomicU64,
    /// 观察到的最小/最大帧间隔（微秒）。
    min_interval_us: AtomicU64,
    max_interval_us: AtomicU64,
    /// 是否已收到首帧。
    seen_first: AtomicBool,
}

impl ProbeSink {
    /// 创建零值的探针。
    fn new() -> Self {
        Self {
            frames: AtomicU64::new(0),
            last_pts_ns: AtomicU64::new(0),
            monotonic_violations: AtomicU64::new(0),
            width: AtomicU64::new(0),
            height: AtomicU64::new(0),
            min_interval_us: AtomicU64::new(u64::MAX),
            max_interval_us: AtomicU64::new(0),
            seen_first: AtomicBool::new(false),
        }
    }
}

impl FrameSink for ProbeSink {
    /// 收帧：只更新计数与单调性检查，不拷贝像素。
    ///
    /// # Arguments
    /// * `frame` - 采集帧；本函数立即释放。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    fn on_frame(&self, frame: Frame) {
        let previous = self.last_pts_ns.swap(frame.meta.pts_ns, Ordering::Relaxed);
        if previous != 0 && frame.meta.pts_ns <= previous {
            self.monotonic_violations.fetch_add(1, Ordering::Relaxed);
        }
        let first = !self.seen_first.swap(true, Ordering::Relaxed);
        if first {
            self.width.store(u64::from(frame.width), Ordering::Relaxed);
            self.height.store(u64::from(frame.height), Ordering::Relaxed);
        }
        // 首帧 interval 为「首帧相对会话起点」（含启动期的稳定复核等待），不计入间隔统计。
        if !first && frame.interval_us > 0 {
            self.min_interval_us
                .fetch_min(u64::from(frame.interval_us), Ordering::Relaxed);
            self.max_interval_us
                .fetch_max(u64::from(frame.interval_us), Ordering::Relaxed);
        }
        self.frames.fetch_add(1, Ordering::Relaxed);
    }
}

// =====** 参数解析 =====**

/// 命令行参数。
struct Args {
    /// 采集时长（秒）。
    secs: u64,
    /// 目标帧率。
    fps: u32,
    /// 目标显示器编号。
    display_id: u32,
    /// 是否合成光标。
    include_cursor: bool,
    /// 判定通过的最小帧率。
    min_fps: f64,
}

/// 解析命令行参数（未知参数直接报错退出，避免实测参数写错还被当成通过）。
///
/// # Returns
/// 解析后的参数。
///
/// # Errors
/// 参数缺失/非法时返回说明文本。
fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        secs: 10,
        fps: 60,
        display_id: 0,
        include_cursor: false,
        min_fps: 55.0,
    };
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--secs" => {
                args.secs = iter
                    .next()
                    .ok_or("--secs 缺少值")?
                    .parse()
                    .map_err(|_| "--secs 需要整数")?;
            }
            "--fps" => {
                args.fps = iter
                    .next()
                    .ok_or("--fps 缺少值")?
                    .parse()
                    .map_err(|_| "--fps 需要整数")?;
            }
            "--display-id" => {
                args.display_id = iter
                    .next()
                    .ok_or("--display-id 缺少值")?
                    .parse()
                    .map_err(|_| "--display-id 需要整数")?;
            }
            "--cursor" => args.include_cursor = true,
            "--min-fps" => {
                args.min_fps = iter
                    .next()
                    .ok_or("--min-fps 缺少值")?
                    .parse()
                    .map_err(|_| "--min-fps 需要数字")?;
            }
            other => return Err(format!("未知参数: {other}")),
        }
    }
    Ok(args)
}

// =====** 入口 =====**

/// 探针入口。
///
/// # Returns
/// 满足判据返回 0，否则返回 2。
///
/// # Errors
/// 不会 panic；初始化失败以退出码 2 表达。
fn main() -> std::process::ExitCode {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .try_init();
    let args = match parse_args() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("参数错误: {error}");
            return std::process::ExitCode::from(2);
        }
    };
    println!(
        "PROBE_START pid={} secs={} target_fps={} display_id={}",
        std::process::id(),
        args.secs,
        args.fps,
        args.display_id
    );

    let mut source = match create_source() {
        Ok(source) => source,
        Err(error) => {
            eprintln!("创建采集源失败: {error}");
            return std::process::ExitCode::from(2);
        }
    };
    let displays = source.enumerate();
    for display in &displays {
        println!(
            "DISPLAY id={} name={} {}x{}@{} primary={}",
            display.id, display.name, display.width, display.height, display.refresh_hz, display.is_primary
        );
    }
    if displays.is_empty() {
        eprintln!("未枚举到显示器");
        return std::process::ExitCode::from(2);
    }

    let sink = Arc::new(ProbeSink::new());
    source.set_sink(Arc::clone(&sink) as Arc<dyn FrameSink>);
    let config = CaptureConfig {
        display_id: args.display_id,
        target_fps: args.fps,
        include_cursor: args.include_cursor,
        chroma: ChromaFormat::Bgra,
        bit_depth: 8,
    };
    if let Err(error) = source.start(config) {
        eprintln!("启动采集失败: {error}");
        return std::process::ExitCode::from(2);
    }
    let backend = source.backend_name();
    println!("BACKEND backend={backend}");

    // 每秒打印一次累计帧数，便于外部脚本记录每秒帧率。
    let mut last_frames = 0u64;
    for second in 1..=args.secs {
        std::thread::sleep(Duration::from_secs(1));
        let frames = sink.frames.load(Ordering::Relaxed);
        let delta = frames - last_frames;
        last_frames = frames;
        println!(
            "PROBE_SECOND t={second}s frames={frames} delta={delta} fps={delta:.1}"
        );
    }

    let snapshot = source.stats();
    source.stop();
    let frames = sink.frames.load(Ordering::Relaxed);
    let violations = sink.monotonic_violations.load(Ordering::Relaxed);
    let width = sink.width.load(Ordering::Relaxed);
    let height = sink.height.load(Ordering::Relaxed);
    let min_interval = sink.min_interval_us.load(Ordering::Relaxed);
    let max_interval = sink.max_interval_us.load(Ordering::Relaxed);
    let elapsed_ms = snapshot.elapsed_ms.max(1);
    let fps = frames as f64 / (elapsed_ms as f64 / 1000.0);

    // 输出稳定的 JSON 行，供测试脚本解析。
    println!(
        "PROBE_JSON {{\"pid\":{},\"secs\":{},\"frames\":{},\"fps\":{:.2},\"captured\":{},\"dropped\":{},\"pool_exhausted\":{},\"monotonic_ok\":{},\"monotonic_violations\":{},\"width\":{},\"height\":{},\"cost_p50_us\":{},\"cost_p99_us\":{},\"interval_p50_us\":{},\"interval_p99_us\":{},\"min_interval_us\":{},\"max_interval_us\":{},\"backend\":\"{}\"}}",
        std::process::id(),
        args.secs,
        frames,
        fps,
        snapshot.captured,
        snapshot.dropped,
        snapshot.pool_exhausted,
        violations == 0,
        violations,
        width,
        height,
        snapshot.cost_p50_us,
        snapshot.cost_p99_us,
        snapshot.interval_p50_us,
        snapshot.interval_p99_us,
        if min_interval == u64::MAX { 0 } else { min_interval },
        max_interval,
        backend,
    );

    if fps >= args.min_fps && violations == 0 {
        println!("PROBE_VERDICT pass fps={fps:.2} min_fps={:.2}", args.min_fps);
        std::process::ExitCode::SUCCESS
    } else {
        println!(
            "PROBE_VERDICT fail fps={fps:.2} min_fps={:.2} monotonic_violations={violations}",
            args.min_fps
        );
        std::process::ExitCode::from(2)
    }
}
