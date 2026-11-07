//! rdc-capture / 帧类型
//!
//! 层级：L3 采集层
//! 用途：定义采集输出的统一帧结构（像素格式、宽高、QPC 时间戳、脏区/移动矩形）与其元数据；
//!       缓冲来自 rdc-runtime 内存池
//! 调用方：rdc-capture 平台后端（构造帧）、FrameSink 实现（消费帧）、rdc-codec（转 NV12）

use rdc_runtime::PooledBuffer;

// =====** PixelFormat =====**

/// 采集输出的像素格式。
///
/// [`PixelFormat::Bgra8`] 是 DXGI Desktop Duplication 与 GDI 共同的原生格式，
/// 避免采集侧做二次转换。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    /// 8-bit BGRA（Windows 桌面复制原生格式）。
    Bgra8,
    /// 8-bit RGBA（软件测试源或跨平台后端）。
    Rgba8,
    /// NV12（硬件编码器首选；平台后端做色彩转换时使用）。
    Nv12,
}

impl PixelFormat {
    /// 单像素字节数（NV12 按 Y 平面计，调用方需用 [`PixelFormat::buffer_size`]）。
    ///
    /// # Returns
    /// BGRA/RGBA 返回 4，NV12 返回 1。
    ///
    /// # Errors
    /// 不会失败。
    pub fn bytes_per_pixel(self) -> usize {
        match self {
            Self::Bgra8 | Self::Rgba8 => 4,
            Self::Nv12 => 1,
        }
    }

    /// 该格式下宽高对应的最小缓冲字节数。
    ///
    /// # Arguments
    /// * `width` - 像素宽
    /// * `height` - 像素高
    ///
    /// # Returns
    /// 所需字节数（NV12 为 `w*h*3/2`，其余为 `w*h*bpp`）。
    ///
    /// # Errors
    /// 不会失败。
    pub fn buffer_size(self, width: u32, height: u32) -> usize {
        let px = width as usize * height as usize;
        match self {
            Self::Bgra8 | Self::Rgba8 => px * 4,
            Self::Nv12 => px * 3 / 2,
        }
    }
}

// =====** 矩形 =====**

/// 帧内矩形（像素坐标，左上为原点）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FrameRect {
    /// 左边界。
    pub left: u32,
    /// 上边界。
    pub top: u32,
    /// 宽。
    pub width: u32,
    /// 高。
    pub height: u32,
}

impl FrameRect {
    /// 构造矩形。
    ///
    /// # Arguments
    /// * `left` / `top` / `width` / `height` - 像素坐标与尺寸。
    ///
    /// # Returns
    /// 矩形值。
    ///
    /// # Errors
    /// 不会失败。
    pub const fn new(left: u32, top: u32, width: u32, height: u32) -> Self {
        Self {
            left,
            top,
            width,
            height,
        }
    }

    /// 面积（像素数）。
    ///
    /// # Returns
    /// `width * height`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn area(self) -> u64 {
        u64::from(self.width) * u64::from(self.height)
    }

    /// 裁剪到帧边界内，越界部分丢弃。
    ///
    /// # Arguments
    /// * `frame_w` - 帧宽
    /// * `frame_h` - 帧高
    ///
    /// # Returns
    /// 裁剪后的矩形；完全越界时返回 `None`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn clamp_to(self, frame_w: u32, frame_h: u32) -> Option<Self> {
        // 先处理完全越界：起点在帧外即无交集，直接返回 None。
        if self.left >= frame_w || self.top >= frame_h {
            return None;
        }
        let width = self.width.min(frame_w - self.left);
        let height = self.height.min(frame_h - self.top);
        if width == 0 || height == 0 {
            return None;
        }
        Some(Self {
            left: self.left,
            top: self.top,
            width,
            height,
        })
    }
}

/// 帧内移动矩形：把 `src` 区域的像素搬到 `(dst_left, dst_top)`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MoveRect {
    /// 源矩形。
    pub src: FrameRect,
    /// 目标左上角 X。
    pub dst_left: u32,
    /// 目标左上角 Y。
    pub dst_top: u32,
}

// =====** FrameMeta =====**

/// 帧时间戳与归属信息。
///
/// [`FrameMeta::qpc_us`] 是采集瞬间的 QPC 读数，作为整条时延链的起点；
/// [`FrameMeta::pts_ns`] 是其纳秒表示。
#[derive(Debug, Clone, Copy)]
pub struct FrameMeta {
    /// 采集时钟时间戳，单位：纳秒（由 QPC 换算，单调递增）。
    pub pts_ns: u64,
    /// QPC 原始读数换算出的微秒时间戳（同源，便于与 CSV 对齐）。
    pub qpc_us: u64,
    /// 自会话开始单调递增的帧号，用于丢帧统计与因果校验。
    pub frame_id: u64,
    /// 来源显示器编号。
    pub display_id: u32,
}

// =====** GpuFrame =====**

/// GPU 帧租约：具体实现（平台后端）在 `Drop` 时把纹理归还池。
///
/// 为什么用 trait 而不是回调：租约对象随 `Frame` 跨线程移动，
/// 只有 `Drop` 语义能保证「无论消费方正常处理还是提前丢弃都归还」。
pub trait GpuLease: Send {}

/// GPU 帧描述（Windows 下为 `ID3D11Texture2D`）。
///
/// 生命周期与线程约束：
/// - `texture` 是裸句柄的 `usize` 形式，只在创建它的 `ID3D11Device` 上有效；
///   消费方必须确认自己用的设备与采集设备是同一个（同适配器）。
/// - 纹理由 [`GpuLease`] 租约保护：消费方 drop 后采集侧才允许复写。
///   编码器可能异步持有纹理直到对应码流包出队，消费方需要延迟释放（见桥接层）。
/// - 本结构 `Send`，但纹理操作（`CopySubresourceRegion`/编码器输入）必须在
///   设备所属的、受 `ID3D10Multithread` 保护的线程上串行执行。
pub struct GpuFrame {
    /// `ID3D11Texture2D*`（以 `usize` 传递，避免平台类型泄漏到跨平台抽象层）。
    pub texture: usize,
    /// 纹理数组层号；单层纹理为 0。
    pub array_slice: u32,
    /// 纹理宽（像素）。
    pub width: u32,
    /// 纹理高（像素）。
    pub height: u32,
    /// 纹理像素格式。
    pub format: PixelFormat,
    /// 租约；drop 时归还纹理池。
    _lease: Option<Box<dyn GpuLease>>,
}

impl GpuFrame {
    /// 构造 GPU 帧。
    ///
    /// # Arguments
    /// * `texture` - 纹理裸句柄（`usize`）。
    /// * `array_slice` - 数组层号。
    /// * `width` / `height` / `format` - 纹理几何与格式。
    /// * `lease` - 租约对象。
    ///
    /// # Returns
    /// GPU 帧描述。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new(
        texture: usize,
        array_slice: u32,
        width: u32,
        height: u32,
        format: PixelFormat,
        lease: Box<dyn GpuLease>,
    ) -> Self {
        Self {
            texture,
            array_slice,
            width,
            height,
            format,
            _lease: Some(lease),
        }
    }
}

// =====** Frame =====**

/// 一帧采集结果：池化像素缓冲 + 统一元数据。
pub struct Frame {
    /// 帧像素数据；来自 [`rdc_runtime::FramePool`]，drop 时自动归还。
    ///
    /// GPU 直出路径下该缓冲仍占用一个池块以维持类型契约，但不写入像素
    ///（纹理内容才是真实帧），因此不产生 CPU 拷贝。
    pub buffer: PooledBuffer,
    /// 帧时间戳与归属信息。
    pub meta: FrameMeta,
    /// 像素格式。
    pub format: PixelFormat,
    /// 宽（像素）。
    pub width: u32,
    /// 高（像素）。
    pub height: u32,
    /// 脏区矩形；平台后端不提供时为空（表示整帧都可能变化）。
    pub dirty_rects: Vec<FrameRect>,
    /// 移动矩形；平台后端不提供时为空。
    pub move_rects: Vec<MoveRect>,
    /// 采集耗时（微秒）：从帧到手到 GPU 拷贝提交/像素拷贝完成。
    pub capture_cost_us: u32,
    /// 距上一帧的间隔（微秒）；首帧为 0。
    pub interval_us: u32,
    /// GPU 纹理帧；`None` 表示像素在 [`Frame::buffer`] 中。
    pub gpu: Option<GpuFrame>,
}

impl Frame {
    /// 读取帧像素的只读视图。
    ///
    /// # Arguments
    /// * 无（使用 `self.buffer`）。
    ///
    /// # Returns
    /// 与内存池块等长的字节切片。
    ///
    /// # Errors
    /// 不会失败。
    pub fn as_slice(&self) -> &[u8] {
        self.buffer.as_slice()
    }

    /// 有效像素区域是否覆盖整帧（无脏区信息时按整帧处理）。
    ///
    /// # Returns
    /// 脏区为空或脏区总面积超过帧面积一半时返回 `true`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn is_full_frame(&self) -> bool {
        if self.dirty_rects.is_empty() {
            return true;
        }
        let total: u64 = self.dirty_rects.iter().map(|r| r.area()).sum();
        total >= u64::from(self.width) * u64::from(self.height) / 2
    }
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 像素格式的缓冲大小必须与宽高匹配，NV12 是 1.5 倍。
    #[test]
    fn buffer_size_matches_format() {
        assert_eq!(PixelFormat::Bgra8.buffer_size(1920, 1080), 1920 * 1080 * 4);
        assert_eq!(
            PixelFormat::Nv12.buffer_size(1920, 1080),
            1920 * 1080 * 3 / 2
        );
        assert_eq!(PixelFormat::Bgra8.bytes_per_pixel(), 4);
    }

    /// 越界矩形必须被裁剪，完全越界返回 None。
    #[test]
    fn rect_clamp_handles_out_of_bounds() {
        let r = FrameRect::new(10, 10, 100, 100);
        assert_eq!(r.clamp_to(1920, 1080), Some(r));
        assert_eq!(FrameRect::new(1915, 0, 100, 10).clamp_to(1920, 1080), Some(FrameRect::new(1915, 0, 5, 10)));
        assert_eq!(FrameRect::new(2000, 0, 10, 10).clamp_to(1920, 1080), None);
        assert_eq!(FrameRect::new(0, 0, 0, 10).clamp_to(1920, 1080), None);
    }

    /// 无脏区信息时按整帧处理，脏区覆盖大半时同样按整帧处理。
    #[test]
    fn full_frame_detection() {
        let pool = rdc_runtime::FramePool::new(1, 16);
        let mut frame = Frame {
            buffer: pool.acquire().expect("可借"),
            meta: FrameMeta {
                pts_ns: 0,
                qpc_us: 0,
                frame_id: 0,
                display_id: 0,
            },
            format: PixelFormat::Bgra8,
            width: 4,
            height: 4,
            dirty_rects: Vec::new(),
            move_rects: Vec::new(),
            capture_cost_us: 0,
            interval_us: 0,
            gpu: None,
        };
        assert!(frame.is_full_frame());
        frame.dirty_rects.push(FrameRect::new(0, 0, 4, 4));
        assert!(frame.is_full_frame());
        frame.dirty_rects.clear();
        frame.dirty_rects.push(FrameRect::new(0, 0, 1, 1));
        assert!(!frame.is_full_frame());
    }
}
