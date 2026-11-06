//! rdc-capture / 帧类型
//!
//! 层级：L3 采集层
//! 用途：定义采集输出的帧与其时间戳元数据；缓冲来自 rdc-runtime 内存池
//! 调用方：rdc-capture 平台后端（构造帧）、FrameSink 实现（消费帧）

use rdc_runtime::PooledBuffer;

// =====** FrameMeta =====**

/// 帧时间戳与归属信息。
#[derive(Debug, Clone, Copy)]
pub struct FrameMeta {
    /// 采集时钟时间戳，单位：纳秒。
    pub pts_ns: u64,
    /// 自会话开始单调递增的帧号，用于丢帧统计与因果校验。
    pub frame_id: u64,
    /// 来源显示器编号。
    pub display_id: u32,
}

// =====** Frame =====**

/// 一帧采集结果：池化像素缓冲 + 元数据。
pub struct Frame {
    /// 帧像素数据；来自 [`rdc_runtime::FramePool`]，drop 时自动归还。
    pub buffer: PooledBuffer,
    /// 帧时间戳与归属信息。
    pub meta: FrameMeta,
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
}
