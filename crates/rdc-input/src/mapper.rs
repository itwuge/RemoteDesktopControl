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
