//! rdc-capture / Windows 平台采集后端（S1 空壳）
//!
//! 层级：L3 采集层 · 平台适配
//! 用途：Windows 桌面采集入口；stage3 落地 DXGI Desktop Duplication，WGC 作为 HDR 回退
//! 调用方：rdc-capture::platform::create_source（由 rdc-ctl 在会话建立时调用）

use std::sync::Arc;

use crate::source::{CaptureConfig, CaptureSource, DisplayInfo, FrameSink};
use crate::CaptureError;

// =====** PlatformCapture =====**

/// Windows 采集源（S1 空壳，未持有任何平台句柄）。
///
/// TODO(stage3)：优先 `IDXGIOutputDuplication::AcquireNextFrame`（低时延、脏矩形），
/// HDR 与多平面场景回退 `Windows.Graphics.Capture`（WGC）帧池。
pub struct PlatformCapture;

impl PlatformCapture {
    /// 构造未绑定的 Windows 采集源。
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
    /// S1 固定为空列表；TODO(stage3) 经 DXGI 适配器枚举输出并填充分辨率/刷新率/主屏标记。
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
    /// stage3 接入 DXGI 后返回权限或设备错误。
    fn start(&mut self, _config: CaptureConfig) -> Result<(), CaptureError> {
        // TODO(stage3)：创建 D3D11 设备、复制输出并启动 rdc-cap-{display} 线程。
        todo!("TODO(stage3)：Windows 采集（DXGI Desktop Duplication / WGC）在 stage3 实现")
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
        // TODO(stage3)：保存 sink，供采集线程写入 SPSC 环 A。
        todo!("TODO(stage3)：Windows 采集 sink 绑定在 stage3 实现")
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
    /// stage3 返回 DXGI 重配失败或显示器变更错误。
    fn reconfigure(&mut self, _config: CaptureConfig) -> Result<(), CaptureError> {
        // TODO(stage3)：按新拓扑重建 duplication，DisplayDisappeared / DisplayBusy 在此上报。
        todo!("TODO(stage3)：Windows 采集重配在 stage3 实现")
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
        // TODO(stage3)：join 采集线程并释放 duplication 句柄。
        todo!("TODO(stage3)：Windows 采集停止在 stage3 实现")
    }
}
