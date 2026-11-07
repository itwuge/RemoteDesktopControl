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
    ///
    /// 仅服务于旧的 [`InputMapper`](crate::mapper::InputMapper) 线性映射；
    /// [`ViewTransform`](crate::mapper::ViewTransform) 以“物理桌面尺寸 / 视图内容尺寸”定标，
    /// DPI 缩放已含在该比值里，不再乘一次 `scale`。
    pub scale: f32,
}

impl Viewport {
    /// 视口是否可用于坐标映射与绝对坐标归一化。
    ///
    /// # Returns
    /// 宽高非 0 且缩放比为有限正数时返回 `true`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn is_valid(&self) -> bool {
        self.width > 0 && self.height > 0 && self.scale > 0.0 && self.scale.is_finite()
    }

    /// 把桌面坐标钳制到视口范围内（含边界最后一行/列）。
    ///
    /// # Arguments
    /// * `x` - 桌面横坐标（物理像素）
    /// * `y` - 桌面纵坐标（物理像素）
    ///
    /// # Returns
    /// 落在 `[x, x + width - 1] × [y, y + height - 1]` 内的坐标；非法视口原样返回。
    ///
    /// # Errors
    /// 不会失败。
    pub fn clamp_point(&self, x: i32, y: i32) -> (i32, i32) {
        if !self.is_valid() {
            return (x, y);
        }
        let max_x = self.x.saturating_add(self.width as i32 - 1);
        let max_y = self.y.saturating_add(self.height as i32 - 1);
        (x.clamp(self.x, max_x), y.clamp(self.y, max_y))
    }
}

/// 把桌面物理坐标换算成 `SendInput` 要求的 0..=65535 绝对坐标。
///
/// 规则与 Win32 一致：以视口宽高各减一为跨度做线性归一化，因此左上角为 `(0,0)`、
/// 右下角为 `(65535,65535)`；多屏通过视口原点（可为负）表达。
///
/// # Arguments
/// * `x` - 桌面横坐标（物理像素）
/// * `y` - 桌面纵坐标（物理像素）
/// * `viewport` - 目标桌面矩形（原点 + 宽高）
///
/// # Returns
/// 归一化后的 `(u16, u16)`；输入先钳制到矩形内。
///
/// # Errors
/// 视口非法（宽高为 0）时返回 [`InputError::InvalidViewport`]。
pub fn normalize_absolute(x: i32, y: i32, viewport: Viewport) -> Result<(u16, u16), InputError> {
    if !viewport.is_valid() {
        return Err(InputError::InvalidViewport);
    }
    let (x, y) = viewport.clamp_point(x, y);
    // 宽度为 1 时跨度为 1，坐标为 0，不会除零。
    let span_x = (viewport.width - 1).max(1) as f64;
    let span_y = (viewport.height - 1).max(1) as f64;
    let nx = ((x - viewport.x) as f64 / span_x) * 65535.0;
    let ny = ((y - viewport.y) as f64 / span_y) * 65535.0;
    Ok((
        nx.round().clamp(0.0, 65535.0) as u16,
        ny.round().clamp(0.0, 65535.0) as u16,
    ))
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

// =====** 自检：绝对坐标归一化 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 主屏 1920×1080。
    fn primary() -> Viewport {
        Viewport {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            scale: 1.0,
        }
    }

    /// 左上角为 (0,0)，右下角为 (65535,65535)。
    #[test]
    fn corners_map_to_full_range() {
        assert_eq!(normalize_absolute(0, 0, primary()).expect("左上"), (0, 0));
        assert_eq!(
            normalize_absolute(1919, 1079, primary()).expect("右下"),
            (65535, 65535)
        );
        // 越界输入先钳制，不允许溢出到 0..65535 之外。
        assert_eq!(
            normalize_absolute(5000, -5000, primary()).expect("钳制"),
            (65535, 0)
        );
    }

    /// 多屏负原点：原点映射到 0，右边界映射到 65535。
    #[test]
    fn negative_origin_is_virtual_desktop() {
        let two_screens = Viewport {
            x: -1920,
            y: 0,
            width: 3840,
            height: 1080,
            scale: 1.0,
        };
        assert_eq!(
            normalize_absolute(-1920, 0, two_screens).expect("左屏左上"),
            (0, 0)
        );
        assert_eq!(
            normalize_absolute(1919, 1079, two_screens).expect("右屏右下"),
            (65535, 65535)
        );
        // 中间分隔处约在 50%。
        let (mid, _) = normalize_absolute(0, 540, two_screens).expect("跨屏中点");
        assert!(mid.abs_diff(32768) < 100, "跨屏中点应接近一半: {mid}");
    }

    /// 非法视口必须报错而不是除零。
    #[test]
    fn invalid_viewport_is_rejected() {
        let zero = Viewport {
            x: 0,
            y: 0,
            width: 0,
            height: 100,
            scale: 1.0,
        };
        assert!(matches!(
            normalize_absolute(0, 0, zero),
            Err(InputError::InvalidViewport)
        ));
        let bad_scale = Viewport {
            x: 0,
            y: 0,
            width: 100,
            height: 100,
            scale: 0.0,
        };
        assert!(!bad_scale.is_valid());
    }

    /// 宽度为 1 时不应除零，钳制后恒为 0。
    #[test]
    fn one_pixel_viewport_is_safe() {
        let one = Viewport {
            x: 5,
            y: 5,
            width: 1,
            height: 1,
            scale: 1.0,
        };
        assert_eq!(normalize_absolute(5, 5, one).expect("单像素"), (0, 0));
        assert_eq!(normalize_absolute(9, 9, one).expect("钳制"), (0, 0));
    }
}
