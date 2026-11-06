//! rdc-capture / macOS 平台采集后端（S1 空壳）
//!
//! 层级：L3 采集层 · 平台适配
//! 用途：macOS 桌面采集入口；stage3 落地 ScreenCaptureKit（SCK）流输出
//! 调用方：rdc-capture::platform::create_source（由 rdc-ctl 在会话建立时调用）

use std::sync::Arc;

use crate::source::{CaptureConfig, CaptureSource, DisplayInfo, FrameSink};
use crate::CaptureError;

// =====** PlatformCapture =====**

/// macOS 采集源（S1 空壳，未持有任何平台句柄）。
///
/// TODO(stage3)：使用 ScreenCaptureKit 的 `SCStream` 输出 BGRA 帧，
/// 首次使用需引导用户授予「屏幕录制」权限（缺失即 `PermissionDenied`）。
pub struct PlatformCapture;

impl PlatformCapture {
    /// 构造未绑定的 macOS 采集源。
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
    /// S1 固定为空列表；TODO(stage3) 经 SCShareableContent 枚举显示器的 Retina 物理分辨率。
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
    /// stage3 接入 SCK 后返回权限或显示器错误。
    fn start(&mut self, _config: CaptureConfig) -> Result<(), CaptureError> {
        // TODO(stage3)：创建 SCStream 与 SCStreamConfiguration，启动 rdc-cap-{display} 线程。
        todo!("TODO(stage3)：macOS 采集（ScreenCaptureKit）在 stage3 实现")
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
        // TODO(stage3)：保存 sink，供 SCStream 输出回调写入 SPSC 环 A。
        todo!("TODO(stage3)：macOS 采集 sink 绑定在 stage3 实现")
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
    /// stage3 返回 SCStream 重配失败或显示器变更错误。
    fn reconfigure(&mut self, _config: CaptureConfig) -> Result<(), CaptureError> {
        // TODO(stage3)：更新 SCStreamConfiguration（帧率/尺寸/光标合成）。
        todo!("TODO(stage3)：macOS 采集重配在 stage3 实现")
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
        // TODO(stage3)：stopCapture 并 join 采集线程。
        todo!("TODO(stage3)：macOS 采集停止在 stage3 实现")
    }
}
