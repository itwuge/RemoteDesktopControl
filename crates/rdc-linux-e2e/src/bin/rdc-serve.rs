//! rdc-serve · Linux/X11 常驻被控端（S8-5）
//!
//! 用途：把 S8-2 的最小被控链路（X11 采集 → 软编 → TCP 推流 + XTest 输入注入）
//!       做成「开机能起、可被连接、可看日志」的常驻服务，供安卓线直连。
//!
//! 启动：`rdc-serve --config ~/.config/rdc/serve.conf`
//! 停止：`kill -TERM <child pid>`（读 `run.pid`）；收到信号后按 exit 0 退出。
//!
//! 线协议：与 `s8_2_host` **完全一致**（`kind: u8 + len: u32 LE + payload`），
//!        本轮未改动字节级格式（见 `crates/rdc-linux-e2e/src/protocol.rs`）。
//! 边界：仅 Linux/X11；一次服务一个客户端（后续连接排 listen backlog）；
//!       无加密、无配对校验（协议无该字段，缺口见 research/s8-5-linux-serve.md）。

#[cfg(not(target_os = "linux"))]
fn main() {
    rdc_linux_e2e::unsupported();
}

#[cfg(target_os = "linux")]
fn main() {
    std::process::exit(serve_impl::main());
}

#[cfg(target_os = "linux")]
mod serve_impl {
    use std::io::{ErrorKind, Write};
    use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

    use rdc_capture::{create_source, CaptureConfig, ChromaFormat, Frame, FrameSink};
    use rdc_codec::{create_encoder, CodecKind, VideoChroma, VideoConfig, VideoEncoder};
    use rdc_input::{create_injector, decode_event, InputInjector, Viewport};
    use rdc_linux_e2e::protocol;

    // =====** 常量 =====**

    /// 默认监听地址：对局域网可连（安卓模拟器 / 真机都要走这里）。
    const DEFAULT_LISTEN: &str = "0.0.0.0:45900";
    /// 默认帧率。
    const DEFAULT_FPS: u32 = 60;
    /// 默认码率（8 Mbps，与 S8-2 一致）。
    const DEFAULT_BITRATE_BPS: u32 = 8_000_000;
    /// 默认显示器编号。
    const DEFAULT_DISPLAY_ID: u32 = 0;
    /// 编码积压上限：超过即丢采集帧，避免无限排队推高延迟。
    const DEFAULT_MAX_BACKLOG: u64 = 3;
    /// 统计输出周期。
    const STATS_INTERVAL: Duration = Duration::from_secs(1);
    /// 空闲心跳周期（无客户端时也证明服务活着）。
    const IDLE_INTERVAL: Duration = Duration::from_secs(10);
    /// 推流写超时：对端不读时不能永久阻塞常驻进程。
    const WRITE_TIMEOUT: Duration = Duration::from_secs(2);

    /// SIGTERM/SIGINT 触发后的停机标志（信号处理器只写这个原子量）。
    static STOP: AtomicBool = AtomicBool::new(false);

    /// 日志文件路径（panic hook 在拿不到 Logger 时直接追加到这里）。
    static LOG_PATH: OnceLock<PathBuf> = OnceLock::new();

    // =====** 配置 =====**

    /// 生效配置（命令行 > 配置文件 > 默认值）。
    struct Config {
        listen: String,
        pair: String,
        fps: u32,
        bitrate_bps: u32,
        display_id: u32,
        max_backlog: u64,
        log_path: PathBuf,
        status_path: PathBuf,
        config_path: PathBuf,
    }

    /// 命令行原始覆盖项（全部可选，未给的项交给配置文件/默认值）。
    #[derive(Default)]
    struct Overrides {
        config: Option<String>,
        listen: Option<String>,
        pair: Option<String>,
        fps: Option<u32>,
        bitrate_bps: Option<u32>,
        display_id: Option<u32>,
        max_backlog: Option<u64>,
        log: Option<String>,
        status: Option<String>,
    }

    /// 解析命令行；`--help` 打印用法后直接退出。
    fn parse_args() -> Result<Overrides, String> {
        let mut over = Overrides::default();
        let mut iter = std::env::args().skip(1);
        while let Some(arg) = iter.next() {
            match arg.as_str() {
                "--config" => over.config = Some(take_value(&arg, &mut iter)?),
                "--listen" => over.listen = Some(take_value(&arg, &mut iter)?),
                "--pair" => over.pair = Some(take_value(&arg, &mut iter)?),
                "--fps" => {
                    over.fps = Some(
                        take_value(&arg, &mut iter)?
                            .parse()
                            .map_err(|_| "--fps 需整数".to_string())?,
                    )
                }
                "--bitrate" => {
                    over.bitrate_bps = Some(
                        take_value(&arg, &mut iter)?
                            .parse()
                            .map_err(|_| "--bitrate 需整数".to_string())?,
                    )
                }
                "--display-id" => {
                    over.display_id = Some(
                        take_value(&arg, &mut iter)?
                            .parse()
                            .map_err(|_| "--display-id 需整数".to_string())?,
                    )
                }
                "--max-backlog" => {
                    over.max_backlog = Some(
                        take_value(&arg, &mut iter)?
                            .parse()
                            .map_err(|_| "--max-backlog 需整数".to_string())?,
                    )
                }
                "--log" => over.log = Some(take_value(&arg, &mut iter)?),
                "--status" => over.status = Some(take_value(&arg, &mut iter)?),
                "--help" | "-h" => {
                    print_usage();
                    std::process::exit(0);
                }
                other => return Err(format!("未知参数: {other}")),
            }
        }
        Ok(over)
    }

    /// 取参数值；缺值统一报错（避免把下一个开关当值吞掉后行为漂移）。
    fn take_value(arg: &str, iter: &mut impl Iterator<Item = String>) -> Result<String, String> {
        iter.next().ok_or_else(|| format!("{arg} 缺值"))
    }

    /// 打印用法。
    fn print_usage() {
        println!(
            "用法: rdc-serve [--config FILE] [--listen ADDR] [--pair CODE] [--fps N] \
             [--bitrate BPS] [--display-id ID] [--max-backlog N] [--log PATH] [--status PATH]"
        );
        println!("默认: listen={DEFAULT_LISTEN} fps={DEFAULT_FPS} bitrate={DEFAULT_BITRATE_BPS}");
        println!("配置文件: 每行 key=value，键同命令行长参数，支持 # 注释");
    }

    /// 读最小 `key=value` 配置文件（不支持引号/转义，刻意保持极简）。
    fn read_config(path: &Path) -> Result<Vec<(String, String)>, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|error| format!("读配置文件 {} 失败: {error}", path.display()))?;
        let mut pairs = Vec::new();
        for (index, raw) in text.lines().enumerate() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(format!(
                    "{} 第 {} 行不是 key=value: {line}",
                    path.display(),
                    index + 1
                ));
            };
            pairs.push((key.trim().to_string(), value.trim().to_string()));
        }
        Ok(pairs)
    }

    /// 合并三层配置并解析配对码。
    fn build_config() -> Result<Config, String> {
        let over = parse_args()?;
        let home = std::env::var("HOME").map_err(|_| "环境变量 HOME 缺失".to_string())?;
        let xdg_config = std::env::var("XDG_CONFIG_HOME").unwrap_or_else(|_| format!("{home}/.config"));
        let xdg_state = std::env::var("XDG_STATE_HOME").unwrap_or_else(|_| format!("{home}/.local/state"));

        let config_path = PathBuf::from(
            over.config
                .clone()
                .unwrap_or_else(|| format!("{xdg_config}/rdc/serve.conf")),
        );
        let file_pairs = if config_path.exists() {
            read_config(&config_path)?
        } else {
            Vec::new()
        };
        let file_value = |key: &str| {
            file_pairs
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
        };

        let listen = over
            .listen
            .or_else(|| file_value("listen"))
            .unwrap_or_else(|| DEFAULT_LISTEN.to_string());
        let fps = over
            .fps
            .or_else(|| file_value("fps").and_then(|v| v.parse().ok()))
            .unwrap_or(DEFAULT_FPS);
        let bitrate_bps = over
            .bitrate_bps
            .or_else(|| file_value("bitrate").and_then(|v| v.parse().ok()))
            .unwrap_or(DEFAULT_BITRATE_BPS);
        let display_id = over
            .display_id
            .or_else(|| file_value("display_id").and_then(|v| v.parse().ok()))
            .unwrap_or(DEFAULT_DISPLAY_ID);
        let max_backlog = over
            .max_backlog
            .or_else(|| file_value("max_backlog").and_then(|v| v.parse().ok()))
            .unwrap_or(DEFAULT_MAX_BACKLOG);

        let pair = match over.pair.or_else(|| file_value("pair")) {
            Some(code) if !code.is_empty() => code,
            _ => resolve_pair_code(&format!("{xdg_config}/rdc/pair-code"))?,
        };

        let log_path = PathBuf::from(
            over.log
                .or_else(|| file_value("log"))
                .unwrap_or_else(|| format!("{xdg_state}/rdc/serve.log")),
        );
        let status_path = PathBuf::from(
            over.status
                .or_else(|| file_value("status"))
                .unwrap_or_else(|| format!("{xdg_state}/rdc/serve-status.json")),
        );

        Ok(Config {
            listen,
            pair,
            fps,
            bitrate_bps,
            display_id,
            max_backlog,
            log_path,
            status_path,
            config_path,
        })
    }

    /// 配对码：优先读持久化文件，缺失则生成 6 位十进制并落盘（0600）。
    fn resolve_pair_code(path: &str) -> Result<String, String> {
        if let Ok(text) = std::fs::read_to_string(path) {
            let code = text.trim();
            if !code.is_empty() {
                return Ok(code.to_string());
            }
        }
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos() as u64)
            .unwrap_or(0);
        let code = format!("{:06}", 100_000 + nanos % 900_000);
        if let Some(parent) = Path::new(path).parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("创建 {} 失败: {error}", parent.display()))?;
        }
        std::fs::write(path, format!("{code}\n"))
            .map_err(|error| format!("写配对码文件 {path} 失败: {error}"))?;
        set_private_permissions(path)?;
        Ok(code)
    }

    /// 把文件权限收紧到 0600（配对码不该被同机其他用户读到）。
    fn set_private_permissions(path: &str) -> Result<(), String> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|error| format!("设置 {path} 权限失败: {error}"))
    }

    // =====** 日志 =====**

    /// 固定路径追加式日志：每行 `时间 级别 错误码 描述`，逐行 flush 保证崩溃前不丢。
    struct Logger {
        file: std::fs::File,
    }

    impl Logger {
        /// 打开日志文件（自动建目录）。
        fn open(path: &Path) -> Result<Self, String> {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("创建日志目录 {} 失败: {error}", parent.display()))?;
            }
            let file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(path)
                .map_err(|error| format!("打开日志 {} 失败: {error}", path.display()))?;
            Ok(Self { file })
        }

        /// 写一行结构化日志。
        fn line(&mut self, level: &str, code: &str, message: &str) {
            let text = format!("{} {level:<5} {code} {message}\n", timestamp());
            let _ = self.file.write_all(text.as_bytes());
            let _ = self.file.flush();
        }

        /// INFO 级。
        fn info(&mut self, code: &str, message: &str) {
            self.line("INFO", code, message);
        }

        /// WARN 级（可恢复的异常）。
        fn warn(&mut self, code: &str, message: &str) {
            self.line("WARN", code, message);
        }

        /// ERROR 级（会话级失败）。
        fn error(&mut self, code: &str, message: &str) {
            self.line("ERROR", code, message);
        }
    }

    /// 共享日志句柄；锁被污染时取回内部值继续写，不让日志先死。
    type SharedLog = Arc<Mutex<Logger>>;

    /// 加锁写日志（任何线程可调用）。
    fn log_info(log: &SharedLog, code: &str, message: &str) {
        match log.lock() {
            Ok(mut guard) => guard.info(code, message),
            Err(poisoned) => poisoned.into_inner().info(code, message),
        }
    }

    /// 加锁写 WARN。
    fn log_warn(log: &SharedLog, code: &str, message: &str) {
        match log.lock() {
            Ok(mut guard) => guard.warn(code, message),
            Err(poisoned) => poisoned.into_inner().warn(code, message),
        }
    }

    /// 加锁写 ERROR。
    fn log_error(log: &SharedLog, code: &str, message: &str) {
        match log.lock() {
            Ok(mut guard) => guard.error(code, message),
            Err(poisoned) => poisoned.into_inner().error(code, message),
        }
    }

    /// UTC 时间戳（`YYYY-MM-DDTHH:MM:SSZ`）；不引第三方时间库。
    fn timestamp() -> String {
        let secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs() as i64)
            .unwrap_or(0);
        let days = secs / 86_400;
        let rem = secs % 86_400;
        let (year, month, day) = civil_from_days(days);
        format!(
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
            rem / 3_600,
            (rem % 3_600) / 60,
            rem % 60
        )
    }

    /// 公历日期换算（Howard Hinnant 的 `civil_from_days`，纯整数无 unsafe）。
    fn civil_from_days(days: i64) -> (i64, u32, u32) {
        let z = days + 719_468;
        let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
        let year = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
        let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
        (year + i64::from(month <= 2), month, day)
    }

    // =====** 状态与计数 =====**

    /// 会话计数（采集/编码/注入/网络），跨线程共享。
    #[derive(Default)]
    struct Counters {
        captured: AtomicU64,
        encoded: AtomicU64,
        produced: AtomicU64,
        packets: AtomicU64,
        bytes: AtomicU64,
        encode_errors: AtomicU64,
        backpressure: AtomicU64,
        write_errors: AtomicU64,
        injected: AtomicU64,
        inject_errors: AtomicU64,
        /** 每次 `inject` 调用的耗时（微秒）；统计线程每秒取走一次。 */
        inject_samples_us: Mutex<Vec<u64>>,
    }

    /// 状态文件内容（每秒覆盖写，供安卓线/运维快速判读）。
    struct Status<'a> {
        pid: u32,
        listen: String,
        pair: &'a str,
        uptime_s: u64,
        peer: Option<SocketAddr>,
        connected: bool,
        capture_fps: u64,
        injected_total: u64,
        rss_kb: u64,
        last_error: &'a str,
    }

    /// 原子替换写状态文件（先写临时文件再 rename，避免读到半个 JSON）。
    fn write_status(path: &Path, status: &Status<'_>) {
        let peer = status
            .peer
            .map(|addr| format!("\"{addr}\""))
            .unwrap_or_else(|| "null".to_string());
        let body = format!(
            "{{\"ts\":\"{}\",\"pid\":{},\"listen\":\"{}\",\"pair\":\"{}\",\"uptime_s\":{},\
             \"peer\":{},\"connected\":{},\"capture_fps\":{},\"injected_total\":{},\
             \"rss_kb\":{},\"last_error\":\"{}\"}}\n",
            timestamp(),
            status.pid,
            status.listen,
            status.pair,
            status.uptime_s,
            peer,
            status.connected,
            status.capture_fps,
            status.injected_total,
            status.rss_kb,
            status.last_error
        );
        let tmp = path.with_extension("json.tmp");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if std::fs::write(&tmp, body).is_ok() {
            let _ = std::fs::rename(&tmp, path);
        }
    }

    /// 读本进程 RSS（kB）；读不到返回 0（不是判据失败，只是缺样本）。
    fn rss_kb() -> u64 {
        let Ok(text) = std::fs::read_to_string("/proc/self/status") else {
            return 0;
        };
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("VmRSS:") {
                return rest
                    .trim()
                    .split_whitespace()
                    .next()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(0);
            }
        }
        0
    }

    /// 计算分位数（输入已排序后调用）。
    fn percentile(sorted: &[u64], pct: f64) -> u64 {
        if sorted.is_empty() {
            return 0;
        }
        let index = ((sorted.len() as f64 - 1.0) * pct).round() as usize;
        sorted[index.min(sorted.len() - 1)]
    }

    /// 取走本秒的注入延迟样本并给出 p50/p99/max（微秒）。
    fn take_inject_stats(counters: &Counters) -> (u64, u64, u64, usize) {
        let mut guard = match counters.inject_samples_us.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let mut samples = std::mem::take(&mut *guard);
        if samples.is_empty() {
            return (0, 0, 0, 0);
        }
        samples.sort_unstable();
        let p50 = percentile(&samples, 0.50);
        let p99 = percentile(&samples, 0.99);
        let max = samples.last().copied().unwrap_or(0);
        (p50, p99, max, samples.len())
    }

    // =====** 采集 → 编码 =====**

    /// 采集回调：计数 + 背压判断 + 送编码器。
    struct HostSink {
        encoder: Arc<Mutex<Box<dyn VideoEncoder>>>,
        counters: Arc<Counters>,
        max_backlog: u64,
    }

    impl FrameSink for HostSink {
        fn on_frame(&self, frame: Frame) {
            self.counters.captured.fetch_add(1, Ordering::Relaxed);
            let encoded = self.counters.encoded.load(Ordering::Relaxed);
            let produced = self.counters.produced.load(Ordering::Relaxed);
            // 背压：已推入编码器但还没被取走的帧数超过上限时丢帧，避免延迟无限堆积。
            if encoded.saturating_sub(produced) >= self.max_backlog {
                self.counters.backpressure.fetch_add(1, Ordering::Relaxed);
                return;
            }
            let pts_ns = rdc_linux_e2e::clock::mono_ns();
            let Frame { buffer, .. } = frame;
            let Ok(mut encoder) = self.encoder.lock() else {
                self.counters.encode_errors.fetch_add(1, Ordering::Relaxed);
                return;
            };
            match encoder.push(buffer, pts_ns, false) {
                Ok(()) => {
                    self.counters.encoded.fetch_add(1, Ordering::Relaxed);
                }
                Err(_) => {
                    self.counters.encode_errors.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
    }

    // =====** 输入注入 =====**

    /// 输入线程：读 `INPUT` 包并用 XTest 注入；每条记录注入耗时（微秒）。
    fn injection_loop(
        mut stream: TcpStream,
        viewport: Viewport,
        alive: Arc<AtomicBool>,
        counters: Arc<Counters>,
        log: SharedLog,
    ) {
        let mut injector: Box<dyn InputInjector> = match create_injector() {
            Ok(injector) => injector,
            Err(error) => {
                log_error(&log, "E_INJECTOR_INIT", &format!("创建注入器失败: {error}"));
                alive.store(false, Ordering::SeqCst);
                return;
            }
        };
        injector.set_viewport(viewport);
        let mut logged_errors = 0u32;
        loop {
            match protocol::read_packet(&mut stream) {
                Ok(Some(packet)) => match packet.kind {
                    protocol::KIND_INPUT => {
                        let event = match decode_event(&packet.payload) {
                            Ok(event) => event,
                            Err(error) => {
                                counters.inject_errors.fetch_add(1, Ordering::Relaxed);
                                if logged_errors < 3 {
                                    logged_errors += 1;
                                    log_warn(&log, "E_INPUT_DECODE", &format!("输入包解析失败: {error}"));
                                }
                                continue;
                            }
                        };
                        let started = Instant::now();
                        match injector.inject(event) {
                            Ok(()) => {
                                counters.injected.fetch_add(1, Ordering::Relaxed);
                                let micros = started.elapsed().as_micros() as u64;
                                if let Ok(mut guard) = counters.inject_samples_us.lock() {
                                    if guard.len() < 4096 {
                                        guard.push(micros);
                                    }
                                }
                            }
                            Err(error) => {
                                counters.inject_errors.fetch_add(1, Ordering::Relaxed);
                                if logged_errors < 3 {
                                    logged_errors += 1;
                                    log_warn(&log, "E_INJECT", &format!("注入失败: {error}"));
                                }
                            }
                        }
                    }
                    protocol::KIND_BYE => break,
                    _ => {}
                },
                Ok(None) | Err(_) => break,
            }
        }
        alive.store(false, Ordering::SeqCst);
    }

    // =====** 单会话 =====**

    /// 会话结束原因。
    struct SessionEnd {
        code: &'static str,
        detail: String,
    }

    impl SessionEnd {
        /// 构造结束原因。
        fn new(code: &'static str, detail: impl Into<String>) -> Self {
            Self {
                code,
                detail: detail.into(),
            }
        }
    }

    /// 服务一个客户端直到断开；返回结束原因（不 panic、不拖垮常驻循环）。
    fn run_session(
        stream: TcpStream,
        peer: SocketAddr,
        cfg: &Config,
        log: &SharedLog,
        pid: u32,
        started_at: Instant,
        counters: Arc<Counters>,
    ) -> SessionEnd {
        if stream.set_nodelay(true).is_err() {
            log_warn(log, "E_SOCKET", "设置 TCP_NODELAY 失败（继续）");
        }
        if stream.set_write_timeout(Some(WRITE_TIMEOUT)).is_err() {
            log_warn(log, "E_SOCKET", "设置写超时失败（继续）");
        }

        // 1) 采集源与显示器。
        let mut source = match create_source() {
            Ok(source) => source,
            Err(error) => {
                return SessionEnd::new("E_CAPTURE_INIT", format!("创建采集源失败: {error}"))
            }
        };
        let displays = source.enumerate();
        let Some(display) = displays
            .iter()
            .find(|item| item.id == cfg.display_id)
            .or_else(|| displays.first())
        else {
            return SessionEnd::new("E_CAPTURE_INIT", "未枚举到显示器（DISPLAY 是否可用？）");
        };
        let display_id = display.id;
        let display_name = display.name.clone();
        let (width, height) = (display.width, display.height);
        if width == 0 || height == 0 {
            return SessionEnd::new("E_CAPTURE_INIT", format!("显示器尺寸非法 {width}x{height}"));
        }
        log_info(
            log,
            "DISPLAY",
            &format!("id={display_id} name={display_name} {width}x{height}"),
        );

        // 2) 编码器。
        let mut encoder = match create_encoder(CodecKind::H264) {
            Ok(encoder) => encoder,
            Err(error) => {
                return SessionEnd::new("E_ENCODER_INIT", format!("创建编码器失败: {error}"))
            }
        };
        let video_config = VideoConfig {
            codec: CodecKind::H264,
            width,
            height,
            fps: cfg.fps,
            bitrate_bps: cfg.bitrate_bps,
            chroma: VideoChroma::Yuv420,
            bit_depth: 8,
            low_latency: true,
        };
        if let Err(error) = encoder.configure(video_config) {
            return SessionEnd::new("E_ENCODER_INIT", format!("配置编码器失败: {error}"));
        }
        let backend = encoder.backend_name().to_string();
        log_info(
            log,
            "ENCODER",
            &format!(
                "backend={backend} {width}x{height}@{fps} bitrate={bitrate}",
                fps = cfg.fps,
                bitrate = cfg.bitrate_bps
            ),
        );
        let encoder = Arc::new(Mutex::new(encoder));

        // 3) 采集启动。
        let sink = Arc::new(HostSink {
            encoder: Arc::clone(&encoder),
            counters: Arc::clone(&counters),
            max_backlog: cfg.max_backlog,
        });
        source.set_sink(Arc::clone(&sink) as Arc<dyn FrameSink>);
        let capture_config = CaptureConfig {
            display_id,
            target_fps: cfg.fps,
            include_cursor: false,
            chroma: ChromaFormat::Bgra,
            bit_depth: 8,
        };
        if let Err(error) = source.start(capture_config) {
            return SessionEnd::new("E_CAPTURE_INIT", format!("启动采集失败: {error}"));
        }
        log_info(
            log,
            "CAPTURE",
            &format!("backend={} target_fps={}", source.backend_name(), cfg.fps),
        );

        // 4) 先发 CONFIG，再进入推流循环。
        let mut writer = match stream.try_clone() {
            Ok(writer) => writer,
            Err(error) => {
                source.stop();
                return SessionEnd::new("E_SOCKET", format!("克隆 socket 失败: {error}"));
            }
        };
        let config_packet = protocol::Packet {
            kind: protocol::KIND_CONFIG,
            payload: protocol::StreamConfig {
                width,
                height,
                fps: cfg.fps,
            }
            .to_payload(),
        };
        if let Err(error) = config_packet.write_to(&mut writer) {
            source.stop();
            return SessionEnd::new("E_WRITE", format!("发送 CONFIG 失败: {error}"));
        }
        log_info(log, "CONFIG_SENT", &format!("{width}x{height}@{fps}", fps = cfg.fps));

        // 5) 输入注入线程。
        let alive = Arc::new(AtomicBool::new(true));
        let reader = match stream.try_clone() {
            Ok(reader) => reader,
            Err(error) => {
                source.stop();
                return SessionEnd::new("E_SOCKET", format!("克隆读端失败: {error}"));
            }
        };
        let viewport = Viewport {
            x: 0,
            y: 0,
            width,
            height,
            scale: 1.0,
        };
        let inject_alive = Arc::clone(&alive);
        let inject_counters = Arc::clone(&counters);
        let inject_log = Arc::clone(log);
        let inject_handle = match std::thread::Builder::new()
            .name("rdc-inject".into())
            .spawn(move || injection_loop(reader, viewport, inject_alive, inject_counters, inject_log))
        {
            Ok(handle) => handle,
            Err(error) => {
                source.stop();
                return SessionEnd::new("E_SOCKET", format!("输入线程创建失败: {error}"));
            }
        };

        // 6) 推流 + 每秒统计。
        let session_started = Instant::now();
        let mut last_stats = Instant::now();
        let mut last_captured = 0u64;
        let mut last_packets = 0u64;
        let mut last_bytes = 0u64;
        let mut last_injected = 0u64;
        let mut end = SessionEnd::new("E_PEER_EOF", "客户端关闭连接");
        loop {
            if STOP.load(Ordering::SeqCst) {
                end = SessionEnd::new("E_STOP", "服务收到停止信号");
                break;
            }
            if !alive.load(Ordering::SeqCst) {
                break;
            }
            loop {
                let packet = match encoder.lock() {
                    Ok(mut guard) => guard.poll_packet(),
                    Err(_) => {
                        end = SessionEnd::new("E_ENCODER", "编码器锁被污染");
                        break;
                    }
                };
                let Some(packet) = packet else {
                    break;
                };
                counters.produced.fetch_add(1, Ordering::Relaxed);
                let payload =
                    protocol::video_payload(packet.pts_ns, packet.is_keyframe, &packet.data);
                let out = protocol::Packet {
                    kind: protocol::KIND_VIDEO,
                    payload,
                };
                if out.write_to(&mut writer).is_err() {
                    counters.write_errors.fetch_add(1, Ordering::Relaxed);
                    end = SessionEnd::new("E_WRITE", "推流写失败或写超时");
                    break;
                }
                counters.packets.fetch_add(1, Ordering::Relaxed);
                counters.bytes.fetch_add(packet.data.len() as u64, Ordering::Relaxed);
            }
            if !matches!(end.code, "E_PEER_EOF") {
                break;
            }
            if last_stats.elapsed() >= STATS_INTERVAL {
                emit_stats(
                    log,
                    &counters,
                    peer,
                    &cfg.status_path,
                    cfg,
                    pid,
                    started_at,
                );
                let captured_now = counters.captured.load(Ordering::Relaxed);
                let packets_now = counters.packets.load(Ordering::Relaxed);
                let bytes_now = counters.bytes.load(Ordering::Relaxed);
                let injected_now = counters.injected.load(Ordering::Relaxed);
                let elapsed = last_stats.elapsed().as_secs_f64().max(0.001);
                let capture_fps = ((captured_now - last_captured) as f64 / elapsed).round() as u64;
                let packet_fps = ((packets_now - last_packets) as f64 / elapsed).round() as u64;
                let bitrate = (((bytes_now - last_bytes) as f64 * 8.0) / elapsed) as u64;
                let (p50, p99, max, samples) = take_inject_stats(&counters);
                log_info(
                    log,
                    "STATS",
                    &format!(
                        "peer={peer} capture_fps={capture_fps} packet_fps={packet_fps} \
                         bitrate_bps={bitrate} packets_total={packets_now} \
                         injected_total={injected_now} inject_rate={} \
                         inject_p50_us={p50} inject_p99_us={p99} inject_max_us={max} \
                         inject_samples={samples} rss_kb={} backpressure={} \
                         encode_errors={} write_errors={}",
                        injected_now - last_injected,
                        rss_kb(),
                        counters.backpressure.load(Ordering::Relaxed),
                        counters.encode_errors.load(Ordering::Relaxed),
                        counters.write_errors.load(Ordering::Relaxed)
                    ),
                );
                last_captured = captured_now;
                last_packets = packets_now;
                last_bytes = bytes_now;
                last_injected = injected_now;
                last_stats = Instant::now();
            }
            std::thread::sleep(Duration::from_millis(1));
        }

        // 7) 收尾：停采集 → 断开读端让注入线程退出 → 冲刷编码器。
        source.stop();
        let _ = stream.shutdown(Shutdown::Both);
        let _ = inject_handle.join();
        let flushed = match encoder.lock() {
            Ok(mut guard) => guard.flush().unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        for packet in flushed {
            counters.produced.fetch_add(1, Ordering::Relaxed);
            let payload =
                protocol::video_payload(packet.pts_ns, packet.is_keyframe, &packet.data);
            let written = protocol::Packet {
                kind: protocol::KIND_VIDEO,
                payload,
            }
            .write_to(&mut writer);
            if written.is_ok() {
                counters.packets.fetch_add(1, Ordering::Relaxed);
                counters.bytes.fetch_add(packet.data.len() as u64, Ordering::Relaxed);
            }
        }
        let _ = protocol::Packet {
            kind: protocol::KIND_BYE,
            payload: Vec::new(),
        }
        .write_to(&mut writer);
        let _ = writer.flush();

        SessionEnd::new(
            end.code,
            format!(
                "{} session_s={:.1} captured={} packets={} injected={}",
                end.detail,
                session_started.elapsed().as_secs_f64(),
                counters.captured.load(Ordering::Relaxed),
                counters.packets.load(Ordering::Relaxed),
                counters.injected.load(Ordering::Relaxed)
            ),
        )
    }

    /// 写一条 STATS/状态文件（会话内每秒调用）。
    fn emit_stats(
        log: &SharedLog,
        counters: &Counters,
        peer: SocketAddr,
        status_path: &Path,
        cfg: &Config,
        pid: u32,
        started_at: Instant,
    ) {
        write_status(
            status_path,
            &Status {
                pid,
                listen: cfg.listen.clone(),
                pair: &cfg.pair,
                uptime_s: started_at.elapsed().as_secs(),
                peer: Some(peer),
                connected: true,
                capture_fps: counters.captured.load(Ordering::Relaxed),
                injected_total: counters.injected.load(Ordering::Relaxed),
                rss_kb: rss_kb(),
                last_error: "",
            },
        );
    }

    // =====** 常驻循环 =====**

    /// 安装 SIGTERM/SIGINT 处理器：只置停机标志，主循环下一轮退出（exit 0）。
    fn install_signal_handlers() {
        unsafe {
            libc::signal(libc::SIGTERM, handle_signal as usize as libc::sighandler_t);
            libc::signal(libc::SIGINT, handle_signal as usize as libc::sighandler_t);
        }
    }

    /// 信号处理器（异步信号安全：只做原子写）。
    extern "C" fn handle_signal(_signal: libc::c_int) {
        STOP.store(true, Ordering::SeqCst);
    }

    /// 绑定监听并按会话循环；不因单个会话失败退出。
    fn serve(cfg: &Config, log: &SharedLog, pid: u32, started_at: Instant) -> Result<(), String> {
        let listener = TcpListener::bind(&cfg.listen)
            .map_err(|error| format!("监听 {} 失败: {error}", cfg.listen))?;
        listener
            .set_nonblocking(true)
            .map_err(|error| format!("设置非阻塞监听失败: {error}"))?;
        let local = listener
            .local_addr()
            .map_err(|error| format!("读取监听地址失败: {error}"))?;
        let banner = format!(
            "addr={} port={} pair={} pid={} fps={} bitrate={} config={}",
            local.ip(),
            local.port(),
            cfg.pair,
            pid,
            cfg.fps,
            cfg.bitrate_bps,
            cfg.config_path.display()
        );
        log_info(log, "SERVE_LISTEN", &banner);
        println!("SERVE_LISTEN {banner}");

        let counters = Arc::new(Counters::default());
        let mut last_idle = Instant::now();
        let mut sessions = 0u64;
        loop {
            if STOP.load(Ordering::SeqCst) {
                log_info(log, "SERVE_STOP", "收到 SIGTERM/SIGINT，干净退出");
                return Ok(());
            }
            match listener.accept() {
                Ok((stream, peer)) => {
                    sessions += 1;
                    let _ = stream.set_nonblocking(false);
                    log_info(
                        log,
                        "CONNECT",
                        &format!("peer={peer} session={sessions} uptime_s={}", started_at.elapsed().as_secs()),
                    );
                    write_status(
                        &cfg.status_path,
                        &Status {
                            pid,
                            listen: cfg.listen.clone(),
                            pair: &cfg.pair,
                            uptime_s: started_at.elapsed().as_secs(),
                            peer: Some(peer),
                            connected: true,
                            capture_fps: 0,
                            injected_total: 0,
                            rss_kb: rss_kb(),
                            last_error: "",
                        },
                    );
                    let outcome = run_session(
                        stream,
                        peer,
                        cfg,
                        log,
                        pid,
                        started_at,
                        Arc::clone(&counters),
                    );
                    if outcome.code != "E_PEER_EOF" && outcome.code != "E_STOP" {
                        log_error(log, outcome.code, &outcome.detail);
                    }
                    log_info(
                        log,
                        "DISCONNECT",
                        &format!("peer={peer} code={} {}", outcome.code, outcome.detail),
                    );
                    write_status(
                        &cfg.status_path,
                        &Status {
                            pid,
                            listen: cfg.listen.clone(),
                            pair: &cfg.pair,
                            uptime_s: started_at.elapsed().as_secs(),
                            peer: None,
                            connected: false,
                            capture_fps: 0,
                            injected_total: counters.injected.load(Ordering::Relaxed),
                            rss_kb: rss_kb(),
                            last_error: outcome.code,
                        },
                    );
                    last_idle = Instant::now();
                }
                Err(error) if error.kind() == ErrorKind::WouldBlock => {
                    if last_idle.elapsed() >= IDLE_INTERVAL {
                        last_idle = Instant::now();
                        log_info(
                            log,
                            "IDLE",
                            &format!(
                                "uptime_s={} rss_kb={} sessions={sessions}",
                                started_at.elapsed().as_secs(),
                                rss_kb()
                            ),
                        );
                        write_status(
                            &cfg.status_path,
                            &Status {
                                pid,
                                listen: cfg.listen.clone(),
                                pair: &cfg.pair,
                                uptime_s: started_at.elapsed().as_secs(),
                                peer: None,
                                connected: false,
                                capture_fps: 0,
                                injected_total: counters.injected.load(Ordering::Relaxed),
                                rss_kb: rss_kb(),
                                last_error: "",
                            },
                        );
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                Err(error) => {
                    log_error(log, "E_ACCEPT", &format!("accept 失败: {error}"));
                    std::thread::sleep(Duration::from_millis(500));
                }
            }
        }
    }

    // =====** 入口 =====**

    /// 进程入口：配置 → 日志/panic 钩子 → 常驻循环。
    pub fn main() -> i32 {
        install_signal_handlers();
        let cfg = match build_config() {
            Ok(cfg) => cfg,
            Err(error) => {
                eprintln!("参数/配置错误: {error}");
                print_usage();
                return 2;
            }
        };
        let _ = LOG_PATH.set(cfg.log_path.clone());
        install_panic_hook();
        let log = match Logger::open(&cfg.log_path) {
            Ok(logger) => Arc::new(Mutex::new(logger)),
            Err(error) => {
                eprintln!("日志初始化失败: {error}");
                return 2;
            }
        };
        let pid = std::process::id();
        let started_at = Instant::now();
        let banner = format!(
            "pair={} listen={} fps={} bitrate={} log={} status={}",
            cfg.pair,
            cfg.listen,
            cfg.fps,
            cfg.bitrate_bps,
            cfg.log_path.display(),
            cfg.status_path.display()
        );
        log_info(&log, "SERVE_START", &banner);
        println!("SERVE_START {banner}");

        match serve(&cfg, &log, pid, started_at) {
            Ok(()) => 0,
            Err(error) => {
                log_error(&log, "E_LISTEN", &error);
                eprintln!("服务失败: {error}");
                2
            }
        }
    }

    /// 把 panic 写进结构化日志（release 为 panic=abort，钩子仍会在 abort 前执行）。
    fn install_panic_hook() {
        let path = LOG_PATH.get().cloned();
        std::panic::set_hook(Box::new(move |info| {
            let message = format!("E_PANIC {info}");
            if let Some(path) = path.as_ref() {
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                {
                    let _ = writeln!(file, "{} ERROR {message}", timestamp());
                }
            }
            eprintln!("{message}");
        }));
    }
}
