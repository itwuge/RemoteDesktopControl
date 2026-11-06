//! rdc-input / 视口与注入器契约
//!
//! 层级：L3 输入层
//! 用途：定义远端显示区域描述与平台无关的输入注入器 trait
//! 调用方：rdc-core（创建注入器并交给 rdc-in 线程）、rdc-input 各平台注入器实现

use crate::error::InputError;
use crate::event::InputEvent;

// =====** Viewport =====**

/// 远端显示区域描述，用于局部坐标到远端坐标的映射。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Viewport {
    /// 远端桌面坐标原点 X，单位：物理像素。
    pub x: i32,
    /// 远端桌面坐标原点 Y，单位：物理像素。
    pub y: i32,
    /// 区域宽度，单位：物理像素。
    pub width: u32,
    /// 区域高度，单位：物理像素。
    pub height: u32,
    /// 远端 DPI 缩放比（`1.0` 表示 100%）。
    pub scale: f32,
}

// =====** InputInjector =====**

/// 平台无关的输入注入器。
///
/// 调用线程：`rdc-in`（全进程最高优先级线程）。
/// 时延预算：单次 [`InputInjector::inject`] 必须 ≤1ms；超预算视为后端异常，由 rdc-core 降载。
/// 排队约定：本接口不排队——256 深的输入队列在 rdc-core，事件进入本接口前已完成消峰，
/// 因此实现方无需也不允许再维护缓冲队列。
pub trait InputInjector: Send {
    /// 注入单个输入事件。
    ///
    /// 调用线程：`rdc-in`（最高优先级线程）。
    /// 阻塞性：不阻塞（单次 ≤1ms）；禁止同步等待系统响应。
    /// 内部队列：无——失败即丢，不重试。
    ///
    /// # Arguments
    /// * `event` - 已完成坐标映射的输入事件。
    ///
    /// # Returns
    /// 注入成功返回 `Ok(())`。
    ///
    /// # Errors
    /// 平台不支持返回 [`InputError::Unsupported`]；系统拒绝返回
    /// [`InputError::InjectionRejected`]。
    fn inject(&mut self, event: InputEvent) -> Result<(), InputError>;

    /// 更新视口，供后续坐标映射与缩放使用。
    ///
    /// 调用线程：`rdc-in`（最高优先级线程）。
    /// 阻塞性：不阻塞。
    /// 内部队列：无。
    ///
    /// # Arguments
    /// * `viewport` - 新的远端显示区域描述。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败；非法视口在 [`InputMapper`](crate::mapper::InputMapper) 层拒绝。
    fn set_viewport(&mut self, viewport: Viewport);
}
