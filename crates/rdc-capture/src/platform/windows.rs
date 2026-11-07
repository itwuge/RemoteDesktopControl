//! rdc-capture / Windows 平台采集后端
//!
//! 层级：L3 采集层 · 平台适配
//! 用途：DXGI Desktop Duplication 为主路径、GDI BitBlt 为兼容回退路径；
//!       统一输出 BGRA8 帧（含 QPC 时间戳与脏区/移动矩形），并累计采集统计
//! 调用方：rdc-capture::platform::create_source（由 rdc-ctl 在会话建立时调用）
//!
//! 安全边界（unsafe）：
//! - 所有 `unsafe` 都集中在 [`DxgiCapturer`] / [`GdiCapturer`] 内部，平台句柄不跨线程共享；
//!   采集线程创建、使用、释放全部在 [`capture_loop`] 内完成。
//! - COM 对象（D3D11 设备、DXGI 复制对象）只在采集线程内使用，遵循 COM 多线程套间（MTA）。
//! - 帧像素只在 `Map`/DIB 指针有效期内拷贝到池化缓冲，绝不把裸指针存进 [`Frame`]。
//!
//! 纹理直出（S7-3）的三条硬约束：
//! 1. **同适配器**：采集线程不自己建设备，直接使用调用方（管线线程）传入的
//!    `ID3D11Device`；`from_output` 仍校验设备适配器 LUID 与输出适配器一致。
//! 2. **立即上下文串行化**：采集线程的 `CopyResource` 与管线线程的时间码覆盖/
//!    编码都走同一个立即上下文，调用方必须已开启 `ID3D10Multithread` 保护。
//! 3. **池回收时机**：纹理以租约形式随 `Frame` 交给消费方；编码器（AMF）可能在
//!    `SubmitInput` 返回后仍引用纹理，消费方必须延迟若干帧再 drop 租约。

use std::sync::{Arc, Mutex};

use rdc_runtime::{spawn_named, ExitToken, FramePool, ThreadPrio};
use tracing::{info, warn};

use crate::frame::{Frame, FrameMeta, FrameRect, GpuFrame, GpuLease, MoveRect, PixelFormat};
use crate::params::POOL_BLOCKS;
use crate::source::{CaptureConfig, CaptureSource, DisplayInfo, FrameSink};
use crate::stats::CaptureStats;
use crate::CaptureError;

use windows::core::Interface;
use windows::Win32::Foundation::{HMODULE, RECT, TRUE};
use windows::Win32::Graphics::Direct3D::{D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_11_0};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, ID3D11Texture2D, D3D11_BIND_RENDER_TARGET,
    D3D11_BIND_SHADER_RESOURCE, D3D11_BOX, D3D11_CPU_ACCESS_READ, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_MAP_READ, D3D11_MAPPED_SUBRESOURCE, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
    D3D11_USAGE_STAGING,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::{
    CreateDXGIFactory1, IDXGIAdapter1, IDXGIDevice, IDXGIFactory1, IDXGIOutput1,
    IDXGIOutputDuplication, IDXGIResource, DXGI_ERROR_ACCESS_LOST, DXGI_ERROR_WAIT_TIMEOUT,
    DXGI_OUTDUPL_FRAME_INFO,
};
use windows::Win32::Graphics::Gdi::{
    BitBlt, CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, EnumDisplayMonitors,
    EnumDisplaySettingsW, GetDC, GetMonitorInfoW, ReleaseDC, SelectObject, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, DEVMODEW, ENUM_CURRENT_SETTINGS, HDC, HMONITOR,
    MONITORINFO, MONITORINFOEXW, SRCCOPY,
};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::GetDesktopWindow;

use std::sync::OnceLock;

// =====** 后端标识 =====**

/// 实际生效的采集后端。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    /// DXGI Desktop Duplication（主路径）。
    Dxgi,
    /// GDI BitBlt（兼容回退，帧率上限 15fps）。
    Gdi,
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
            Self::Dxgi => "dxgi-desktop-duplication",
            Self::Gdi => "gdi-bitblt",
        }
    }
}

// =====** PlatformCapture =====**

/// Windows 采集源。
///
/// 生命周期与 [`CaptureSource`] 一致；平台句柄只存在于采集线程内，
/// 本结构只保存线程控制状态与统计句柄，因此可安全 `Send`。
pub struct PlatformCapture {
    /// 帧消费者。
    sink: Option<Arc<dyn FrameSink>>,
    /// 当前配置。
    config: Option<CaptureConfig>,
    /// 退出令牌。
    token: Arc<ExitToken>,
    /// 采集线程。
    thread: Option<std::thread::JoinHandle<()>>,
    /// 本次实际后端；未启动时为 `None`。
    backend: Option<Backend>,
    /// 统计（与采集线程共享）。
    stats: Arc<CaptureStats>,
    /// 帧内存池（与采集线程共享）。
    pool: Option<Arc<FramePool>>,
    /// 启动时刻（用于帧率统计）。
    started_at: Option<std::time::Instant>,
    /// 上次停止时的会话时长（毫秒）；启动期间为 0。
    stopped_elapsed_ms: u64,
    /// 调用方提供的 D3D11 设备（`ID3D11Device*` 以 `usize` 保存）；`0` 表示走 CPU 路径。
    gpu_device: usize,
}

impl PlatformCapture {
    /// 构造未绑定的 Windows 采集源。
    ///
    /// # Arguments
    /// * 无。
    ///
    /// # Returns
    /// 未启动实例；同时声明进程 DPI 感知（docs/08 §2.2 要求）。
    ///
    /// # Errors
    /// 不会失败；DPI 声明失败只记录不阻断。
    pub fn new() -> Self {
        // 为什么在这里声明：GDI 回退路径的 BitBlt 受 DPI 虚拟化影响，
        // 必须在任何窗口/DC 创建前声明 per-monitor v2，否则抓到的像素会被系统缩放。
        ensure_dpi_awareness();
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
            gpu_device: 0,
        }
    }

    /// 当前是否以纹理直出模式运行（供日志与状态接口使用）。
    ///
    /// 说明：桥接层通过 `set_gpu_device` 的返回值即可知道结果，此方法保留给
    /// 未来的状态接口；当前无调用点。
    ///
    /// # Returns
    /// 已绑定外部设备返回 `true`。
    ///
    /// # Errors
    /// 不会失败。
    #[allow(dead_code)]
    pub fn gpu_direct(&self) -> bool {
        self.gpu_device != 0
    }

    /// 探测当前可用的采集后端，不建立持久会话。
    ///
    /// # Arguments
    /// * `display_id` - 目标显示器编号。
    ///
    /// # Returns
    /// 优先返回 DXGI；DXGI 不可用时返回 GDI；两者都不可用返回 [`CaptureError::Unsupported`]。
    ///
    /// # Errors
    /// 目标显示器不存在返回 [`CaptureError::DisplayDisappeared`]。
    pub fn probe(display_id: u32) -> Result<Backend, CaptureError> {
        if DxgiCapturer::new(display_id, 0).is_ok() {
            return Ok(Backend::Dxgi);
        }
        let displays = enumerate_displays();
        let display = displays
            .iter()
            .find(|d| d.id == display_id)
            .ok_or(CaptureError::DisplayDisappeared { display_id })?;
        if GdiCapturer::new(display).is_ok() {
            return Ok(Backend::Gdi);
        }
        Err(CaptureError::Unsupported)
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

    /// 采集统计快照（诊断用）。
    ///
    /// # Returns
    /// 当前统计；未启动时计数为 0。
    ///
    /// # Errors
    /// 不会失败。
    fn stats(&self) -> crate::CaptureSnapshot {
        // 停止后仍要能读到会话时长，否则 fps 会被算成 0。
        let elapsed = self
            .started_at
            .map_or(self.stopped_elapsed_ms, |t| t.elapsed().as_millis() as u64);
        self.stats.snapshot(elapsed)
    }

    /// 枚举显示器（GDI 监视器枚举，返回物理像素尺寸）。
    ///
    /// # Arguments
    /// * 无。
    ///
    /// # Returns
    /// 显示器列表；无显示器返回空列表。
    ///
    /// # Errors
    /// 不会失败，错误以空列表表达。
    fn enumerate(&self) -> Vec<DisplayInfo> {
        enumerate_displays()
    }

    /// 启动采集。
    ///
    /// # Arguments
    /// * `config` - 目标显示器、帧率与光标合成参数。
    ///
    /// # Returns
    /// 采集线程已启动返回 `Ok(())`。
    ///
    /// # Errors
    /// 未绑定 sink、显示器消失、后端不可用或线程创建失败时返回 [`CaptureError`]。
    fn start(&mut self, config: CaptureConfig) -> Result<(), CaptureError> {
        let sink = self
            .sink
            .clone()
            .ok_or_else(|| CaptureError::Internal("未调用 set_sink".to_owned()))?;
        if self.thread.is_some() {
            return Err(CaptureError::Internal("采集已在运行".to_owned()));
        }
        let displays = enumerate_displays();
        let display_info = displays
            .iter()
            .find(|d| d.id == config.display_id)
            .cloned()
            .ok_or(CaptureError::DisplayDisappeared {
                display_id: config.display_id,
            })?;

        // 启动时探测可用性，并把实际路径写入日志（要求：不得静默换路径）。
        let backend = Self::probe(config.display_id)?;
        info!(
            display_id = config.display_id,
            backend = backend.name(),
            width = display_info.width,
            height = display_info.height,
            refresh_hz = display_info.refresh_hz,
            "采集后端探测完成"
        );

        let pool = Arc::new(FramePool::new(
            POOL_BLOCKS,
            /* 单块按 BGRA8 整帧计算：1920×1080×4 ≈ 8.3MB，8 块约 66MB（docs/06 §1.3 固定池） */
            PixelFormat::Bgra8.buffer_size(display_info.width, display_info.height),
        ));
        self.pool = Some(Arc::clone(&pool));
        self.stats = Arc::new(CaptureStats::new());
        self.config = Some(config);
        self.backend = Some(backend);
        self.token = Arc::new(ExitToken::default());
        let token = Arc::clone(&self.token);
        let stats = Arc::clone(&self.stats);

        let name = format!("rdc-cap-{}", config.display_id);
        let gpu_device = self.gpu_device;
        let handle = spawn_named(
            &name,
            ThreadPrio::Net,
            Arc::clone(&token),
            move || capture_loop(backend, config, display_info, sink, pool, stats, token, gpu_device),
        )
        .map_err(|e| CaptureError::Internal(format!("采集线程创建失败: {e}")))?;
        self.thread = Some(handle);
        self.started_at = Some(std::time::Instant::now());
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

    /// 绑定调用方的 D3D11 设备以启用纹理直出。
    ///
    /// 为什么在 start 之前一次性绑定：DDA 的 duplication 与设备绑定，
    /// 中途换设备必须重建 duplication（即一次 reconfigure），没有收益。
    ///
    /// # Arguments
    /// * `device` - `ID3D11Device*`（`usize`）；`0` 关闭纹理直出。
    /// * `_context` - 预留参数；采集侧从设备取立即上下文。
    ///
    /// # Returns
    /// 非 0 设备被接受时返回 `true`。
    ///
    /// # Errors
    /// 不会失败；设备是否与输出同适配器由采集线程建立 duplication 时校验。
    fn set_gpu_device(&mut self, device: usize, _context: usize) -> bool {
        if self.thread.is_some() {
            // 运行中改设备会让下一帧的纹理归属不确定，直接拒绝。
            return false;
        }
        self.gpu_device = device;
        device != 0
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
        // 为什么重启而不是改参数：DDA 的 duplication 与输出模式绑定，
        // 分辨率/显示器变化必须重建 duplication（docs/08 §2.2 第 2 条）。
        self.stop();
        self.start(config)
    }

    /// 停止采集并释放平台句柄。
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
            let _ = handle.join();
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
/// * `backend` - 探测阶段确定的后端。
/// * `config` - 采集配置。
/// * `display` - 目标显示器信息。
/// * `sink` - 帧消费者。
/// * `pool` - 帧内存池。
/// * `stats` - 统计计数器。
/// * `token` - 退出令牌。
///
/// # Returns
/// 无返回值；线程在退出令牌置位或不可恢复错误时结束。
///
/// # Errors
/// 不返回错误；失败写日志并通过统计计数体现。
fn capture_loop(
    backend: Backend,
    config: CaptureConfig,
    display: DisplayInfo,
    sink: Arc<dyn FrameSink>,
    pool: Arc<FramePool>,
    stats: Arc<CaptureStats>,
    token: Arc<ExitToken>,
    gpu_device: usize,
) {
    stats.mark_start(qpc_us());
    let frame_interval_us = if config.target_fps == 0 {
        16_667
    } else {
        1_000_000 / u64::from(config.target_fps)
    };

    match backend {
        Backend::Dxgi => capture_loop_dxgi(
            &config,
            &display,
            &sink,
            &pool,
            &stats,
            &token,
            frame_interval_us,
            gpu_device,
        ),
        Backend::Gdi => capture_loop_gdi(
            &config,
            &display,
            &sink,
            &pool,
            &stats,
            &token,
            frame_interval_us,
        ),
    }
}

/// DXGI 采集循环（支持 ACCESS_LOST 后重建 duplication）。
fn capture_loop_dxgi(
    config: &CaptureConfig,
    display: &DisplayInfo,
    sink: &Arc<dyn FrameSink>,
    pool: &Arc<FramePool>,
    stats: &Arc<CaptureStats>,
    token: &Arc<ExitToken>,
    frame_interval_us: u64,
    gpu_device: usize,
) {
    // 纹理直出有两种传法（`RDC_CAPTURE_GPU`）：
    //   1 = DDA → 池化纹理 `CopyResource` → 编码器（有中间拷贝，实测首次 GPU 读付 28~43 ms）
    //   2 = DDA 纹理**直接**交编码器（无拷贝，实验路径）
    let gpu_mode: u8 = if gpu_device == 0 {
        0
    } else {
        std::env::var("RDC_CAPTURE_GPU")
            .ok()
            .and_then(|v| v.parse::<u8>().ok())
            .unwrap_or(1)
            .clamp(1, 2)
    };
    let mut capturer = match DxgiCapturer::new(config.display_id, gpu_device) {
        Ok(c) => c,
        Err(err) => {
            warn!(error = %err, "DXGI 采集初始化失败，线程退出");
            return;
        }
    };
    // 纹理直出：池在采集线程创建（设备已通过 set_gpu_device 绑定）。
    let gpu_pool = if gpu_mode == 1 {
        match GpuTexturePool::new(&capturer.device, display.width, display.height) {
            Ok(pool) => {
                let (dw, dh) = (display.width, display.height);
                info!(
                    blocks = GpuTexturePool::BLOCKS,
                    "采集走纹理直出路径（DDA → 池化 ID3D11Texture2D，无 CPU 回读） {}x{}",
                    dw,
                    dh
                );
                Some(pool)
            }
            Err(err) => {
                warn!(error = %err, "纹理池创建失败，线程退出（不回退到 CPU 路径以免编码器无纹理输入）");
                return;
            }
        }
    } else if gpu_mode == 2 {
        info!("采集走 DDA 纹理直入路径（不拷贝，DDA 纹理直接交编码器）");
        None
    } else {
        info!("采集走 CPU BGRA 路径（staging 回读）");
        None
    };
    let mut frame_id: u64 = 0;
    loop {
        if token.is_requested() {
            break;
        }
        let timeout_ms = (frame_interval_us / 1000).max(1) as u32;
        match capturer.capture(timeout_ms, gpu_pool.as_ref(), gpu_mode == 2) {
            Ok(Acquired::Cpu(shot)) => {
                let Some(mut buffer) = pool.acquire() else {
                    // 池满：丢帧而不是等待（docs/06 §1.3）。
                    stats.add_pool_exhausted();
                    stats.add_dropped(1);
                    continue;
                };
                shot.copy_into(buffer.as_mut_slice());
                let qpc = shot.qpc_us;
                let interval = stats.record_frame(qpc, shot.cost_us);
                let frame = Frame {
                    buffer,
                    meta: FrameMeta {
                        pts_ns: qpc.saturating_mul(1000),
                        qpc_us: qpc,
                        frame_id,
                        display_id: config.display_id,
                    },
                    format: PixelFormat::Bgra8,
                    width: display.width,
                    height: display.height,
                    dirty_rects: shot.dirty_rects,
                    move_rects: shot.move_rects,
                    capture_cost_us: shot.cost_us as u32,
                    interval_us: interval as u32,
                    gpu: None,
                };
                sink.on_frame(frame);
                frame_id = frame_id.wrapping_add(1);
            }
            Ok(Acquired::Gpu(shot)) => {
                let Some(buffer) = pool.acquire() else {
                    stats.add_pool_exhausted();
                    stats.add_dropped(1);
                    continue;
                };
                let qpc = shot.qpc_us;
                let interval = stats.record_frame(qpc, shot.cost_us);
                let frame = Frame {
                    buffer,
                    meta: FrameMeta {
                        pts_ns: qpc.saturating_mul(1000),
                        qpc_us: qpc,
                        frame_id,
                        display_id: config.display_id,
                    },
                    format: PixelFormat::Bgra8,
                    width: display.width,
                    height: display.height,
                    dirty_rects: shot.dirty_rects,
                    move_rects: shot.move_rects,
                    capture_cost_us: shot.cost_us as u32,
                    interval_us: interval as u32,
                    gpu: Some(shot.gpu),
                };
                sink.on_frame(frame);
                frame_id = frame_id.wrapping_add(1);
            }
            Ok(Acquired::NoFrame) => {
                // 桌面无变化：不是丢帧，记超时便于区分「静止画面」与「采集故障」。
                stats.add_timeout();
            }
            Ok(Acquired::PoolExhausted) => {
                // 纹理池满：说明下游（编码/通道）积压，记丢帧暴露背压。
                stats.add_pool_exhausted();
                stats.add_dropped(1);
            }
            Err(err) => {
                // 切换分辨率/锁屏/UAC 会返回 ACCESS_LOST：重建 duplication 而不是结束会话。
                if is_access_lost(&err) {
                    warn!("DXGI duplication 失效，重建中");
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    match DxgiCapturer::new(config.display_id, gpu_device) {
                        Ok(c) => capturer = c,
                        Err(e) => {
                            warn!(error = %e, "DXGI 重建失败，线程退出");
                            return;
                        }
                    }
                    continue;
                }
                warn!(error = %err, "DXGI 采集失败，线程退出");
                return;
            }
        }
    }
}

/// GDI 采集循环（固定 15fps 上限，docs/08 §2.2 第 4 条）。
fn capture_loop_gdi(
    config: &CaptureConfig,
    display: &DisplayInfo,
    sink: &Arc<dyn FrameSink>,
    pool: &Arc<FramePool>,
    stats: &Arc<CaptureStats>,
    token: &Arc<ExitToken>,
    frame_interval_us: u64,
) {
    let mut capturer = match GdiCapturer::new(display) {
        Ok(c) => c,
        Err(err) => {
            warn!(error = %err, "GDI 采集初始化失败，线程退出");
            return;
        }
    };
    let mut frame_id: u64 = 0;
    let period = frame_interval_us.max(66_667);
    let mut deadline = std::time::Instant::now();
    loop {
        if token.is_requested() {
            break;
        }
        let start = qpc_us();
        match capturer.capture(display.width, display.height) {
            Ok(shot) => {
                let cost = qpc_us().saturating_sub(start);
                let Some(mut buffer) = pool.acquire() else {
                    stats.add_pool_exhausted();
                    stats.add_dropped(1);
                    continue;
                };
                shot.copy_into(buffer.as_mut_slice());
                let qpc = start;
                let interval = stats.record_frame(qpc, cost);
                let frame = Frame {
                    buffer,
                    meta: FrameMeta {
                        pts_ns: qpc.saturating_mul(1000),
                        qpc_us: qpc,
                        frame_id,
                        display_id: config.display_id,
                    },
                    format: PixelFormat::Bgra8,
                    width: display.width,
                    height: display.height,
                    dirty_rects: Vec::new(),
                    move_rects: Vec::new(),
                    capture_cost_us: cost as u32,
                    interval_us: interval as u32,
                    gpu: None,
                };
                sink.on_frame(frame);
                frame_id = frame_id.wrapping_add(1);
            }
            Err(err) => {
                warn!(error = %err, "GDI 采集失败");
            }
        }
        deadline += std::time::Duration::from_micros(period);
        let now = std::time::Instant::now();
        if deadline > now {
            std::thread::sleep(deadline - now);
        } else {
            // 落后超过一个周期则重置基准，避免无限追赶。
            deadline = now;
        }
    }
}

// =====** 采集结果 =====**

/// 一帧已读到 CPU 内存的采集结果（像素在 `pixels` 中）。
struct Shot {
    /// 采集瞬间 QPC 微秒。
    qpc_us: u64,
    /// 从帧到手到像素拷贝完成耗时（微秒）。
    cost_us: u64,
    /// 脏区。
    dirty_rects: Vec<FrameRect>,
    /// 移动矩形。
    move_rects: Vec<MoveRect>,
    /// 像素数据（BGRA8，紧凑排布）。
    pixels: Vec<u8>,
}

impl Shot {
    /// 把像素拷贝进池化缓冲（长度不足时按行截断，绝不越界）。
    ///
    /// # Arguments
    /// * `dst` - 目标缓冲。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    fn copy_into(&self, dst: &mut [u8]) {
        let n = dst.len().min(self.pixels.len());
        dst[..n].copy_from_slice(&self.pixels[..n]);
    }
}

// =====** 纹理直出池 =====**

/// 一次 `AcquireNextFrame` 的结果。
enum Acquired {
    /// CPU 像素（staging 回读）。
    Cpu(Shot),
    /// GPU 纹理（零 CPU 拷贝）。
    Gpu(GpuShot),
    /// 超时：桌面无变化。
    NoFrame,
    /// 纹理池暂时无空闲块：调用方记丢帧。
    PoolExhausted,
}

/// GPU 纹理帧 + 元数据。
struct GpuShot {
    /// 纹理与租约。
    gpu: GpuFrame,
    /// 采集时刻（QPC 微秒）。
    qpc_us: u64,
    /// 采集耗时（微秒）。
    cost_us: u64,
    /// 脏区。
    dirty_rects: Vec<FrameRect>,
    /// 移动矩形。
    move_rects: Vec<MoveRect>,
}

/// 固定大小、租约式回收的 D3D11 纹理池。
///
/// 为什么不复用 DDA 纹理：`AcquireNextFrame` 的纹理在 `ReleaseFrame` 后归属驱动，
/// 且尺寸/格式随显示模式变化；自有池可以固定尺寸并在写完后立即释放 DDA 资源。
///
/// 线程模型：池在采集线程创建，纹理只在采集线程写入；租约的 `Drop` 只触碰
/// `free` 互斥量，因此跨线程归还安全。设备/上下文均为 COM 接口，按 COM 约定
/// 不做 `Send`/`Sync` 自动推导，这里显式声明：所有设备访问都在受
/// `ID3D10Multithread` 保护的线程上串行发生。
struct GpuTexturePool {
    /// 创建纹理的设备；持有引用保证纹理在池生命周期内有效。
    _device: ID3D11Device,
    /// 池纹理（每块同时保留 `ID3D11Resource` 视图，供 `CopyResource`）。
    textures: Vec<PoolTexture>,
    /// 空闲索引。
    free: Mutex<Vec<usize>>,
    /// 纹理宽。
    width: u32,
    /// 纹理高。
    height: u32,
}

/// 池中一块纹理的两个接口视图。
struct PoolTexture {
    /// `ID3D11Texture2D`。
    texture: ID3D11Texture2D,
    /// `ID3D11Resource`（`CopyResource` 目标）。
    resource: windows::Win32::Graphics::Direct3D11::ID3D11Resource,
}

/// 纹理租约：`Drop` 时归还索引。
struct TextureLease {
    /// 所属池。
    pool: Arc<GpuTexturePool>,
    /// 占用的索引。
    index: usize,
}

impl GpuLease for TextureLease {}

/// DDA 纹理租约：只持有 COM 引用，不参与池回收。
///
/// 用于 `dda_direct` 实验路径：纹理由 DDA 提供，我们没有自己的池块可归还。
struct DdaLease {
    /// 持有的 DDA 纹理；`Drop` 时释放引用。
    _texture: ID3D11Texture2D,
}

impl GpuLease for DdaLease {}

impl Drop for TextureLease {
    fn drop(&mut self) {
        if let Ok(mut free) = self.pool.free.lock() {
            free.push(self.index);
        }
    }
}

impl TextureLease {
    /// 租约对应的 `ID3D11Resource` 视图（`CopyResource` 目标）。
    ///
    /// # Returns
    /// 资源接口引用。
    ///
    /// # Errors
    /// 不会失败。
    fn resource(&self) -> &windows::Win32::Graphics::Direct3D11::ID3D11Resource {
        &self.pool.textures[self.index].resource
    }

    /// 转成随 `Frame` 移动的 GPU 帧描述。
    ///
    /// # Returns
    /// GPU 帧（租约随帧移动）。
    ///
    /// # Errors
    /// 不会失败。
    fn into_frame(self) -> GpuFrame {
        let texture = self.pool.textures[self.index].texture.as_raw() as usize;
        GpuFrame::new(
            texture,
            0,
            self.pool.width,
            self.pool.height,
            PixelFormat::Bgra8,
            Box::new(self),
        )
    }
}

// SAFETY: 设备与纹理只在采集线程访问；跨线程共享的是 `Arc<GpuTexturePool>`，
// 其中唯一跨线程访问的成员是 `Mutex<Vec<usize>>`（租约归还）。
unsafe impl Send for GpuTexturePool {}
unsafe impl Sync for GpuTexturePool {}

impl GpuTexturePool {
    /// 池容量：3 帧在途（管线 1 + 通道 1 + AMF 内部 1）再留 3 帧余量。
    const BLOCKS: usize = 6;

    /// 创建设备上的纹理池。
    ///
    /// # Arguments
    /// * `device` - 管线设备（与编码器同一设备）。
    /// * `width` / `height` - 纹理尺寸（像素）。
    ///
    /// # Returns
    /// 可共享的纹理池。
    ///
    /// # Errors
    /// 纹理创建失败时返回 [`CaptureError::Internal`]。
    fn new(device: &ID3D11Device, width: u32, height: u32) -> Result<Arc<Self>, CaptureError> {
        unsafe {
            let desc = D3D11_TEXTURE2D_DESC {
                Width: width,
                Height: height,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_DEFAULT,
                // VideoProcessor 输入视图要求可作 SRV/RT；AMF 的 BGRA→NV12 走 VP。
                BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let mut textures = Vec::with_capacity(Self::BLOCKS);
            for index in 0..Self::BLOCKS {
                let mut texture: Option<ID3D11Texture2D> = None;
                device
                    .CreateTexture2D(&desc, None, Some(&mut texture))
                    .map_err(|e| {
                        CaptureError::Internal(format!("CreateTexture2D[{index}]: {e}"))
                    })?;
                let texture = texture.ok_or_else(|| {
                    CaptureError::Internal("CreateTexture2D 未返回纹理".to_owned())
                })?;
                let resource = texture
                    .cast()
                    .map_err(|e| CaptureError::Internal(format!("cast ID3D11Resource: {e}")))?;
                textures.push(PoolTexture { texture, resource });
            }
            Ok(Arc::new(Self {
                _device: device.clone(),
                textures,
                free: Mutex::new((0..Self::BLOCKS).collect()),
                width,
                height,
            }))
        }
    }

    /// 取一块空闲纹理。
    ///
    /// 为什么 `self: &Arc<Self>`：租约必须持有池的强引用，否则帧还在管线里时
    /// 采集线程退出会把池释放掉，租约归还时就成了悬垂指针。
    ///
    /// # Returns
    /// 有空闲块返回租约；池满返回 `None`（**不阻塞**，调用方丢帧）。
    ///
    /// # Errors
    /// 不会失败。
    fn acquire(self: &Arc<Self>) -> Option<TextureLease> {
        let index = self.free.lock().ok()?.pop()?;
        Some(TextureLease {
            pool: Arc::clone(self),
            index,
        })
    }
}

// =====** DXGI 实现 =====**

/// DXGI Desktop Duplication 采集器。
///
/// 安全前置：所有 COM 对象在创建它的线程内使用与释放；`Map` 指针仅在
/// [`DxgiCapturer::capture`] 的作用域内有效。
struct DxgiCapturer {
    /// D3D11 设备（dup 的对象）。
    device: ID3D11Device,
    /// 立即上下文（用于 CopyResource/Map）。
    context: ID3D11DeviceContext,
    /// 桌面复制对象。
    duplication: IDXGIOutputDuplication,
    /// 读回用的 staging 纹理；尺寸/格式变化时重建。
    staging: Option<ID3D11Texture2D>,
    /// staging 对应的宽高。
    staging_size: (u32, u32),
    /// 「预热探针」：1x1 staging 纹理；`dda_direct` 路径用它强制 DDA 内容完成
    /// 一次真实 GPU 读（`CopySubresourceRegion` + `Map`），把首次读取的同步代价
    /// 显式落在 `capture_us` 里，用来量化「同步可否提前触发」。
    probe: Option<ID3D11Texture2D>,
}

impl DxgiCapturer {
    /// 为指定显示器创建 duplication。
    ///
    /// # Arguments
    /// * `display_id` - 全局显示器编号（按适配器/输出枚举顺序）。
    /// * `gpu_device` - 调用方设备（`ID3D11Device*` 的 `usize`）；`0` 表示自建设备走 CPU 路径。
    ///
    /// # Returns
    /// 可用的采集器。
    ///
    /// # Errors
    /// 显示器越界返回 [`CaptureError::DisplayDisappeared`]；
    /// D3D11/DXGI 初始化失败或 `DuplicateOutput` 被拒绝返回 [`CaptureError::DisplayBusy`]。
    fn new(display_id: u32, gpu_device: usize) -> Result<Self, CaptureError> {
        unsafe {
            let factory: IDXGIFactory1 = CreateDXGIFactory1()
                .map_err(|e| CaptureError::Internal(format!("CreateDXGIFactory1: {e}")))?;
            let mut global_index = 0u32;
            let mut adapter_index = 0u32;
            loop {
                let adapter = match factory.EnumAdapters1(adapter_index) {
                    Ok(a) => a,
                    // DXGI_ERROR_NOT_FOUND：没有更多适配器，说明 display_id 越界。
                    Err(_) => {
                        return Err(CaptureError::DisplayDisappeared { display_id });
                    }
                };
                let output_index = 0u32;
                loop {
                    let output = match adapter.EnumOutputs(output_index) {
                        Ok(o) => o,
                        Err(_) => break,
                    };
                    let mut desc = Default::default();
                    output
                        .GetDesc(&mut desc)
                        .map_err(|e| CaptureError::Internal(format!("IDXGIOutput::GetDesc: {e}")))?;
                    if !desc.AttachedToDesktop.as_bool() {
                        // 未接显示的输出不参与编号，避免 display_id 漂移。
                        continue;
                    }
                    if global_index == display_id {
                        return Self::from_output(&adapter, &output, display_id, gpu_device);
                    }
                    global_index += 1;
                }
                adapter_index += 1;
            }
        }
    }

    /// 在给定适配器上创建设备并对输出建立 duplication。
    ///
    /// # Arguments
    /// * `adapter` - 输出所属适配器（混合显卡场景必须一致，docs/08 §2.2 坑位）。
    /// * `output` - 目标输出。
    ///
    /// # Returns
    /// 采集器。
    ///
    /// # Errors
    /// 设备创建或 duplication 建立失败时返回 [`CaptureError::DisplayBusy`]。
    fn from_output(
        adapter: &IDXGIAdapter1,
        output: &windows::Win32::Graphics::Dxgi::IDXGIOutput,
        display_id: u32,
        gpu_device: usize,
    ) -> Result<Self, CaptureError> {
        unsafe {
            let (device, context) = if gpu_device != 0 {
                // 纹理直出：复用调用方设备，保证纹理与编码器在同一设备上。
                // `from_raw_borrowed` 不接管所有权，`cloned()` 只做一次 AddRef，
                // 采集侧 drop 时 Release；调用方的引用不受影响。
                let raw = gpu_device as *mut std::ffi::c_void;
                let device = ID3D11Device::from_raw_borrowed(&raw)
                    .cloned()
                    .ok_or_else(|| {
                        CaptureError::Internal("外部 D3D11 设备指针为空".to_owned())
                    })?;
                let context = device
                    .GetImmediateContext()
                    .map_err(|e| CaptureError::Internal(format!("GetImmediateContext: {e}")))?;
                (device, context)
            } else {
                let levels = [D3D_FEATURE_LEVEL_11_0];
                let mut device = None;
                let mut context = None;
                // D3D11CreateDevice 期望 IDXGIAdapter（基接口），IDXGIAdapter1 需先降级转换。
                let adapter_base: windows::Win32::Graphics::Dxgi::IDXGIAdapter = adapter
                    .cast()
                    .map_err(|e| CaptureError::Internal(format!("cast IDXGIAdapter: {e}")))?;
                D3D11CreateDevice(
                    &adapter_base,
                    D3D_DRIVER_TYPE_UNKNOWN,
                    HMODULE::default(),
                    D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                    Some(&levels),
                    windows::Win32::Graphics::Direct3D11::D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    Some(&mut context),
                )
                .map_err(|e| CaptureError::Internal(format!("D3D11CreateDevice: {e}")))?;
                let device: ID3D11Device = device.ok_or_else(|| {
                    CaptureError::Internal("D3D11CreateDevice 未返回设备".to_owned())
                })?;
                let context = context
                    .ok_or_else(|| CaptureError::Internal("未取得 D3D11 上下文".to_owned()))?;
                (device, context)
            };

            // 确认设备与输出属于同一适配器，避免拿到错误显卡的 duplication。
            // 外部设备同样校验：跨适配器的纹理无法被编码器（同设备）直接消费。
            let dxgi_device: IDXGIDevice = device
                .cast()
                .map_err(|e| CaptureError::Internal(format!("cast IDXGIDevice: {e}")))?;
            let device_adapter = dxgi_device
                .GetAdapter()
                .map_err(|e| CaptureError::Internal(format!("IDXGIDevice::GetAdapter: {e}")))?;
            let mut device_adapter_desc = Default::default();
            device_adapter
                .GetDesc(&mut device_adapter_desc)
                .map_err(|e| CaptureError::Internal(format!("IDXGIAdapter::GetDesc: {e}")))?;
            let mut output_adapter_desc = Default::default();
            adapter
                .GetDesc(&mut output_adapter_desc)
                .map_err(|e| CaptureError::Internal(format!("IDXGIAdapter1::GetDesc: {e}")))?;
            if device_adapter_desc.AdapterLuid.LowPart != output_adapter_desc.AdapterLuid.LowPart {
                return Err(CaptureError::Internal(
                    "设备适配器与输出适配器不一致".to_owned(),
                ));
            }

            let output1: IDXGIOutput1 = output
                .cast()
                .map_err(|e| CaptureError::Internal(format!("cast IDXGIOutput1: {e}")))?;
            // DuplicateOutput 失败最常见原因是同一输出已有会话在复制（独占）。
            let duplication = output1
                .DuplicateOutput(&device)
                .map_err(|_| CaptureError::DisplayBusy { display_id })?;
            Ok(Self {
                device,
                context,
                duplication,
                staging: None,
                staging_size: (0, 0),
                probe: None,
            })
        }
    }

    /// 抓取一帧。
    ///
    /// # Arguments
    /// * `timeout_ms` - AcquireNextFrame 超时（毫秒）。
    /// * `gpu_pool` - 纹理直出池；`Some` 时把帧拷到池纹理并返回 [`Acquired::Gpu`]，
    ///   `None` 时读回 CPU 返回 [`Acquired::Cpu`]。
    /// * `dda_direct` - `true` 时不拷贝，直接把 DDA 纹理 AddRef 后包成
    ///   [`Acquired::Gpu`]，租约持有该 `ID3D11Texture2D`。
    ///
    /// # Returns
    /// 有新帧返回 [`Acquired::Cpu`]/[`Acquired::Gpu`]；超时（画面无变化）返回
    /// [`Acquired::NoFrame`]；池耗尽返回 [`Acquired::PoolExhausted`]（由调用方记丢帧）。
    ///
    /// # 约束（`dda_direct`）
    /// `ReleaseFrame` 后纹理归驱动所有，下一次 `AcquireNextFrame` 可能覆盖同一
    /// surface：跨迭代持有只保证 COM 对象存活，不保证像素没被改写。调用方必须
    /// 尽快取样（见 TESTPLAN 块 4 Q5）。
    ///
    /// # Errors
    /// ACCESS_LOST 或其它 DXGI 错误返回 [`CaptureError`]，由调用方决定重建。
    fn capture(
        &mut self,
        timeout_ms: u32,
        gpu_pool: Option<&Arc<GpuTexturePool>>,
        dda_direct: bool,
    ) -> Result<Acquired, CaptureError> {
        unsafe {
            let mut frame_info = DXGI_OUTDUPL_FRAME_INFO::default();
            let mut resource: Option<IDXGIResource> = None;
            match self
                .duplication
                .AcquireNextFrame(timeout_ms, &mut frame_info, &mut resource)
            {
                Ok(()) => {}
                Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(Acquired::NoFrame),
                Err(e) => return Err(CaptureError::Internal(format!("AcquireNextFrame: {e}"))),
            }
            let resource = resource
                .ok_or_else(|| CaptureError::Internal("AcquireNextFrame 未返回资源".to_owned()))?;
            let qpc = qpc_us();
            let acquired = std::time::Instant::now();

            // 无论后续拷贝是否成功，都必须 ReleaseFrame，否则下一次 Acquire 会永久失败。
            let result = if dda_direct {
                self.wrap_dda_texture(&frame_info, &resource)
            } else {
                match gpu_pool {
                    Some(pool) => self.copy_to_pool(&frame_info, &resource, pool),
                    None => self
                        .read_frame(&frame_info, &resource)
                        .map(|shot| Acquired::Cpu(shot)),
                }
            };
            let _ = self.duplication.ReleaseFrame();
            let mut out = result?;
            let cost_us = acquired.elapsed().as_micros() as u64;
            match &mut out {
                Acquired::Cpu(shot) => {
                    shot.qpc_us = qpc;
                    shot.cost_us = cost_us;
                }
                Acquired::Gpu(shot) => {
                    shot.qpc_us = qpc;
                    shot.cost_us = cost_us;
                }
                Acquired::NoFrame | Acquired::PoolExhausted => {}
            }
            Ok(out)
        }
    }

    /// 把 DDA 纹理**原样**包成 [`Acquired::Gpu`]（不做任何拷贝）。
    ///
    /// 租约 [`DdaLease`] 只持有 `ID3D11Texture2D` 的 COM 引用，`Drop` 时释放引用、
    /// 不归还任何池。像素内容在 `ReleaseFrame` 之后可能被下一次采集覆盖，
    /// 因此该路径要求消费方在同一迭代内完成取样。
    ///
    /// # Arguments
    /// * `frame_info` - `AcquireNextFrame` 返回的帧信息（脏区来源）。
    /// * `resource` - DDA 返回的资源（`ID3D11Texture2D`）。
    ///
    /// # Returns
    /// 成功返回 [`Acquired::Gpu`]。
    ///
    /// # Errors
    /// 类型不符或取描述失败返回 [`CaptureError::Internal`]。
    unsafe fn wrap_dda_texture(
        &mut self,
        frame_info: &DXGI_OUTDUPL_FRAME_INFO,
        resource: &IDXGIResource,
    ) -> Result<Acquired, CaptureError> {
        let dirty_rects = self.read_dirty_rects(frame_info);
        let move_rects = self.read_move_rects(frame_info);
        let texture: ID3D11Texture2D = resource
            .cast()
            .map_err(|e| CaptureError::Internal(format!("cast ID3D11Texture2D: {e}")))?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);
        let format = if desc.Format == DXGI_FORMAT_B8G8R8A8_UNORM {
            PixelFormat::Bgra8
        } else {
            PixelFormat::Rgba8
        };
        let raw = texture.as_raw() as usize;
        // 预热探针（`RDC_GPU_PREWARM=0` 可关）：主动做一次 1x1 拷贝 + `Map`，把
        // 「DDA 内容被第一次真实 GPU 读」的同步代价挤到 capture 段里，用来判定
        // 该同步是否可以提前触发（若可以，编码器侧就不再承担这 18~28 ms）。
        if std::env::var("RDC_GPU_PREWARM").map(|v| v != "0").unwrap_or(true) {
            self.prewarm(&texture);
        }
        Ok(Acquired::Gpu(GpuShot {
            gpu: GpuFrame::new(
                raw,
                0,
                desc.Width,
                desc.Height,
                format,
                Box::new(DdaLease { _texture: texture }),
            ),
            qpc_us: 0,
            cost_us: 0,
            dirty_rects,
            move_rects,
        }))
    }

    /// 强制对 DDA 纹理做一次真实 GPU 读（1x1 `CopySubresourceRegion` + `Map`）。
    ///
    /// `Map` 会阻塞到拷贝真正完成，因此这段耗时能被 `capture_us` 量化，
    /// 而不是让编码器在 `SubmitInput`/编码时隐式承担。
    ///
    /// # Arguments
    /// * `texture` - 当前 DDA 帧纹理。
    ///
    /// # Errors
    /// 失败不影响采集（探针只用于测量，失败即跳过）。
    unsafe fn prewarm(&mut self, texture: &ID3D11Texture2D) {
        if self.probe.is_none() {
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            desc.Width = 1;
            desc.Height = 1;
            desc.MipLevels = 1;
            desc.ArraySize = 1;
            desc.Format = DXGI_FORMAT_B8G8R8A8_UNORM;
            desc.SampleDesc.Count = 1;
            desc.Usage = D3D11_USAGE_STAGING;
            desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
            let mut tex = None;
            if self
                .device
                .CreateTexture2D(&desc, None, Some(&mut tex))
                .is_err()
            {
                return;
            }
            self.probe = tex;
        }
        let Some(probe) = self.probe.as_ref() else {
            return;
        };
        let Ok(src): Result<windows::Win32::Graphics::Direct3D11::ID3D11Resource, _> =
            texture.cast()
        else {
            return;
        };
        let Ok(dst): Result<windows::Win32::Graphics::Direct3D11::ID3D11Resource, _> = probe.cast()
        else {
            return;
        };
        let region = D3D11_BOX {
            left: 0,
            top: 0,
            front: 0,
            right: 1,
            bottom: 1,
            back: 1,
        };
        self.context
            .CopySubresourceRegion(&dst, 0, 0, 0, 0, &src, 0, Some(&region));
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        if self
            .context
            .Map(probe, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .is_ok()
        {
            self.context.Unmap(probe, 0);
        }
    }

    /// 把 DDA 纹理 `CopyResource` 到池化纹理并收集脏区（不读回 CPU）。
    ///
    /// 代价是一次 GPU→GPU 拷贝提交（异步，微秒级），随后立即 `ReleaseFrame`；
    /// 像素直到编码器取样时才真正被访问，驱动内部按需同步。
    ///
    /// # Arguments
    /// * `frame_info` - `AcquireNextFrame` 返回的帧信息（脏区来源）。
    /// * `resource` - DDA 返回的资源（`ID3D11Texture2D`）。
    /// * `pool` - 目标纹理池。
    ///
    /// # Returns
    /// 池耗尽返回 [`Acquired::PoolExhausted`]，否则返回 [`Acquired::Gpu`]。
    ///
    /// # Errors
    /// 脏区读取失败或类型不符时返回 [`CaptureError::Internal`]。
    unsafe fn copy_to_pool(
        &mut self,
        frame_info: &DXGI_OUTDUPL_FRAME_INFO,
        resource: &IDXGIResource,
        pool: &Arc<GpuTexturePool>,
    ) -> Result<Acquired, CaptureError> {
        let dirty_rects = self.read_dirty_rects(frame_info);
        let move_rects = self.read_move_rects(frame_info);
        let Some(lease) = pool.acquire() else {
            return Ok(Acquired::PoolExhausted);
        };
        let src: ID3D11Texture2D = resource
            .cast()
            .map_err(|e| CaptureError::Internal(format!("cast ID3D11Texture2D: {e}")))?;
        let src_res: windows::Win32::Graphics::Direct3D11::ID3D11Resource = src
            .cast()
            .map_err(|e| CaptureError::Internal(format!("cast ID3D11Resource: {e}")))?;
        self.context.CopyResource(lease.resource(), &src_res);
        Ok(Acquired::Gpu(GpuShot {
            gpu: lease.into_frame(),
            qpc_us: 0,
            cost_us: 0,
            dirty_rects,
            move_rects,
        }))
    }

    /// 把 GPU 纹理读回 CPU 并收集脏区。
    unsafe fn read_frame(
        &mut self,
        frame_info: &DXGI_OUTDUPL_FRAME_INFO,
        resource: &IDXGIResource,
    ) -> Result<Shot, CaptureError> {
        let texture: ID3D11Texture2D = resource
            .cast()
            .map_err(|e| CaptureError::Internal(format!("cast ID3D11Texture2D: {e}")))?;
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        texture.GetDesc(&mut desc);
        let w = desc.Width;
        let h = desc.Height;

        // staging 纹理只在尺寸变化时重建（分辨率切换会触发）。
        if self.staging.is_none() || self.staging_size != (w, h) {
            let staging_desc = D3D11_TEXTURE2D_DESC {
                Width: w,
                Height: h,
                MipLevels: 1,
                ArraySize: 1,
                Format: desc.Format,
                SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Usage: D3D11_USAGE_STAGING,
                BindFlags: 0,
                CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                MiscFlags: 0,
            };
            let mut staging = None;
            self.device
                .CreateTexture2D(&staging_desc, None, Some(&mut staging))
                .map_err(|e| CaptureError::Internal(format!("CreateTexture2D(staging): {e}")))?;
            let staging = staging
                .ok_or_else(|| CaptureError::Internal("CreateTexture2D 返回空纹理".to_owned()))?;
            self.staging = Some(staging);
            self.staging_size = (w, h);
        }
        let staging = self
            .staging
            .as_ref()
            .ok_or_else(|| CaptureError::Internal("staging 纹理缺失".to_owned()))?;
        self.context.CopyResource(staging, &texture);
        let mut mapped = Default::default();
        self.context
            .Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))
            .map_err(|e| CaptureError::Internal(format!("Map: {e}")))?;
        let mapped: windows::Win32::Graphics::Direct3D11::D3D11_MAPPED_SUBRESOURCE = mapped;

        let mut pixels = vec![0u8; PixelFormat::Bgra8.buffer_size(w, h)];
        // 逐行拷贝：源行距可能大于宽度*4（GPU 对齐），目标使用紧凑排布；
        // 每行字节数必须是 宽度*4，之前误传像素数导致每行只拷到左侧 1/4 画面。
        let row_bytes = w as usize * 4;
        copy_rows(
            mapped.pData as *const u8,
            mapped.RowPitch as usize,
            &mut pixels,
            row_bytes,
            row_bytes,
            h as usize,
        );
        self.context.Unmap(staging, 0);

        // 脏区/移动矩形：有多少算多少；拿不到就留空，由编码侧按整帧处理。
        let dirty_rects = self.read_dirty_rects(frame_info);
        let move_rects = self.read_move_rects(frame_info);
        Ok(Shot {
            qpc_us: 0,
            cost_us: 0,
            dirty_rects,
            move_rects,
            pixels,
        })
    }

    /// 读取脏区矩形。
    unsafe fn read_dirty_rects(&self, info: &DXGI_OUTDUPL_FRAME_INFO) -> Vec<FrameRect> {
        let count = info.TotalMetadataBufferSize as usize;
        if count == 0 {
            return Vec::new();
        }
        // 元数据缓冲大小以字节计；RECT 为 16 字节，按上界分配并留一帧余量。
        let capacity = (count / std::mem::size_of::<RECT>()).clamp(1, 256);
        let mut rects = vec![RECT::default(); capacity];
        let mut count_out = 0u32;
        let buffer_bytes = (rects.len() * std::mem::size_of::<RECT>()) as u32;
        match self
            .duplication
            .GetFrameDirtyRects(buffer_bytes, rects.as_mut_ptr(), &mut count_out)
        {
            Ok(()) => rects[..(count_out as usize).min(rects.len())]
                .iter()
                .map(|r| rect_from_rect(r).unwrap_or_default())
                .filter(|r| r.width > 0 && r.height > 0)
                .collect(),
            Err(_) => Vec::new(),
        }
    }

    /// 读取移动矩形。
    unsafe fn read_move_rects(&self, info: &DXGI_OUTDUPL_FRAME_INFO) -> Vec<MoveRect> {
        let count = info.TotalMetadataBufferSize as usize;
        if count == 0 {
            return Vec::new();
        }
        let capacity = (count / std::mem::size_of::<windows::Win32::Graphics::Dxgi::DXGI_OUTDUPL_MOVE_RECT>())
            .clamp(1, 128);
        let mut moves =
            vec![windows::Win32::Graphics::Dxgi::DXGI_OUTDUPL_MOVE_RECT::default(); capacity];
        let mut count_out = 0u32;
        let buffer_bytes = (moves.len()
            * std::mem::size_of::<windows::Win32::Graphics::Dxgi::DXGI_OUTDUPL_MOVE_RECT>())
            as u32;
        match self
            .duplication
            .GetFrameMoveRects(buffer_bytes, moves.as_mut_ptr(), &mut count_out)
        {
            Ok(()) => moves[..(count_out as usize).min(moves.len())]
                .iter()
                .filter_map(|m| {
                    let dst = rect_from_rect(&m.DestinationRect)?;
                    // 源区域位于 `SourcePoint`，尺寸与目标矩形一致（DDA 语义）。
                    let src = FrameRect::new(
                        m.SourcePoint.x.max(0) as u32,
                        m.SourcePoint.y.max(0) as u32,
                        dst.width,
                        dst.height,
                    );
                    Some(MoveRect {
                        src,
                        dst_left: dst.left,
                        dst_top: dst.top,
                    })
                })
                .collect(),
            Err(_) => Vec::new(),
        }
    }
}

// =====** GDI 实现 =====**

/// GDI BitBlt 采集器（兼容回退，15fps 上限）。
struct GdiCapturer {
    /// 屏幕 DC。
    screen_dc: HDC,
    /// 兼容内存 DC。
    mem_dc: HDC,
    /// DIB 位图句柄。
    dib: windows::Win32::Graphics::Gdi::HBITMAP,
    /// DIB 像素指针（由 `CreateDIBSection` 分配，生命周期与本结构一致）。
    bits: *mut u8,
}

impl GdiCapturer {
    /// 创建 GDI 采集器。
    ///
    /// # Arguments
    /// * `display` - 目标显示器（取左上角与宽高）。
    ///
    /// # Returns
    /// 可用采集器。
    ///
    /// # Errors
    /// DC 或 DIB 创建失败返回 [`CaptureError::Internal`]。
    fn new(display: &DisplayInfo) -> Result<Self, CaptureError> {
        unsafe {
            let screen_dc = GetDC(GetDesktopWindow());
            if screen_dc.is_invalid() {
                return Err(CaptureError::Internal("GetDC 失败".to_owned()));
            }
            let mem_dc = CreateCompatibleDC(screen_dc);
            if mem_dc.is_invalid() {
                ReleaseDC(GetDesktopWindow(), screen_dc);
                return Err(CaptureError::Internal("CreateCompatibleDC 失败".to_owned()));
            }
            // 负高度表示 top-down DIB，读出的行序与屏幕坐标一致，省一次翻转。
            let header = BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: display.width as i32,
                biHeight: -(display.height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            };
            let info = BITMAPINFO {
                bmiHeader: header,
                ..Default::default()
            };
            let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
            let dib = CreateDIBSection(
                mem_dc,
                &info,
                DIB_RGB_COLORS,
                &mut bits,
                windows::Win32::Foundation::HANDLE::default(),
                0,
            )
            .map_err(|e| CaptureError::Internal(format!("CreateDIBSection: {e}")))?;
            if dib.is_invalid() || bits.is_null() {
                let _ = DeleteDC(mem_dc);
                ReleaseDC(GetDesktopWindow(), screen_dc);
                return Err(CaptureError::Internal("CreateDIBSection 返回空句柄".to_owned()));
            }
            let _ = SelectObject(mem_dc, dib);
            Ok(Self {
                screen_dc,
                mem_dc,
                dib,
                bits: bits as *mut u8,
            })
        }
    }

    /// 抓取一帧。
    ///
    /// # Arguments
    /// * `width` / `height` - 帧尺寸。
    ///
    /// # Returns
    /// 采集结果。
    ///
    /// # Errors
    /// BitBlt 失败返回 [`CaptureError::Internal`]。
    fn capture(&mut self, width: u32, height: u32) -> Result<Shot, CaptureError> {
        unsafe {
            let origin = display_origin();
            BitBlt(
                self.mem_dc,
                0,
                0,
                width as i32,
                height as i32,
                self.screen_dc,
                origin.0 as i32,
                origin.1 as i32,
                SRCCOPY,
            )
            .map_err(|e| CaptureError::Internal(format!("BitBlt: {e}")))?;
            let mut pixels = vec![0u8; PixelFormat::Bgra8.buffer_size(width, height)];
            // DIB 是紧凑排布（行距 = width*4），按顶部对齐直接整块拷贝。
            std::ptr::copy_nonoverlapping(self.bits, pixels.as_mut_ptr(), pixels.len());
            Ok(Shot {
                qpc_us: 0,
                cost_us: 0,
                dirty_rects: Vec::new(),
                move_rects: Vec::new(),
                pixels,
            })
        }
    }
}

impl Drop for GdiCapturer {
    /// 释放 GDI 资源。
    fn drop(&mut self) {
        unsafe {
            // 先解绑 DIB 再删除，否则位图仍被 DC 占用无法释放。
            SelectObject(
                self.mem_dc,
                windows::Win32::Graphics::Gdi::HGDIOBJ::default(),
            );
            let _ = DeleteObject(self.dib);
            let _ = DeleteDC(self.mem_dc);
            ReleaseDC(GetDesktopWindow(), self.screen_dc);
        }
    }
}

// =====** 显示器枚举 =====**

/// 枚举显示器（物理像素尺寸与刷新率）。
///
/// # Arguments
/// * 无。
///
/// # Returns
/// 按系统枚举顺序排列的显示器列表；`id` 即后续 `display_id`。
///
/// # Errors
/// 不会失败。
fn enumerate_displays() -> Vec<DisplayInfo> {
    let mut out: Vec<DisplayInfo> = Vec::new();
    // 同步回调：EnumDisplayMonitors 返回前完成全部累积，`data` 生命周期安全。
    unsafe {
        let mut data = MonitorEnumData { list: &mut out };
        let _ = EnumDisplayMonitors(
            HDC::default(),
            None,
            Some(monitor_enum_proc),
            windows::Win32::Foundation::LPARAM(&mut data as *mut _ as isize),
        );
    }
    out
}

/// 回调上下文。
struct MonitorEnumData<'a> {
    /// 输出列表。
    list: &'a mut Vec<DisplayInfo>,
}

/// 显示器枚举回调。
unsafe extern "system" fn monitor_enum_proc(
    monitor: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    data: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::BOOL {
    let data = &mut *(data.0 as *mut MonitorEnumData);
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
    if GetMonitorInfoW(
        monitor,
        &mut info as *mut MONITORINFOEXW as *mut MONITORINFO,
    )
    .as_bool()
    {
        let rect = info.monitorInfo.rcMonitor;
        let width = (rect.right - rect.left).max(0) as u32;
        let height = (rect.bottom - rect.top).max(0) as u32;
        let refresh_hz = display_refresh_hz(&info.szDevice);
        let id = data.list.len() as u32;
        data.list.push(DisplayInfo {
            id,
            name: format!("DISPLAY{id}"),
            width,
            height,
            refresh_hz,
            scale: 1.0,
            // MONITORINFOF_PRIMARY 在部分绑定版本中未导出，直接用文档值 1。
            is_primary: info.monitorInfo.dwFlags & 1 != 0,
            hdr: false,
        });
    }
    TRUE
}

/// 查询显示器当前刷新率。
unsafe fn display_refresh_hz(device: &[u16; 32]) -> u32 {
    let mut mode = DEVMODEW {
        dmSize: std::mem::size_of::<DEVMODEW>() as u16,
        ..DEVMODEW::default()
    };
    if EnumDisplaySettingsW(
        windows::core::PCWSTR(device.as_ptr()),
        ENUM_CURRENT_SETTINGS,
        &mut mode,
    )
    .as_bool()
    {
        mode.dmDisplayFrequency
    } else {
        0
    }
}

/// GDI 回退的捕获原点（主显示器为 0,0；多屏时为虚拟桌面坐标）。
fn display_origin() -> (u32, u32) {
    (0, 0)
}

// =====** 辅助函数 =====**

/// 把 D3D11 的行距拷贝成紧凑缓冲。
///
/// # Arguments
/// * `src` - 源像素起始指针。
/// * `src_pitch` - 源行距（字节）。
/// * `dst` - 目标缓冲。
/// * `dst_pitch` - 目标行距（字节）。
/// * `row_bytes` - 每行有效字节数。
/// * `rows` - 行数。
///
/// # Returns
/// 无返回值。
///
/// # Errors
/// 不会失败；越界行被跳过。
///
/// # Safety
/// `src` 必须指向至少 `rows * src_pitch` 字节的可读内存（由 `Map` 保证且在调用期间有效）。
pub(crate) unsafe fn copy_rows(
    src: *const u8,
    src_pitch: usize,
    dst: &mut [u8],
    dst_pitch: usize,
    row_bytes: usize,
    rows: usize,
) {
    for row in 0..rows {
        let dst_off = row * dst_pitch;
        if dst_off + row_bytes > dst.len() {
            break;
        }
        let src_off = row * src_pitch;
        std::ptr::copy_nonoverlapping(src.add(src_off), dst.as_mut_ptr().add(dst_off), row_bytes);
    }
}

/// 把 Win32 RECT 转成帧矩形（负坐标按 0 截断）。
fn rect_from_rect(r: &RECT) -> Option<FrameRect> {
    let left = r.left.max(0) as u32;
    let top = r.top.max(0) as u32;
    let width = (r.right - r.left).max(0) as u32;
    let height = (r.bottom - r.top).max(0) as u32;
    (width > 0 && height > 0).then_some(FrameRect {
        left,
        top,
        width,
        height,
    })
}

/// 判断错误码是否为 `DXGI_ERROR_ACCESS_LOST`。
fn is_access_lost(err: &CaptureError) -> bool {
    match err {
        CaptureError::Internal(msg) => {
            // 用 HRESULT 十进制串匹配，避免把 windows crate 的错误类型带进抽象层。
            msg.contains(&format!("{:#010X}", DXGI_ERROR_ACCESS_LOST.0))
                || msg.contains("0x887A0022")
        }
        _ => false,
    }
}

/// 读取 QPC 并换算为微秒。
///
/// # Returns
/// 单调递增的微秒时间戳。
///
/// # Errors
/// 不会失败。
fn qpc_us() -> u64 {
    static FREQ: OnceLock<i64> = OnceLock::new();
    let freq = *FREQ.get_or_init(|| {
        let mut f = 0i64;
        unsafe {
            let _ = QueryPerformanceFrequency(&mut f);
        }
        if f <= 0 {
            1
        } else {
            f
        }
    });
    let mut counter = 0i64;
    unsafe {
        let _ = QueryPerformanceCounter(&mut counter);
    }
    ((u128::from(counter as u64)) * 1_000_000 / (freq as u128)) as u64
}

/// 声明进程 DPI 感知（per-monitor v2）。
fn ensure_dpi_awareness() {
    // 已声明过（例如宿主 Flutter 进程）时返回失败，属正常情况，不报错。
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 行距大于宽度时只拷贝有效行宽，且不越界。
    #[test]
    fn copy_rows_respects_pitch() {
        let src_pitch = 8usize;
        let row_bytes = 4usize;
        let rows = 2usize;
        let mut src = vec![0u8; src_pitch * rows];
        src[0] = 1;
        src[src_pitch] = 2;
        let mut dst = vec![0u8; row_bytes * rows];
        unsafe {
            copy_rows(src.as_ptr(), src_pitch, &mut dst, row_bytes, row_bytes, rows);
        }
        assert_eq!(dst[0], 1);
        assert_eq!(dst[row_bytes], 2);
    }

    /// 负坐标 RECT 必须被截断而不是下溢。
    #[test]
    fn rect_conversion_clamps_negative() {
        let r = RECT {
            left: -10,
            top: -10,
            right: 20,
            bottom: 20,
        };
        let rect = rect_from_rect(&r).expect("有效矩形");
        assert_eq!(rect.left, 0);
        assert_eq!(rect.top, 0);
        assert_eq!(rect.width, 30);
        assert_eq!(rect.height, 30);

        let empty = RECT {
            left: 10,
            top: 10,
            right: 10,
            bottom: 10,
        };
        assert!(rect_from_rect(&empty).is_none());
    }

    /// 后端名称必须稳定（报告与日志按它归类）。
    #[test]
    fn backend_names_are_stable() {
        assert_eq!(Backend::Dxgi.name(), "dxgi-desktop-duplication");
        assert_eq!(Backend::Gdi.name(), "gdi-bitblt");
    }
}
