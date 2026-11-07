//! rdc-input / 坐标映射
//!
//! 层级：L3 输入层
//! 用途：提供 3x3 齐次变换矩阵与默认线性映射器，把画面局部坐标转成远端桌面坐标
//! 调用方：rdc-core（rdc-in 线程在注入前调用）、rdc-input 平台注入器（可选复用）

use crate::injector::Viewport;

// =====** TransformMatrix =====**

/// 3x3 行主序齐次变换矩阵。
///
/// S1 只提供基础算子；缩放/旋转/翻转的组合由 rdc-core 在 stage4 组装后传入。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TransformMatrix {
    /// 行主序元素：`m[row][col]`。
    pub m: [[f32; 3]; 3],
}

impl TransformMatrix {
    /// 构造单位矩阵（恒等变换）。
    ///
    /// # Arguments
    /// * 无。
    ///
    /// # Returns
    /// 对角线为 `1`、其余为 `0` 的 3x3 矩阵。
    ///
    /// # Errors
    /// 不会失败。
    pub fn identity() -> Self {
        Self {
            m: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        }
    }

    /// 对点 `(x, y)` 做齐次变换。
    ///
    /// # Arguments
    /// * `x` - 输入点横坐标（局部坐标系）。
    /// * `y` - 输入点纵坐标（局部坐标系）。
    ///
    /// # Returns
    /// 变换后的 `(x', y')`；齐次分量 `w == 0`（退化矩阵）时返回 `(0.0, 0.0)`，
    /// 避免 NaN/Inf 扩散到注入坐标。
    ///
    /// # Errors
    /// 不会失败。
    pub fn map_point(&self, x: f32, y: f32) -> (f32, f32) {
        let nx = self.m[0][0] * x + self.m[0][1] * y + self.m[0][2];
        let ny = self.m[1][0] * x + self.m[1][1] * y + self.m[1][2];
        let w = self.m[2][0] * x + self.m[2][1] * y + self.m[2][2];
        // 齐次坐标必须先除 w：仿射矩阵 w 恒为 1，投影矩阵则可能是任意值。
        if w == 0.0 {
            // 退化矩阵无有效映射，返回原点而不是产生 NaN。
            return (0.0, 0.0);
        }
        (nx / w, ny / w)
    }
}

// =====** InputMapper =====**

/// 把采集画面上的局部坐标映射为远端桌面坐标。
///
/// 调用线程：`rdc-in`（最高优先级线程）。
/// 约束：`&self` 无锁只读，单次 ≤1ms，禁止分配与系统调用。
pub trait InputMapper: Send + Sync {
    /// 映射单个指针位置。
    ///
    /// 调用线程：`rdc-in`（最高优先级线程）。
    /// 阻塞性：不阻塞。
    /// 内部队列：无。
    ///
    /// # Arguments
    /// * `local_x` - 画面局部横坐标（物理像素），由调用方保证落在采集帧内。
    /// * `local_y` - 画面局部纵坐标（物理像素），由调用方保证落在采集帧内。
    /// * `viewport` - 远端显示区域描述（原点、尺寸与 DPI 缩放）。
    ///
    /// # Returns
    /// 远端桌面坐标 `(x, y)`，单位：物理像素。
    ///
    /// # Errors
    /// 不会失败；越界由调用方保证不会发生，超出部分按 i32 饱和转换。
    fn map_pointer(&self, local_x: f32, local_y: f32, viewport: Viewport) -> (i32, i32);
}

// =====** DefaultInputMapper =====**

/// 默认线性映射器：局部坐标按视口缩放后平移到远端桌面原点。
pub struct DefaultInputMapper;

impl InputMapper for DefaultInputMapper {
    /// 线性映射：`远端 = 视口原点 + 局部坐标 × scale`。
    ///
    /// # Arguments
    /// * `local_x` - 画面局部横坐标。
    /// * `local_y` - 画面局部纵坐标。
    /// * `viewport` - 远端显示区域描述。
    ///
    /// # Returns
    /// 远端桌面坐标 `(x, y)`。
    ///
    /// # Errors
    /// 不会失败。
    fn map_pointer(&self, local_x: f32, local_y: f32, viewport: Viewport) -> (i32, i32) {
        // 采用 scale 而非 width/height 归一化：局部坐标已是采集帧物理像素，
        // 只有 DPI 缩放会改变其与远端逻辑坐标的倍率。
        let remote_x = viewport.x + (local_x * viewport.scale) as i32;
        let remote_y = viewport.y + (local_y * viewport.scale) as i32;
        (remote_x, remote_y)
    }
}

// =====** ViewTransform =====**

/// 控制端视图坐标 → 被控端桌面物理坐标。
///
/// 三个矩形的关系：
/// - `view_*`：控制端视频视图的整体尺寸（逻辑像素，左上角 `(0,0)`）；
/// - `video_*`：视频画面在视图内实际占用的矩形（逻辑像素，含 letterbox 黑边偏移）；
/// - `desktop`：被控端桌面（物理像素，`x/y` 为多屏拼接后的原点，可为负）。
///
/// DPI 缩放体现在“物理桌面尺寸 / 逻辑视图内容尺寸”的比值上：例如 1920×1080 @125%
/// 的桌面，视图内容为 1536×864 逻辑像素，比值 1.25 就是把 100% 缩放补回来的部分。
/// 因此不要再额外乘 `Viewport::scale`，否则会重复缩放。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewTransform {
    /// 视图整体宽度（控制端逻辑像素）。
    pub view_width: f32,
    /// 视图整体高度（控制端逻辑像素）。
    pub view_height: f32,
    /// 视频内容在视图内的左偏移（逻辑像素，letterbox）。
    pub video_x: f32,
    /// 视频内容在视图内的上偏移（逻辑像素，letterbox）。
    pub video_y: f32,
    /// 视频内容宽度（逻辑像素）。
    pub video_width: f32,
    /// 视频内容高度（逻辑像素）。
    pub video_height: f32,
    /// 被控端桌面矩形（物理像素）。
    pub desktop: Viewport,
}

impl ViewTransform {
    /// 按“保持宽高比、居中、留黑边”计算视图内视频内容矩形。
    ///
    /// # Arguments
    /// * `desktop` - 被控端桌面矩形（物理像素，原点可为负）
    /// * `view_width` - 控制端视图宽度（逻辑像素）
    /// * `view_height` - 控制端视图高度（逻辑像素）
    ///
    /// # Returns
    /// 内容矩形为空的 `ViewTransform`（视图或桌面非法时 `video_width/height` 为 0，
    /// 后续 [`Self::map_view_point`] 一律返回 `None`）。
    ///
    /// # Errors
    /// 不会失败；非法输入用空内容矩形表达。
    pub fn fit(desktop: Viewport, view_width: f32, view_height: f32) -> Self {
        if !desktop.is_valid() || view_width <= 0.0 || view_height <= 0.0 {
            return Self {
                view_width,
                view_height,
                video_x: 0.0,
                video_y: 0.0,
                video_width: 0.0,
                video_height: 0.0,
                desktop,
            };
        }
        let scale_x = view_width / desktop.width as f32;
        let scale_y = view_height / desktop.height as f32;
        let scale = scale_x.min(scale_y);
        let video_width = desktop.width as f32 * scale;
        let video_height = desktop.height as f32 * scale;
        Self {
            view_width,
            view_height,
            video_x: (view_width - video_width) * 0.5,
            video_y: (view_height - video_height) * 0.5,
            video_width,
            video_height,
            desktop,
        }
    }

    /// 把控制端视图坐标映射为被控端桌面物理坐标。
    ///
    /// 调用线程：`rdc-in`（最高优先级线程）；无分配、无系统调用。
    ///
    /// # Arguments
    /// * `view_x` - 视图横坐标（控制端逻辑像素）
    /// * `view_y` - 视图纵坐标（控制端逻辑像素）
    ///
    /// # Returns
    /// 落在视频内容内时返回桌面坐标 `(x, y)`（物理像素，已钳制到最后一行/列）；
    /// 落在黑边或内容矩形非法时返回 `None`，调用方应丢弃该事件而不是注入错误坐标。
    ///
    /// # Errors
    /// 不会失败。
    pub fn map_view_point(&self, view_x: f32, view_y: f32) -> Option<(i32, i32)> {
        if self.video_width <= 0.0 || self.video_height <= 0.0 || !self.desktop.is_valid() {
            return None;
        }
        // 闭区间：内容右/下边界视为落在最后一个像素上。
        if !(view_x >= self.video_x && view_x <= self.video_x + self.video_width)
            || !(view_y >= self.video_y && view_y <= self.video_y + self.video_height)
        {
            return None;
        }
        let nx = ((view_x - self.video_x) / self.video_width).clamp(0.0, 1.0) as f64;
        let ny = ((view_y - self.video_y) / self.video_height).clamp(0.0, 1.0) as f64;
        let x = self.desktop.x as f64 + nx * self.desktop.width as f64;
        let y = self.desktop.y as f64 + ny * self.desktop.height as f64;
        Some(self.desktop.clamp_point(x.round() as i32, y.round() as i32))
    }
}

// =====** 自检 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 主屏 1920×1080，视图同尺寸。
    fn desktop_1080p() -> Viewport {
        Viewport {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            scale: 1.0,
        }
    }

    /// 1:1 视图：坐标必须原样穿过，不产生 ±1 偏移。
    #[test]
    fn identity_mapping() {
        let t = ViewTransform::fit(desktop_1080p(), 1920.0, 1080.0);
        assert_eq!(t.map_view_point(100.0, 200.0), Some((100, 200)));
        assert_eq!(t.map_view_point(0.0, 0.0), Some((0, 0)));
        // 右下边界是闭区间，钳制到最后一个像素而不是越界。
        assert_eq!(t.map_view_point(1920.0, 1080.0), Some((1919, 1079)));
    }

    /// letterbox：4:3 视图装 16:9 内容，中心对中心，黑边返回 None。
    #[test]
    fn letterbox_centers_content() {
        // 视图 1600×1200（4:3），内容 16:9 → 1600×900，上下各留 150。
        let t = ViewTransform::fit(desktop_1080p(), 1600.0, 1200.0);
        assert_eq!(t.video_width, 1600.0);
        assert_eq!(t.video_height, 900.0);
        assert_eq!(t.video_y, 150.0);
        // 视图中心 → 桌面中心附近。
        let (cx, cy) = t.map_view_point(800.0, 600.0).expect("内容内");
        assert_eq!((cx, cy), (960, 540));
        // 上下黑边不得映射。
        assert_eq!(t.map_view_point(800.0, 100.0), None);
        assert_eq!(t.map_view_point(-1.0, 600.0), None);
    }

    /// 125% DPI：物理 1920×1080 桌面显示在 1536×864 逻辑视图里，比例 1.25。
    #[test]
    fn dpi_125_mapping() {
        let t = ViewTransform::fit(desktop_1080p(), 1536.0, 864.0);
        // 逻辑 (768, 432) 是内容中心 → 物理中心 (960, 540)。
        assert_eq!(t.map_view_point(768.0, 432.0), Some((960, 540)));
        // 逻辑 (300, 200) → 物理 (375, 250)。
        assert_eq!(t.map_view_point(300.0, 200.0), Some((375, 250)));
    }

    /// 多屏负原点：左屏视图左上角映射到 -1920。
    #[test]
    fn multi_monitor_offset() {
        let desktop = Viewport {
            x: -1920,
            y: 0,
            width: 3840,
            height: 1080,
            scale: 1.0,
        };
        let t = ViewTransform::fit(desktop, 3840.0, 1080.0);
        assert_eq!(t.map_view_point(0.0, 0.0), Some((-1920, 0)));
        assert_eq!(t.map_view_point(1920.0, 540.0), Some((0, 540)));
        assert_eq!(t.map_view_point(3839.0, 1079.0), Some((1919, 1079)));
    }

    /// 非法视图/桌面：不映射、不 panic。
    #[test]
    fn degenerate_inputs_return_none() {
        let zero_view = ViewTransform::fit(desktop_1080p(), 0.0, 0.0);
        assert!(zero_view.map_view_point(0.0, 0.0).is_none());
        let zero_desktop = Viewport {
            x: 0,
            y: 0,
            width: 0,
            height: 0,
            scale: 1.0,
        };
        let t = ViewTransform::fit(zero_desktop, 800.0, 600.0);
        assert!(t.map_view_point(400.0, 300.0).is_none());
    }

    /// 旧线性映射器保持既有语义（局部采集像素 × scale + 原点）。
    #[test]
    fn default_mapper_keeps_legacy_semantics() {
        let viewport = Viewport {
            x: -1920,
            y: 0,
            width: 3840,
            height: 1080,
            scale: 1.25,
        };
        let mapper = DefaultInputMapper;
        assert_eq!(mapper.map_pointer(0.0, 0.0, viewport), (-1920, 0));
        assert_eq!(mapper.map_pointer(100.0, 80.0, viewport), (-1795, 100));
    }
}
