//! rdc-capture / Linux 平台采集后端（S1 空壳）
//!
//! 层级：L3 采集层 · 平台适配
//! 用途：Linux 桌面采集入口；stage3 以 PipeWire 为主，X11 作为无 portal 环境的回退
//! 调用方：rdc-capture::platform::create_source（由 rdc-ctl 在会话建立时调用）

use std::sync::Arc;

use crate::source::{CaptureConfig, CaptureSource, DisplayInfo, FrameSink};
use crate::CaptureError;

// =====** PlatformCapture =====**

/// Linux 采集源（S1 空壳，未持有任何平台句柄）。
///
/// TODO(stage3)：优先 PipeWire（xdg-desktop-portal ScreenCast，兼容 Wayland），
/// X11 会话回退 `XGetImage` / XShm；portal 授权被拒时返回 `PermissionDenied`。
pub struct PlatformCapture;

impl PlatformCapture {
    /// 构造未绑定的 Linux 采集源。
    ///
    /// # Arguments
    /// * 无。
    ///
    /// # Returns
    /// 空壳实例；显示器与 sink 需在 [`CaptureSource::start`] 之前设置。
    ///
    /// # Errors
    /// 不会失败（真实后端在 stage3 于 `start` 中返回错误）。
    pub fn new() -> Self {
        Self
    }
}

// =====** CaptureSource 实现 =====**

impl CaptureSource for PlatformCapture {
    /// 枚举显示器。
    ///
    /// S1 返回空列表。
    ///
    /// # Arguments
    /// * 无。
    ///
    /// # Returns
    /// S1 固定为空列表；TODO(stage3) 经 portal 或 XRandR 枚举输出与缩放比。
    ///
    /// # Errors
    /// 不会失败。
    fn enumerate(&self) -> Vec<DisplayInfo> {
        Vec::new()
    }

    /// 启动采集。
    ///
    /// # Arguments
    /// * `_config` - 采集参数（S1 未使用）。
    ///
    /// # Returns
    /// stage3 之前不会正常返回。
    ///
    /// # Errors
    /// stage3 接入 PipeWire/X11 后返回授权或设备错误。
    fn start(&mut self, _config: CaptureConfig) -> Result<(), CaptureError> {
        // TODO(stage3)：建立 PipeWire 流（或 XShm），启动 rdc-cap-{display} 线程。
        todo!("TODO(stage3)：Linux 采集（PipeWire / X11）在 stage3 实现")
    }

    /// 注册帧消费者。
    ///
    /// # Arguments
    /// * `_sink` - 帧消费者（S1 未保存）。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// stage3 之前不会正常返回。
    fn set_sink(&mut self, _sink: Arc<dyn FrameSink>) {
        // TODO(stage3)：保存 sink，供 PipeWire 回调写入 SPSC 环 A。
        todo!("TODO(stage3)：Linux 采集 sink 绑定在 stage3 实现")
    }

    /// 运行期重配。
    ///
    /// # Arguments
    /// * `_config` - 新采集参数（S1 未使用）。
    ///
    /// # Returns
    /// stage3 之前不会正常返回。
    ///
    /// # Errors
    /// stage3 返回流重配失败或显示器变更错误。
    fn reconfigure(&mut self, _config: CaptureConfig) -> Result<(), CaptureError> {
        // TODO(stage3)：按新参数重新协商 PipeWire 格式，或重建 X11 抓取区域。
        todo!("TODO(stage3)：Linux 采集重配在 stage3 实现")
    }

    /// 停止采集并释放平台句柄。
    ///
    /// # Arguments
    /// * 无。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// stage3 之前不会正常返回。
    fn stop(&mut self) {
        // TODO(stage3)：销毁 PipeWire 流并 join 采集线程。
        todo!("TODO(stage3)：Linux 采集停止在 stage3 实现")
    }
}
