//! rdc-capture / 非桌面平台采集桩
//!
//! 层级：L3 采集层 · 平台适配
//! 用途：为移动端与其它目标提供可编译的采集源，所有能力一律返回 Unsupported
//! 调用方：rdc-capture::platform::create_source（非桌面目标）

use std::sync::Arc;

use crate::source::{CaptureConfig, CaptureSource, DisplayInfo, FrameSink};
use crate::CaptureError;

// =====** PlatformCapture =====**

/// 非桌面平台的采集桩：不持有任何状态，不做平台调用。
pub struct PlatformCapture;

impl PlatformCapture {
    /// 构造桩实例。
    ///
    /// # Arguments
    /// * 无。
    ///
    /// # Returns
    /// 无状态的桩实例。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new() -> Self {
        Self
    }
}

// =====** CaptureSource 实现 =====**

impl CaptureSource for PlatformCapture {
    /// 枚举显示器。
    ///
    /// # Arguments
    /// * 无。
    ///
    /// # Returns
    /// 空列表（该平台无采集能力）。
    ///
    /// # Errors
    /// 不会失败。
    fn enumerate(&self) -> Vec<DisplayInfo> {
        // 平台没有可用后端，不存在可枚举的显示器。
        Vec::new()
    }

    /// 启动采集（永远拒绝）。
    ///
    /// # Arguments
    /// * `_config` - 采集参数（被忽略）。
    ///
    /// # Returns
    /// 不会返回 `Ok`。
    ///
    /// # Errors
    /// 恒返回 [`CaptureError::Unsupported`]。
    fn start(&mut self, _config: CaptureConfig) -> Result<(), CaptureError> {
        Err(CaptureError::Unsupported)
    }

    /// 注册帧消费者（空操作）。
    ///
    /// # Arguments
    /// * `_sink` - 帧消费者（被忽略）。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    fn set_sink(&mut self, _sink: Arc<dyn FrameSink>) {
        // 无后端可持有 sink，保留空操作以维持契约。
    }

    /// 运行期重配（永远拒绝）。
    ///
    /// # Arguments
    /// * `_config` - 新采集参数（被忽略）。
    ///
    /// # Returns
    /// 不会返回 `Ok`。
    ///
    /// # Errors
    /// 恒返回 [`CaptureError::Unsupported`]。
    fn reconfigure(&mut self, _config: CaptureConfig) -> Result<(), CaptureError> {
        Err(CaptureError::Unsupported)
    }

    /// 停止采集（空操作）。
    ///
    /// # Arguments
    /// * 无。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    fn stop(&mut self) {
        // 从未启动，无需释放资源。
    }
}
