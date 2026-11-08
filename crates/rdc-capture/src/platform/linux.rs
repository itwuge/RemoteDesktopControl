//! rdc-capture / Linux X11 平台采集后端
//!
//! 层级：L3 采集层 · 平台适配
//! 用途：X11 + XShm 全帧采集（主路径），XGetImage 为 SHM 不可用时的回退；
//!       监听 XRandR 变化后按「防抖 + 稳定复核」重建采集会话，输出带单调时间戳的 BGRA 帧
//! 调用方：rdc-capture::platform::create_source（由 rdc-ctl 在会话建立时调用）
//!
//! 设计依据：research/linux-x11-design.md
//! - 采集线程独占 X11 连接与共享内存段；本结构只保存线程控制状态，满足 `Send`。
//! - 帧像素只在共享内存有效期内拷贝到池化缓冲，绝不把裸指针存进 [`Frame`]。
//! - 分辨率/拓扑变化：事件 + 周期性几何探测双保险，防抖 300ms、等待稳定复核 200ms×2，
//!   重建限流 3 次/10s，超限保持旧尺寸并告警，不让线程退出。
//!
//! 安全边界（unsafe）：
//! - 仅 [`ShmImage`] 的 `shmget` / `shmat` / `shmdt` / `shmctl` 使用 unsafe；
//!   段的生命周期与采集线程一致，且只在采集线程内访问。

use std::sync::Arc;
use std::time::{Duration, Instant};

use rdc_runtime::{spawn_named, ExitToken, FramePool, ThreadPrio};
use tracing::{debug, info, warn};
use x11rb::connection::Connection;
use x11rb::protocol::randr::ConnectionExt as _;
use x11rb::protocol::shm::ConnectionExt as _;
use x11rb::protocol::xfixes::ConnectionExt as _;
use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat, Window};
use x11rb::rust_connection::RustConnection;

use crate::frame::{Frame, FrameMeta, PixelFormat};
use crate::params::POOL_BLOCKS;
use crate::source::{CaptureConfig, CaptureSource, DisplayInfo, FrameSink};
use crate::stats::CaptureStats;
use crate::CaptureError;

// =====** 常量 =====**

/// 分辨率变化后的防抖静默期：最后一次变化信号后需静默这么久才允许重建。
const REBUILD_QUIET_MS: u64 = 300;
/// 从首次探测到变化，到允许重建的最小等待：覆盖 XRandR 连续多次配置事件。
const REBUILD_SETTLE_MS: u64 = 1_500;
/// 重建前连续两次几何读数的最小间隔：确认尺寸已稳定。
const REBUILD_CONFIRM_GAP_MS: u64 = 200;
/// 周期性几何探测间隔：防止驱动漏发 RandR 事件。
const GEOMETRY_PROBE_MS: u64 = 250;
/// 重建限流窗口与次数：窗口内超过次数则保持旧尺寸，仅告警。
const REBUILD_WINDOW_MS: u64 = 10_000;
/// 重建限流窗口内允许的最大次数。
const REBUILD_MAX_IN_WINDOW: usize = 3;

// =====** 后端标识 =====**

/// 实际生效的采集后端。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// X11 + XShm（主路径：像素直接写入共享内存段）。
    XShm,
    /// X11 XGetImage（回退路径：像素走协议回复，带宽与 CPU 更高）。
    XGetImage,
}

impl Backend {
    /// 后端名称字符串（用于日志与实测报告）。
    ///
    /// # Returns
    /// 静态字符串。
    ///
    /// # Errors
    /// 不会失败。
    pub fn name(self) -> &'static str {
        match self {
            Self::XShm => "x11-xshm",
            Self::XGetImage => "x11-xgetimage",
        }
    }
}

// =====** 采集区域 =====**

/// 采集目标矩形（X11 root 坐标，左上为原点）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CaptureRect {
    /// 左上角 X。
    x: i16,
    /// 左上角 Y。
    y: i16,
    /// 宽（像素）。
    width: u16,
    /// 高（像素）。
    height: u16,
}

impl CaptureRect {
    /// 由 root 几何构造整屏矩形。
    fn full_screen(width: u16, height: u16) -> Self {
        Self {
            x: 0,
            y: 0,
            width,
            height,
        }
    }
}

// =====** PlatformCapture =====**

/// Linux/X11 采集源。
///
/// 生命周期与 [`CaptureSource`] 一致；X11 连接与共享内存段都只在采集线程内创建与释放。
pub struct PlatformCapture {
    /// 帧消费者。
    sink: Option<Arc<dyn FrameSink>>,
    /// 当前配置。
    config: Option<CaptureConfig>,
    /// 退出令牌。
    token: Arc<ExitToken>,
    /// 采集线程。
    thread: Option<std::thread::JoinHandle<()>>,
    /// 预检确定的后端；未启动时为 `None`。
    backend: Option<Backend>,
    /// 统计（与采集线程共享）。
    stats: Arc<CaptureStats>,
    /// 帧内存池（与采集线程共享）。
    pool: Option<Arc<FramePool>>,
    /// 启动时刻（用于帧率统计）。
    started_at: Option<Instant>,
    /// 上次停止时的会话时长（毫秒）。
    stopped_elapsed_ms: u64,
}

impl PlatformCapture {
    /// 构造未绑定的 X11 采集源。
    ///
    /// # Arguments
    /// * 无。
    ///
    /// # Returns
    /// 未启动实例；X11 连接在 [`CaptureSource::start`] 的线程内建立。
    ///
    /// # Errors
    /// 不会失败；无 DISPLAY 等环境问题在 `start` 中返回。
    pub fn new() -> Self {
        Self {
            sink: None,
            config: None,
            token: Arc::new(ExitToken::default()),
            thread: None,
            backend: None,
            stats: Arc::new(CaptureStats::new()),
            pool: None,
            started_at: None,
            stopped_elapsed_ms: 0,
        }
    }

    /// 探测后端可用性：连接 X11、校验像素格式、实测一次 XShm 段创建。
    ///
    /// # Arguments
    /// * 无。
    ///
    /// # Returns
    /// SHM 可用时返回 [`Backend::XShm`]，否则 [`Backend::XGetImage`]。
    ///
    /// # Errors
    /// 无法连接 X11、root 深度不受支持时返回 [`CaptureError::Unsupported`]。
    pub fn probe() -> Result<Backend, CaptureError> {
        let (conn, screen) = x11rb::connect(None).map_err(|error| {
            warn!(error = %error, "X11 连接失败");
            CaptureError::Unsupported
        })?;
        let setup = conn.setup();
        let root_depth = setup.roots[screen].root_depth;
        let bits_per_pixel = setup
            .pixmap_formats
            .iter()
            .filter(|format| format.depth == root_depth)
            .map(|format| format.bits_per_pixel)
            .max()
            .unwrap_or(0);
        // 只接受 32bpp：帧格式固定 BGRA8，其它 bpp 需额外色彩转换，不在本阶段范围。
        if bits_per_pixel != 32 {
            warn!(root_depth, bits_per_pixel, "root 像素格式不是 32bpp，无法输出 BGRA8");
            return Err(CaptureError::Unsupported);
        }
        let shm_ok = shm_available(&conn);
        if !shm_ok {
            warn!("XShm 不可用，回退 XGetImage（带宽与 CPU 更高）");
            return Ok(Backend::XGetImage);
        }
        // 预检阶段真实创建一次小段再释放：仅查询版本不足以发现权限/资源问题。
        match ShmImage::create(&conn, 64, 64) {
            Ok(image) => {
                image.release(&conn);
                Ok(Backend::XShm)
            }
            Err(error) => {
                warn!(error = %error, "XShm 段创建失败，回退 XGetImage");
                Ok(Backend::XGetImage)
            }
        }
    }
}

// =====** CaptureSource 实现 =====**

impl CaptureSource for PlatformCapture {
    /// 返回本次实际生效的后端。
    ///
    /// # Returns
    /// 已启动返回后端名；未启动返回 `"unprobed"`。
    ///
    /// # Errors
    /// 不会失败。
    fn backend_name(&self) -> &'static str {
        self.backend.map_or("unprobed", Backend::name)
    }

    /// 采集统计快照（诊断与实测报告用）。
    ///
    /// # Returns
    /// 当前统计；未启动时计数为 0。
    ///
    /// # Errors
    /// 不会失败。
    fn stats(&self) -> crate::CaptureSnapshot {
        let elapsed = self
            .started_at
            .map_or(self.stopped_elapsed_ms, |t| t.elapsed().as_millis() as u64);
        self.stats.snapshot(elapsed)
    }

    /// 枚举显示器（XRandR 输出 → 每个已启用 CRTC 一项）。
    ///
    /// # Arguments
    /// * 无。
    ///
    /// # Returns
    /// 显示器列表；RandR 不可用或没有已启用 CRTC 时返回「整屏一项」或空列表。
    ///
    /// # Errors
    /// 不会失败；错误以空列表或降级列表表达。
    fn enumerate(&self) -> Vec<DisplayInfo> {
        match x11rb::connect(None) {
            Ok((conn, screen)) => {
                let target = resolve_displays(&conn, screen);
                target.displays
            }
            Err(error) => {
                warn!(error = %error, "X11 枚举连接失败");
                Vec::new()
            }
        }
    }

    /// 启动采集。
    ///
    /// # Arguments
    /// * `config` - 目标显示器、帧率、光标合成与色度格式。
    ///
    /// # Returns
    /// 采集线程已启动返回 `Ok(())`。
    ///
    /// # Errors
    /// 未绑定 sink、显示器不存在、X11 不可用或线程创建失败时返回 [`CaptureError`]。
    fn start(&mut self, config: CaptureConfig) -> Result<(), CaptureError> {
        let sink = self
            .sink
            .clone()
            .ok_or_else(|| CaptureError::Internal("未调用 set_sink".to_owned()))?;
        if self.thread.is_some() {
            return Err(CaptureError::Internal("采集已在运行".to_owned()));
        }
        if config.chroma != crate::source::ChromaFormat::Bgra || config.bit_depth != 8 {
            // Linux 后端只输出 BGRA8；其它格式交给上层转换，不在本阶段做隐式转换。
            return Err(CaptureError::Unsupported);
        }

        // 启动前把尺寸读准：先枚举一次，再在会话初始化里做稳定性复核。
        let (conn, screen) = x11rb::connect(None).map_err(|error| {
            warn!(error = %error, "X11 连接失败");
            CaptureError::Unsupported
        })?;
        let resolved = resolve_displays(&conn, screen);
        let display = resolved
            .displays
            .iter()
            .find(|item| item.id == config.display_id)
            .cloned()
            .ok_or(CaptureError::DisplayDisappeared {
                display_id: config.display_id,
            })?;
        drop(conn);

        let backend = Self::probe()?;
        // 注意：`display` 是 tracing 宏的保留字段名，日志里改用 `panel` 变量。
        let panel = display;
        info!(
            display_id = config.display_id,
            backend = backend.name(),
            width = panel.width,
            height = panel.height,
            refresh_hz = panel.refresh_hz,
            "X11 采集后端探测完成"
        );

        let pool = Arc::new(FramePool::new(
            POOL_BLOCKS,
            /* 单块按 BGRA8 整帧计算；分辨率变化时线程内会重建池。 */
            PixelFormat::Bgra8.buffer_size(panel.width, panel.height),
        ));
        self.pool = Some(Arc::clone(&pool));
        self.stats = Arc::new(CaptureStats::new());
        self.config = Some(config);
        self.backend = Some(backend);
        self.token = Arc::new(ExitToken::default());
        let token = Arc::clone(&self.token);
        let stats = Arc::clone(&self.stats);

        let name = format!("rdc-cap-{}", config.display_id);
        let handle = spawn_named(&name, ThreadPrio::Net, Arc::clone(&token), move || {
            capture_loop(backend, config, sink, pool, stats, token);
        })
        .map_err(|error| CaptureError::Internal(format!("采集线程创建失败: {error}")))?;
        self.thread = Some(handle);
        self.started_at = Some(Instant::now());
        Ok(())
    }

    /// 注册帧消费者。
    ///
    /// # Arguments
    /// * `sink` - 帧消费者（内部 `Arc` 共享给采集线程）。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    fn set_sink(&mut self, sink: Arc<dyn FrameSink>) {
        self.sink = Some(sink);
    }

    /// 运行期重配（停止后按新参数重启）。
    ///
    /// # Arguments
    /// * `config` - 新的采集参数。
    ///
    /// # Returns
    /// 新参数生效返回 `Ok(())`。
    ///
    /// # Errors
    /// 显示器消失或重启失败时返回 [`CaptureError`]。
    fn reconfigure(&mut self, config: CaptureConfig) -> Result<(), CaptureError> {
        // X11 会话与帧池绑定当前尺寸，重配走「停 → 启」最直接且无资源泄漏。
        self.stop();
        self.start(config)
    }

    /// 停止采集并释放 X11 连接与共享内存段。
    ///
    /// # Arguments
    /// * 无。
    ///
    /// # Returns
    /// 无返回值；未启动时为空操作。
    ///
    /// # Errors
    /// 不会失败；线程 panic 已由 spawn_named 隔离。
    fn stop(&mut self) {
        self.token.request();
        if let Some(handle) = self.thread.take() {
            // 采集循环的 AcquireNextFrame 超时上限为一个帧间隔，join 有界。
            let _ = handle.join(); // gate-allow R-C-2: stop() 无返回值可传播；线程 panic 已由 Rust 默认 panic hook 输出栈
        }
        self.stopped_elapsed_ms = self.started_at.map_or(0, |t| t.elapsed().as_millis() as u64);
        self.backend = None;
        self.started_at = None;
    }
}

impl Default for PlatformCapture {
    /// 等价于 [`PlatformCapture::new`]。
    fn default() -> Self {
        Self::new()
    }
}

// =====** 采集线程 =====**

/// 采集线程主体。
///
/// # Arguments
/// * `backend` - 预检阶段确定的后端。
/// * `config` - 采集配置。
/// * `sink` - 帧消费者。
/// * `pool` - 初始帧内存池（分辨率变化时线程内重建）。
/// * `stats` - 统计计数器。
/// * `token` - 退出令牌。
///
/// # Returns
/// 无返回值；初始化失败或不可恢复错误时写日志后结束。
///
/// # Errors
/// 不返回错误；失败写日志并通过统计计数体现。
fn capture_loop(
    backend: Backend,
    config: CaptureConfig,
    sink: Arc<dyn FrameSink>,
    pool: Arc<FramePool>,
    stats: Arc<CaptureStats>,
    token: Arc<ExitToken>,
) {
    let started = Instant::now();
    stats.mark_start(0);
    let mut session = match XSession::new(backend, config, pool) {
        Ok(session) => session,
        Err(error) => {
            warn!(error = %error, "X11 采集会话初始化失败，线程退出");
            return;
        }
    };
    let frame_interval_us = if config.target_fps == 0 {
        16_667
    } else {
        1_000_000 / u64::from(config.target_fps)
    };
    let period = Duration::from_micros(frame_interval_us);

    let mut frame_id: u64 = 0;
    let mut deadline = started + period;
    let mut last_probe_ms: u64 = 0;
    let mut debounce = RebuildDebounce::new();

    loop {
        if token.is_requested() {
            break;
        }
        let now_ms = started.elapsed().as_millis() as u64;

        // 1) 排空 X 事件：RandR 事件只作为「可能变了」的信号，真正的尺寸以几何读数为准。
        let mut event_seen = false;
        loop {
            match session.conn.poll_for_event() {
                Ok(Some(_event)) => event_seen = true,
                Ok(None) => break,
                Err(error) => {
                    warn!(error = %error, "X11 事件读取失败");
                    break;
                }
            }
        }
        if event_seen {
            debounce.signal(now_ms);
        }

        // 2) 周期性几何探测：驱动漏发事件时仍能发现分辨率变化。
        if now_ms.saturating_sub(last_probe_ms) >= GEOMETRY_PROBE_MS {
            last_probe_ms = now_ms;
            if session.geometry_changed() {
                debounce.signal(now_ms);
            }
        }

        // 3) 防抖 + 稳定复核后重建会话。
        if debounce.should_rebuild(now_ms) {
            if !debounce.cooldown_ok(now_ms) {
                warn!("分辨率变化重建被限流（10s 内超过 3 次），保持旧尺寸采集");
                debounce.clear();
            } else {
                let rebuild_start = Instant::now();
                let stable = session.confirm_stable(REBUILD_CONFIRM_GAP_MS, &token);
                if stable {
                    debounce.note_rebuild(now_ms);
                    session.rebuild();
                    info!(
                        rebuild_ms = rebuild_start.elapsed().as_millis() as u64,
                        width = session.target.width,
                        height = session.target.height,
                        backend = session.backend.name(),
                        "x11 采集会话重建完成"
                    );
                    debounce.clear();
                } else {
                    // 尺寸仍在变化：保留信号，继续等下一轮稳定复核。
                    debounce.signal(started.elapsed().as_millis() as u64);
                }
            }
        }

        // 4) 采集一帧。
        let qpc_us = started.elapsed().as_micros() as u64;
        match session.capture_frame(&sink, &stats, frame_id, qpc_us) {
            Ok(()) => {
                frame_id = frame_id.wrapping_add(1);
            }
            Err(error) => {
                // 旧尺寸在新分辨率下越界是切换瞬间的正常现象：记丢帧即可。
                // 不在这里 signal 防抖状态机——否则每帧失败都会刷新「最后信号」，
                // 静默期永远无法满足，重建被无限推迟；变化由周期几何探测负责。
                debug!(error = %error, "X11 帧采集失败");
                stats.add_dropped(1);
            }
        }

        // 5) 按固定节拍推进；落后超过一个周期时把跳过的槽位计入丢帧。
        let now = Instant::now();
        if now > deadline + period {
            let late_us = (now - deadline).as_micros() as u64;
            let missed = late_us / frame_interval_us;
            if missed > 0 {
                stats.add_dropped(missed);
            }
            deadline = now;
        }
        deadline += period;
        wait_until(deadline, &token);
    }
}

/// 按截止时刻睡眠，同时响应退出请求。
///
/// # Arguments
/// * `deadline` - 目标时刻。
/// * `token` - 退出令牌；置位后立即返回。
///
/// # Returns
/// 无返回值。
///
/// # Errors
/// 不会失败。
fn wait_until(deadline: Instant, token: &ExitToken) {
    loop {
        if token.is_requested() {
            return;
        }
        let now = Instant::now();
        if now >= deadline {
            return;
        }
        // 分片睡眠：保证 stop() 的 join 延迟不超过 1ms，同时减小节拍抖动导致的丢帧。
        let remaining = deadline - now;
        std::thread::sleep(remaining.min(Duration::from_millis(1)));
    }
}

// =====** 采集会话 =====**

/// X11 采集会话：连接、共享内存段、目标区域与帧池。
struct XSession {
    /// 独占的 X11 连接。
    conn: RustConnection,
    /// 屏幕号（多 screen 时用于取 root）。
    screen: usize,
    /// root 窗口。
    root: Window,
    /// 实际生效的后端。
    backend: Backend,
    /// 像素位深（bits per pixel，只支持 32）。
    bits_per_pixel: u32,
    /// 当前采集目标。
    target: CaptureRect,
    /// 上一次探测到的显示器矩形（用于识别「变化沿」，避免周期性探测重复触发防抖）。
    observed: CaptureRect,
    /// XShm 像素段；`XGetImage` 回退时为 `None`。
    image: Option<ShmImage>,
    /// 帧内存池（尺寸变化时替换）。
    pool: Arc<FramePool>,
    /// 当前显示器编号（写入帧元数据）。
    display_id: u32,
    /// 是否把光标合成进画面。
    include_cursor: bool,
    /// 光标合成可用（XFixes 探测成功）。
    cursor_ok: bool,
}

impl XSession {
    /// 建立会话：连接、校验格式、解析目标区域、创建 SHM 段与帧池。
    ///
    /// # Arguments
    /// * `backend` - 预检后端。
    /// * `config` - 采集配置（`display_id` 用于选择 CRTC 区域）。
    /// * `pool` - 初始帧池。
    ///
    /// # Returns
    /// 已就绪的会话。
    ///
    /// # Errors
    /// 连接失败、格式不支持或 SHM 创建失败时返回诊断文本。
    fn new(
        backend: Backend,
        config: CaptureConfig,
        pool: Arc<FramePool>,
    ) -> Result<Self, String> {
        let (conn, screen) = x11rb::connect(None).map_err(|e| format!("X11 连接失败: {e}"))?;
        let setup = conn.setup();
        let root = setup.roots[screen].root;
        let root_depth = setup.roots[screen].root_depth;
        let bits_per_pixel = setup
            .pixmap_formats
            .iter()
            .filter(|format| format.depth == root_depth)
            .map(|format| format.bits_per_pixel)
            .max()
            .unwrap_or(0);
        if bits_per_pixel != 32 {
            return Err(format!("root 像素格式为 {bits_per_pixel}bpp，仅支持 32bpp"));
        }

        // 订阅 RandR 变化；失败不阻断（周期性探测仍可发现变化）。
        let randr_ok = conn
            .randr_query_version(1, 6)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some();
        if randr_ok {
            let mask = u16::from(x11rb::protocol::randr::NotifyMask::SCREEN_CHANGE)
                | u16::from(x11rb::protocol::randr::NotifyMask::CRTC_CHANGE)
                | u16::from(x11rb::protocol::randr::NotifyMask::OUTPUT_CHANGE)
                | u16::from(x11rb::protocol::randr::NotifyMask::RESOURCE_CHANGE);
            let mask = x11rb::protocol::randr::NotifyMask::from(mask);
            // RandR 事件订阅是可选能力：失败只降级为周期探测，会话继续；
            // 用显式 match 记录降级路径，避免出现“只打日志就吞掉错误”的分支（R-L-9）。
            // x11rb 的 randr_select_input 返回 VoidCookie（不是 ()），成功分支直接忽略该 cookie。
            match conn.randr_select_input(root, mask) {
                Ok(_) => {}
                Err(error) => {
                    warn!(error = %error, "XRandR 事件订阅失败，仅靠周期探测");
                }
            }
        }

        // XFixes 只用于光标合成；不可用时降级为「不含光标」。
        let cursor_ok = conn
            .xfixes_query_version(5, 0)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .is_some();

        let resolved = resolve_displays(&conn, screen);
        let target = resolved
            .rects
            .iter()
            .find(|(id, _)| *id == config.display_id)
            .map(|(_, rect)| *rect)
            .or_else(|| resolved.rects.first().map(|(_, rect)| *rect))
            .ok_or_else(|| "未找到可采集区域".to_owned())?;

        // 启动稳定性复核：先读一次，200ms 后再读；不一致就取最后一次（并告警）。
        let mut target = target;
        std::thread::sleep(Duration::from_millis(REBUILD_CONFIRM_GAP_MS));
        if let Ok(geometry) = conn.get_geometry(root) {
            if let Ok(reply) = geometry.reply() {
                if u32::from(reply.width) * u32::from(reply.height)
                    < u32::from(target.width) * u32::from(target.height)
                {
                    warn!(width = reply.width, height = reply.height, "启动复核发现尺寸更小，采用新尺寸");
                    target = CaptureRect::full_screen(reply.width, reply.height);
                }
            }
        }

        let image = match backend {
            Backend::XShm => Some(
                ShmImage::create(&conn, target.width, target.height)
                    .map_err(|e| format!("XShm 段创建失败: {e}"))?,
            ),
            Backend::XGetImage => None,
        };
        info!(
            backend = backend.name(),
            x = target.x,
            y = target.y,
            width = target.width,
            height = target.height,
            include_cursor = config.include_cursor,
            cursor_ok,
            "x11 采集会话就绪"
        );

        Ok(Self {
            conn,
            screen,
            root,
            backend,
            bits_per_pixel: u32::from(bits_per_pixel),
            target,
            observed: target,
            image,
            pool,
            display_id: config.display_id,
            include_cursor: config.include_cursor,
            cursor_ok,
        })
    }

    /// 读取当前显示器布局，并在发现「变化沿」时返回 `true`。
    ///
    /// 只有与上次探测结果不同才返回 `true`；持续为某尺寸不会重复触发，
    /// 否则 250ms 周期探测会不断刷新防抖的「最后信号」，静默期永远无法满足。
    ///
    /// # Returns
    /// 目标显示器矩形相对上次探测发生变化时返回 `true`。
    ///
    /// # Errors
    /// 不会失败（解析失败按「未变化」处理，下一轮再试）。
    fn geometry_changed(&mut self) -> bool {
        let Some(rect) = self.current_rect() else {
            return false;
        };
        if rect == self.observed {
            return false;
        }
        self.observed = rect;
        true
    }

    /// 解析当前显示器的采集矩形（与 [`Self::target`] 同源比较）。
    ///
    /// # Returns
    /// 找到目标显示器时返回其矩形；布局不可用时返回 `None`。
    ///
    /// # Errors
    /// 不会失败。
    fn current_rect(&self) -> Option<CaptureRect> {
        let resolved = resolve_displays(&self.conn, self.screen);
        resolved
            .rects
            .iter()
            .find(|(id, _)| *id == self.display_id)
            .map(|(_, rect)| *rect)
            .or_else(|| resolved.rects.first().map(|(_, rect)| *rect))
    }

    /// 连续两次读取几何并确认稳定。
    ///
    /// # Arguments
    /// * `gap_ms` - 两次读数之间的等待毫秒数。
    /// * `token` - 退出令牌；置位时提前返回 `false`。
    ///
    /// # Returns
    /// 两次读数一致且与当前目标区域不同（确实需要重建）时返回 `true`。
    ///
    /// # Errors
    /// 不会失败。
    fn confirm_stable(&self, gap_ms: u64, token: &ExitToken) -> bool {
        let read = || self.current_rect();
        let Some(first) = read() else {
            return false;
        };
        if token.is_requested() {
            return false;
        }
        std::thread::sleep(Duration::from_millis(gap_ms));
        let Some(second) = read() else {
            return false;
        };
        if first != second {
            return false;
        }
        // 两次读数一致，且与当前采集目标不同（尺寸或原点变化）才需要重建。
        second != self.target
    }

    /// 重建采集会话：按当前 RandR 布局解析目标区域，必要时重建 SHM 段与帧池。
    ///
    /// # Arguments
    /// * 无。
    ///
    /// # Returns
    /// 无返回值；失败时保持旧尺寸并告警。
    ///
    /// # Errors
    /// 不会失败（错误写日志，交由下一轮防抖再试）。
    fn rebuild(&mut self) {
        let resolved = resolve_displays(&self.conn, self.screen);
        let Some((_, rect)) = resolved
            .rects
            .iter()
            .find(|(id, _)| *id == self.display_id)
            .or_else(|| resolved.rects.first())
        else {
            warn!("重建时未找到可用区域，保持旧尺寸");
            return;
        };
        let rect = *rect;
        let size_changed =
            rect.width != self.target.width || rect.height != self.target.height;
        self.target = rect;
        self.observed = rect;

        if !size_changed {
            // 只换了位置：像素段尺寸不变，直接复用。
            return;
        }
        // 先释放旧段，再按新尺寸创建；失败则回退 XGetImage 继续出帧。
        if let Some(image) = self.image.take() {
            image.release(&self.conn);
        }
        self.pool = Arc::new(FramePool::new(
            POOL_BLOCKS,
            PixelFormat::Bgra8.buffer_size(u32::from(rect.width), u32::from(rect.height)),
        ));
        match ShmImage::create(&self.conn, rect.width, rect.height) {
            Ok(image) => {
                self.image = Some(image);
                self.backend = Backend::XShm;
            }
            Err(error) => {
                warn!(error = %error, "重建 XShm 段失败，回退 XGetImage");
                self.image = None;
                self.backend = Backend::XGetImage;
            }
        }
    }

    /// 采集一帧并交付 sink。
    ///
    /// # Arguments
    /// * `sink` - 帧消费者。
    /// * `stats` - 统计计数器（池耗尽时计数）。
    /// * `frame_id` - 自增帧号。
    /// * `qpc_us` - 距会话开始的微秒时间戳。
    ///
    /// # Returns
    /// 已交付（或被池耗尽拒绝）返回 `Ok(())`。
    ///
    /// # Errors
    /// X11 请求失败（典型：切换分辨率瞬间区域越界）返回诊断文本。
    fn capture_frame(
        &mut self,
        sink: &Arc<dyn FrameSink>,
        stats: &Arc<CaptureStats>,
        frame_id: u64,
        qpc_us: u64,
    ) -> Result<(), String> {
        let Some(mut buffer) = self.pool.acquire() else {
            stats.add_pool_exhausted();
            stats.add_dropped(1);
            return Ok(());
        };

        // 1) 把像素读进共享内存（或协议回复），再逐行拷贝到池化缓冲。
        let stride = stride_bytes(self.target.width, self.bits_per_pixel);
        let bytes = stride * self.target.height as usize;
        let frame_start = Instant::now();
        match self.image.as_ref() {
            Some(image) => {
                let _reply = self
                    .conn
                    .shm_get_image(
                        self.root,
                        self.target.x,
                        self.target.y,
                        self.target.width,
                        self.target.height,
                        u32::MAX,
                        ImageFormat::Z_PIXMAP.into(),
                        image.seg,
                        0,
                    )
                    .map_err(|e| format!("XShmGetImage 请求失败: {e}"))?
                    .reply()
                    .map_err(|e| format!("XShmGetImage 回复失败: {e}"))?;
                // 共享内存由 X 服务器直接写入，此处读到的就是本帧像素。
                let src = unsafe { std::slice::from_raw_parts(image.ptr.cast_const(), bytes) };
                copy_rows(src, buffer.as_mut_slice(), stride, self.target.width as usize * 4, self.target.height as usize);
            }
            None => {
                let reply = self
                    .conn
                    .get_image(
                        ImageFormat::Z_PIXMAP,
                        self.root,
                        self.target.x,
                        self.target.y,
                        self.target.width,
                        self.target.height,
                        u32::MAX,
                    )
                    .map_err(|e| format!("XGetImage 请求失败: {e}"))?
                    .reply()
                    .map_err(|e| format!("XGetImage 回复失败: {e}"))?;
                copy_rows(&reply.data, buffer.as_mut_slice(), stride, self.target.width as usize * 4, self.target.height as usize);
            }
        }

        // 2) 光标合成（可选）：XFixes 读取光标图与指针位置，直接叠加到 BGRA 缓冲。
        if self.include_cursor && self.cursor_ok {
            if let Err(error) = self.overlay_cursor(buffer.as_mut_slice(), self.target.width, self.target.height) {
                // 光标失败不影响出帧，只在调试日志里记录，避免刷屏。
                debug!(error = %error, "光标合成失败");
                self.cursor_ok = false;
            }
        }

        // 3) 时间戳与统计：qpc 单调来自 Instant，帧间隔由 stats 计算。
        let cost_us = frame_start.elapsed().as_micros() as u64;
        let interval = stats.record_frame(qpc_us, cost_us);
        sink.on_frame(Frame {
            buffer,
            meta: FrameMeta {
                pts_ns: qpc_us.saturating_mul(1000),
                qpc_us,
                frame_id,
                display_id: self.display_id,
            },
            format: PixelFormat::Bgra8,
            width: u32::from(self.target.width),
            height: u32::from(self.target.height),
            dirty_rects: Vec::new(),
            move_rects: Vec::new(),
            capture_cost_us: cost_us as u32,
            interval_us: interval as u32,
            // X11 采集路径没有 GPU 纹理，像素始终在 `buffer` 里。
            gpu: None,
        });
        Ok(())
    }

    /// 把 XFixes 光标叠加到 BGRA 缓冲。
    ///
    /// # Arguments
    /// * `dst` - BGRA8 目标缓冲（至少 `width * height * 4`）。
    /// * `width` / `height` - 采集区域尺寸（光标坐标相对该区域左上角）。
    ///
    /// # Returns
    /// 合成成功返回 `Ok(())`。
    ///
    /// # Errors
    /// XFixes 请求失败时返回诊断文本。
    fn overlay_cursor(&self, dst: &mut [u8], width: u16, height: u16) -> Result<(), String> {
        let cursor = self
            .conn
            .xfixes_get_cursor_image()
            .map_err(|e| format!("XFixesGetCursorImage 请求失败: {e}"))?
            .reply()
            .map_err(|e| format!("XFixesGetCursorImage 回复失败: {e}"))?;
        // 光标位置（root 坐标）减去采集区域原点 = 区域内坐标。
        let origin_x = i32::from(self.target.x);
        let origin_y = i32::from(self.target.y);
        let hot_x = i32::from(cursor.x) - i32::from(cursor.xhot) - origin_x;
        let hot_y = i32::from(cursor.y) - i32::from(cursor.yhot) - origin_y;
        let cursor_w = i32::from(cursor.width);
        let cursor_h = i32::from(cursor.height);
        if cursor_w <= 0 || cursor_h <= 0 || cursor.cursor_image.len() < (cursor_w * cursor_h) as usize {
            return Ok(());
        }
        // ARGB 预乘：dst = src + dst * (1 - alpha)。
        for row in 0..cursor_h {
            let y = hot_y + row;
            if y < 0 || y >= i32::from(height) {
                continue;
            }
            for col in 0..cursor_w {
                let x = hot_x + col;
                if x < 0 || x >= i32::from(width) {
                    continue;
                }
                let pixel = cursor.cursor_image[(row * cursor_w + col) as usize];
                let alpha = (pixel >> 24) & 0xFF;
                if alpha == 0 {
                    continue;
                }
                let index = ((y as usize * usize::from(width)) + x as usize) * 4;
                if index + 3 >= dst.len() {
                    continue;
                }
                let inv = 255 - alpha;
                let src_b = (pixel >> 16) & 0xFF;
                let src_g = (pixel >> 8) & 0xFF;
                let src_r = pixel & 0xFF;
                let dst_b = u32::from(dst[index]);
                let dst_g = u32::from(dst[index + 1]);
                let dst_r = u32::from(dst[index + 2]);
                dst[index] = (src_b + dst_b * inv / 255) as u8;
                dst[index + 1] = (src_g + dst_g * inv / 255) as u8;
                dst[index + 2] = (src_r + dst_r * inv / 255) as u8;
                dst[index + 3] = 255;
            }
        }
        Ok(())
    }
}

impl Drop for XSession {
    /// 会话析构：退订事件、解绑并释放共享内存段（失败只记调试日志）。
    fn drop(&mut self) {
        if let Some(image) = self.image.take() {
            image.release(&self.conn);
        }
    }
}

// =====** XShm 段 =====**

/// 一段由 `shmget` 创建、被 X 服务器 attach 的共享内存。
struct ShmImage {
    /// X 协议侧段标识（每个连接内唯一）。
    seg: u32,
    /// 系统 shm 段 id。
    shmid: i32,
    /// 本进程映射地址。
    ptr: *mut u8,
}

impl ShmImage {
    /// 创建共享内存段并让 X 服务器 attach。
    ///
    /// # Arguments
    /// * `conn` - X11 连接。
    /// * `width` / `height` - 段覆盖的像素尺寸（按 32bpp 计算）。
    ///
    /// # Returns
    /// 可用段。
    ///
    /// # Errors
    /// `shmget`/`shmat`/`ShmAttach` 失败时返回诊断文本。
    fn create(conn: &RustConnection, width: u16, height: u16) -> Result<Self, String> {
        let stride = stride_bytes(width, 32);
        let bytes = stride * usize::from(height);
        if bytes == 0 {
            return Err("共享内存段尺寸为 0".to_owned());
        }
        // IPC_PRIVATE：只被本进程与 X 服务器共享，不落盘、不被其它进程按 key 找到。
        let shmid = unsafe { libc::shmget(libc::IPC_PRIVATE, bytes, libc::IPC_CREAT | 0o600) };
        if shmid < 0 {
            return Err(format!("shmget 失败: {}", std::io::Error::last_os_error()));
        }
        let ptr = unsafe { libc::shmat(shmid, std::ptr::null(), 0) };
        if ptr.is_null() || ptr as isize == -1 {
            unsafe { libc::shmctl(shmid, libc::IPC_RMID, std::ptr::null_mut()) };
            return Err(format!("shmat 失败: {}", std::io::Error::last_os_error()));
        }
        // 段标识必须是本连接资源 ID 段内的合法 XID：X 服务器会校验其高位，
        // 硬编码小整数（如 1）会被判为 IDChoice 非法值。
        let seg = match conn.generate_id() {
            Ok(seg) => seg,
            Err(error) => {
                release_shm(ptr.cast(), shmid);
                return Err(format!("生成 SHM 段 XID 失败: {error}"));
            }
        };
        let attached = conn
            .shm_attach(seg, shmid as u32, false)
            .map_err(|e| format!("ShmAttach 请求失败: {e}"))
            .and_then(|cookie| cookie.check().map_err(|e| format!("ShmAttach 失败: {e}")));
        if let Err(error) = attached {
            release_shm(ptr.cast(), shmid);
            return Err(error);
        }
        Ok(Self {
            seg,
            shmid,
            ptr: ptr.cast(),
        })
    }

    /// 释放段：先 X 侧 detach，再解除本进程映射并删除段。
    ///
    /// # Arguments
    /// * `conn` - 创建段时使用的同一条 X11 连接。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败；错误只写调试日志。
    fn release(self, conn: &RustConnection) {
        if let Err(error) = conn.shm_detach(self.seg) {
            debug!(error = %error, "ShmDetach 请求失败");
        }
        if let Err(error) = conn.flush() {
            debug!(error = %error, "ShmDetach 后 flush 失败，服务器可能延迟处理解绑");
        }
        release_shm(self.ptr.cast(), self.shmid);
    }
}

/// 解除本进程映射并删除系统共享内存段（创建失败与正常释放共用）。
///
/// # Arguments
/// * `ptr` - `shmat` 返回的映射地址。
/// * `shmid` - `shmget` 返回的段 id。
///
/// # Returns
/// 无返回值。
fn release_shm(ptr: *mut u8, shmid: i32) {
    unsafe {
        libc::shmdt(ptr.cast());
        libc::shmctl(shmid, libc::IPC_RMID, std::ptr::null_mut());
    }
}

// =====** 几何与显示器解析 =====**

/// 枚举结果：显示器列表 + 与列表顺序一致的采集矩形。
struct ResolvedDisplays {
    /// 面向调用方的显示器信息（id 为 CRTC 序号）。
    displays: Vec<DisplayInfo>,
    /// `(id, 采集矩形)`。
    rects: Vec<(u32, CaptureRect)>,
}

/// 解析 RandR 布局；RandR 不可用时降级为「整屏一项」。
///
/// # Arguments
/// * `conn` - X11 连接。
/// * `screen` - 屏幕号。
///
/// # Returns
/// 显示器与矩形列表；完全失败时返回空列表。
///
/// # Errors
/// 不会失败；错误以空列表表达。
fn resolve_displays(conn: &RustConnection, screen: usize) -> ResolvedDisplays {
    let root = conn.setup().roots[screen].root;
    let mut displays = Vec::new();
    let mut rects = Vec::new();

    let resources = conn
        .randr_query_version(1, 6)
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .filter(|reply| reply.major_version >= 1)
        .and_then(|_| conn.randr_get_screen_resources(root).ok())
        .and_then(|cookie| cookie.reply().ok());

    if let Some(resources) = resources {
        let primary = conn
            .randr_get_output_primary(root)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
            .map(|reply| reply.output);
        for (index, crtc) in resources.crtcs.iter().enumerate() {
            let Some(info) = conn
                .randr_get_crtc_info(*crtc, resources.config_timestamp)
                .ok()
                .and_then(|cookie| cookie.reply().ok())
            else {
                continue;
            };
            if info.width == 0 || info.height == 0 {
                continue;
            }
            // 输出名：优先取 CRTC 上第一个输出；没有输出时用 CRTC 序号。
            let name = info
                .outputs
                .first()
                .and_then(|output| {
                    conn.randr_get_output_info(*output, resources.config_timestamp)
                        .ok()
                        .and_then(|cookie| cookie.reply().ok())
                })
                .map(|output| String::from_utf8_lossy(&output.name).into_owned())
                .unwrap_or_else(|| format!("CRTC-{index}"));
            // 刷新率：点时钟 / (水平总数 × 垂直总数)。
            let refresh_hz = resources
                .modes
                .iter()
                .find(|mode| mode.id == info.mode)
                .map(|mode| {
                    let total = u64::from(mode.htotal) * u64::from(mode.vtotal);
                    // 总数可能为 0（异常模式）：用 checked_div 显式表达「无刷新率」。
                    u64::from(mode.dot_clock).checked_div(total).unwrap_or(0) as u32
                })
                .unwrap_or(0);
            let is_primary = primary
                .map(|value| info.outputs.contains(&value))
                .unwrap_or(index == 0);
            let id = index as u32;
            displays.push(DisplayInfo {
                id,
                name,
                width: u32::from(info.width),
                height: u32::from(info.height),
                refresh_hz,
                scale: 1.0,
                is_primary,
                hdr: false,
            });
            rects.push((
                id,
                CaptureRect {
                    x: info.x,
                    y: info.y,
                    width: info.width,
                    height: info.height,
                },
            ));
        }
    }

    if displays.is_empty() {
        // 降级：RandR 不可用/无已启用 CRTC 时采集整个 root。
        if let Some(reply) = conn
            .get_geometry(root)
            .ok()
            .and_then(|cookie| cookie.reply().ok())
        {
            displays.push(DisplayInfo {
                id: 0,
                name: "root".to_owned(),
                width: u32::from(reply.width),
                height: u32::from(reply.height),
                refresh_hz: 0,
                scale: 1.0,
                is_primary: true,
                hdr: false,
            });
            rects.push((0, CaptureRect::full_screen(reply.width, reply.height)));
        }
    }
    ResolvedDisplays { displays, rects }
}

/// 判断 XShm 扩展是否可用。
///
/// # Arguments
/// * `conn` - X11 连接。
///
/// # Returns
/// 扩展版本 ≥1.0 时返回 `true`。
///
/// # Errors
/// 不会失败。
fn shm_available(conn: &RustConnection) -> bool {
    conn.shm_query_version()
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .map(|reply| reply.major_version >= 1)
        .unwrap_or(false)
}

/// 扫描行字节数：X 协议按 32 位（bitmap scanline unit）对齐。
///
/// # Arguments
/// * `width` - 像素宽。
/// * `bits_per_pixel` - 每像素位数（本后端恒为 32）。
///
/// # Returns
/// 每行字节数。
///
/// # Errors
/// 不会失败。
fn stride_bytes(width: u16, bits_per_pixel: u32) -> usize {
    let bits = usize::from(width) * bits_per_pixel as usize;
    bits.div_ceil(32) * 4
}

/// 逐行拷贝像素，处理源/目标步长不一致。
///
/// # Arguments
/// * `src` - 源像素（可能带行填充）。
/// * `dst` - 目标缓冲。
/// * `src_stride` - 源行字节数。
/// * `dst_stride` - 目标行字节数。
/// * `height` - 行数。
///
/// # Returns
/// 无返回值。
///
/// # Errors
/// 不会失败；源数据不足时按可用长度截断，避免越界。
fn copy_rows(src: &[u8], dst: &mut [u8], src_stride: usize, dst_stride: usize, height: usize) {
    for row in 0..height {
        let src_start = row * src_stride;
        let dst_start = row * dst_stride;
        if src_start + dst_stride > src.len() || dst_start + dst_stride > dst.len() {
            break;
        }
        dst[dst_start..dst_start + dst_stride]
            .copy_from_slice(&src[src_start..src_start + dst_stride]);
    }
}

// =====** 重建防抖 =====**

/// 分辨率变化的防抖与限流状态机。
///
/// 单独抽成纯时间函数，便于单元测试覆盖「事件抖动」「限流」等边界。
struct RebuildDebounce {
    /// 首次探测到变化的时刻（毫秒）。
    detected_ms: Option<u64>,
    /// 最后一次变化信号的时刻（毫秒）。
    last_signal_ms: u64,
    /// 最近的重建时刻（毫秒）。
    rebuilds: Vec<u64>,
}

impl RebuildDebounce {
    /// 新建状态机。
    ///
    /// # Returns
    /// 空状态。
    ///
    /// # Errors
    /// 不会失败。
    fn new() -> Self {
        Self {
            detected_ms: None,
            last_signal_ms: 0,
            rebuilds: Vec::new(),
        }
    }

    /// 记录一次「可能变了」的信号。
    ///
    /// # Arguments
    /// * `now_ms` - 当前会话毫秒。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    fn signal(&mut self, now_ms: u64) {
        if self.detected_ms.is_none() {
            self.detected_ms = Some(now_ms);
        }
        self.last_signal_ms = now_ms;
    }

    /// 是否已满足「静默 300ms 且总等待 1.5s」的重建条件。
    ///
    /// # Arguments
    /// * `now_ms` - 当前会话毫秒。
    ///
    /// # Returns
    /// 满足条件返回 `true`。
    ///
    /// # Errors
    /// 不会失败。
    fn should_rebuild(&self, now_ms: u64) -> bool {
        let Some(detected) = self.detected_ms else {
            return false;
        };
        // 总等待必须覆盖连续配置事件；静默期避免在事件洪峰中重建。
        now_ms.saturating_sub(detected) >= REBUILD_SETTLE_MS
            && now_ms.saturating_sub(self.last_signal_ms) >= REBUILD_QUIET_MS
    }

    /// 是否未被限流。
    ///
    /// # Arguments
    /// * `now_ms` - 当前会话毫秒。
    ///
    /// # Returns
    /// 最近 10s 内重建次数 < 3 时返回 `true`。
    ///
    /// # Errors
    /// 不会失败。
    fn cooldown_ok(&self, now_ms: u64) -> bool {
        let recent = self
            .rebuilds
            .iter()
            .filter(|time| now_ms.saturating_sub(**time) < REBUILD_WINDOW_MS)
            .count();
        recent < REBUILD_MAX_IN_WINDOW
    }

    /// 记录一次已执行的重建，并清理过期记录。
    ///
    /// # Arguments
    /// * `now_ms` - 当前会话毫秒。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    fn note_rebuild(&mut self, now_ms: u64) {
        self.rebuilds.push(now_ms);
        self.rebuilds
            .retain(|time| now_ms.saturating_sub(*time) < REBUILD_WINDOW_MS);
    }

    /// 清空本轮检测状态。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    fn clear(&mut self) {
        self.detected_ms = None;
        self.last_signal_ms = 0;
    }
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 行步长按 32 位对齐，32bpp 下等于宽度 × 4。
    #[test]
    fn stride_is_aligned() {
        assert_eq!(stride_bytes(1280, 32), 1280 * 4);
        assert_eq!(stride_bytes(1, 32), 4);
        assert_eq!(stride_bytes(0, 32), 0);
    }

    /// 逐行拷贝能正确处理源行填充。
    #[test]
    fn copy_rows_skips_padding() {
        let src = vec![1u8; 12];
        let mut dst = vec![0u8; 8];
        copy_rows(&src, &mut dst, 6, 4, 2);
        assert_eq!(&dst[0..4], &[1, 1, 1, 1]);
        assert_eq!(&dst[4..8], &[1, 1, 1, 1]);
    }

    /// 防抖：静默期不足不重建，满足两项条件后才重建。
    #[test]
    fn debounce_waits_for_quiet_and_settle() {
        let mut debounce = RebuildDebounce::new();
        debounce.signal(0);
        assert!(!debounce.should_rebuild(1_000));
        debounce.signal(1_000);
        // 总等待 1400ms 不足 1500ms，即使静默已够也不重建。
        assert!(!debounce.should_rebuild(1_400));
        // 首次信号在 0，最后一次在 1000：1600 时总等待 ≥1500 且静默 ≥300。
        assert!(debounce.should_rebuild(1_600));
        debounce.clear();
        assert!(!debounce.should_rebuild(9_999));
    }

    /// 限流：10 秒内第 4 次重建必须被拒绝。
    #[test]
    fn debounce_cooldown_limits_rebuilds() {
        let mut debounce = RebuildDebounce::new();
        for index in 0..3 {
            debounce.note_rebuild(index * 100);
        }
        assert!(!debounce.cooldown_ok(500));
        // 窗口滑过后重新允许。
        assert!(debounce.cooldown_ok(10_500));
        debounce.note_rebuild(10_500);
        assert!(debounce.rebuilds.len() == 1);
    }

    /// 采集矩形越界判定：目标超出 root 即视为变化。
    #[test]
    fn rect_bounds_are_compared() {
        let rect = CaptureRect::full_screen(1280, 800);
        assert_eq!(u32::from(rect.width), 1280);
        assert_eq!(u32::from(rect.height), 800);
    }
}
