//! rdc-codec / 解码器契约类型
//!
//! 层级：L3 编解码层
//! 用途：定义平台无关的解码器 trait，供 rdc-core 驱动接收侧解码
//! 调用方：rdc-codec 平台后端实现 [`VideoDecoder`]；rdc-core（解码线程）

use crate::encoder::{EncodedPacket, VideoConfig};
use crate::error::CodecError;

// =====** VideoDecoder =====**

/// 平台无关的视频解码器。
///
/// 生命周期：`configure` →（`push` / `poll_frame` 循环）→ `flush`。
/// 实现必须是 `Send`：会话建立后由 rdc-ctl 移交给解码线程独占使用。
pub trait VideoDecoder: Send {
    /// 配置解码器。
    ///
    /// 调用线程：rdc-ctl（会话建立或重配时）。
    /// 阻塞性：阻塞（解码会话创建可能耗时），禁止在解码线程启动前以外的时机争用。
    /// 内部队列：无——重配会丢弃旧会话的在途帧。
    ///
    /// # Arguments
    /// * `config` - 编码格式、分辨率、色度与位深等参数。
    ///
    /// # Returns
    /// 配置成功返回 `Ok(())`。
    ///
    /// # Errors
    /// 配置非法或平台不支持该格式时返回 [`CodecError`]。
    fn configure(&mut self, config: VideoConfig) -> Result<(), CodecError>;

    /// 提交一包码流到解码器。
    ///
    /// 调用线程：解码线程。
    /// 阻塞性：非阻塞；输入队列满时立即丢包并等待下一个关键帧。
    /// 内部队列：解码器私有提交队列（此处仅入队）。
    ///
    /// # Arguments
    /// * `packet` - 待解码的编码包。
    ///
    /// # Returns
    /// 入队成功返回 `Ok(())`。
    ///
    /// # Errors
    /// 解码器未配置或码流损坏返回 [`CodecError`]。
    fn push(&mut self, packet: &EncodedPacket) -> Result<(), CodecError>;

    /// 非阻塞取出一帧解码结果。
    ///
    /// 调用线程：解码线程。
    /// 阻塞性：不阻塞。
    /// 内部队列：读取解码器输出队列（FIFO）。
    ///
    /// # Arguments
    /// * 无。
    ///
    /// # Returns
    /// 有输出返回像素字节（格式与 [`VideoConfig`] 一致）；暂无输出返回 `None`。
    ///
    /// # Errors
    /// 不会失败；解码失败通过后续 `push` 返回或统计计数上报。
    fn poll_frame(&mut self) -> Option<Vec<u8>>;

    /// 冲刷解码器，输出剩余帧并清空内部状态。
    ///
    /// 调用线程：rdc-ctl（会话结束或重配前）。
    /// 阻塞性：短阻塞。
    /// 内部队列：清空在途码流，不做重排。
    ///
    /// # Arguments
    /// * 无。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    fn flush(&mut self);
}
