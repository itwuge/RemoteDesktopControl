//! rdc-capture / 采集契约类型
//!
//! 层级：L3 采集层
//! 用途：定义显示器信息、色度格式、采集配置、帧消费者与采集源 trait（平台无关）
//! 调用方：rdc-capture 各平台后端实现 [`CaptureSource`]；rdc-core 通过该 trait 驱动采集

use std::sync::Arc;

use crate::{CaptureError, Frame};

// =====** DisplayInfo =====**

/// 可采集显示器的静态信息。
#[derive(Debug, Clone)]
pub struct DisplayInfo {
    /// 平台分配且在本会话内稳定的显示器编号。
    pub id: u32,
    /// 显示器名称（平台原样返回，仅用于展示与日志）。
    pub name: String,
    /// 当前模式宽度，单位：物理像素。
    pub width: u32,
    /// 当前模式高度，单位：物理像素。
    pub height: u32,
    /// 当前刷新率，单位：Hz；`0` 表示平台不提供。
    pub refresh_hz: u32,
    /// DPI 缩放比（`1.0` 表示 100%）。
    pub scale: f32,
    /// 是否系统主显示器。
    pub is_primary: bool,
    /// 是否处于 HDR 模式。
    pub hdr: bool,
}

// =====** ChromaFormat =====**

/// 采集输出的色度格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChromaFormat {
    /// NV12：Y 平面 + 交织 UV，硬件编码器首选。
    Nv12,
    /// I420：Y/U/V 三平面。
    I420,
    /// BGRA 直通：8 bit 4:4:4，兼容性兜底，带宽最高。
    Bgra,
}

// =====** CaptureConfig =====**

/// 单次采集会话配置。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CaptureConfig {
    /// 目标显示器编号，必须来自 [`CaptureSource::enumerate`] 的结果。
    pub display_id: u32,
    /// 目标帧率，单位：Hz；实际帧率受显示器刷新率限制。
    pub target_fps: u32,
    /// 是否把光标合成进采集画面。
    pub include_cursor: bool,
    /// 输出色度格式。
    pub chroma: ChromaFormat,
    /// 输出位深（8 或 10）。
    pub bit_depth: u8,
}

// =====** FrameSink =====**

/// 帧消费者。
///
/// 调用线程：`rdc-cap-{display}`（每个显示器一个采集线程）。
/// 时延约束：`on_frame` 必须在 1ms 内返回，禁止在此做编码、IO 或锁等待。
/// 写入目标：只允许写入 SPSC 环 A（容量 [`crate::params::RING_CAP_TO_ENC_CAP`]）；
/// 环满即丢帧，由实现方保证不阻塞采集线程。
pub trait FrameSink: Send + Sync {
    /// 接收一帧采集结果。
    ///
    /// # Arguments
    /// * `frame` - 池化帧；消费者 drop 后缓冲自动归还内存池。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败；无法接收时静默丢帧，并通过统计计数上报。
    fn on_frame(&self, frame: Frame);
}

// =====** CaptureSource =====**

/// 平台无关的采集源。
///
/// 生命周期：`enumerate` → `set_sink` → `start` →（可选 `reconfigure`）→ `stop`。
/// 实现必须是 `Send`：会话建立后由 rdc-ctl 移交给采集线程。
pub trait CaptureSource: Send {
    /// 枚举当前可用显示器。
    ///
    /// 调用线程：rdc-ctl（会话建立前一次性调用）。
    /// 阻塞性：可能阻塞（平台枚举为同步调用），仅允许在会话建立阶段使用。
    /// 内部队列：无。
    ///
    /// # Arguments
    /// * 无。
    ///
    /// # Returns
    /// 显示器列表；无显示器或平台不支持时返回空列表。
    ///
    /// # Errors
    /// 不会失败；错误通过空列表表达，由 `start` 再给出具体原因。
    fn enumerate(&self) -> Vec<DisplayInfo>;

    /// 按配置启动采集。
    ///
    /// 调用线程：rdc-ctl。
    /// 阻塞性：短阻塞（平台初始化可能耗时数十毫秒），禁止在媒体线程调用。
    /// 内部队列：无——采集线程在此之后创建，帧直接写入 SPSC 环 A。
    ///
    /// # Arguments
    /// * `config` - 目标显示器、帧率、光标合成与色度格式等采集参数。
    ///
    /// # Returns
    /// 启动成功返回 `Ok(())`。
    ///
    /// # Errors
    /// 权限缺失、显示器不存在/被占用、平台不支持或后端初始化失败时返回 [`CaptureError`]。
    fn start(&mut self, config: CaptureConfig) -> Result<(), CaptureError>;

    /// 注册帧消费者；必须在 [`CaptureSource::start`] 之前调用。
    ///
    /// 调用线程：rdc-ctl。
    /// 阻塞性：不阻塞。
    /// 内部队列：无。
    ///
    /// # Arguments
    /// * `sink` - 帧消费者，内部以 `Arc` 共享给采集线程。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    fn set_sink(&mut self, sink: Arc<dyn FrameSink>);

    /// 运行期重配（分辨率、帧率或显示器拓扑变化）。
    ///
    /// 调用线程：rdc-ctl。
    /// 阻塞性：短阻塞（需重建平台会话），期间采集线程暂停。
    /// 内部队列：无——重配不缓存待处理帧，旧帧直接丢弃。
    ///
    /// # Arguments
    /// * `config` - 新的采集参数。
    ///
    /// # Returns
    /// 重配成功返回 `Ok(())`。
    ///
    /// # Errors
    /// 显示器已消失、被占用或平台重配失败时返回 [`CaptureError`]。
    fn reconfigure(&mut self, config: CaptureConfig) -> Result<(), CaptureError>;

    /// 停止采集并释放平台句柄。
    ///
    /// 调用线程：rdc-ctl。
    /// 阻塞性：阻塞至采集线程退出（有上界，超时由 rdc-core 监管）。
    /// 内部队列：无。
    ///
    /// # Arguments
    /// * 无。
    ///
    /// # Returns
    /// 无返回值；未启动时调用为空操作。
    ///
    /// # Errors
    /// 不会失败。
    fn stop(&mut self);
}
