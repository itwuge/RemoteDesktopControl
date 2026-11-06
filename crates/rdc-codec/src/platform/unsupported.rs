//! rdc-codec / 非桌面平台编码器桩
//!
//! 层级：L3 编解码层 · 平台适配
//! 用途：为移动端与其它目标提供可编译的编码器，配置与提交一律返回 Unsupported
//! 调用方：rdc-codec::platform::create_encoder（非桌面目标）

use rdc_runtime::PooledBuffer;

use crate::encoder::{CodecKind, EncodedPacket, EncoderCaps, VideoConfig, VideoEncoder};
use crate::error::CodecError;

// =====** PlatformEncoder =====**

/// 非桌面平台的编码器桩：无状态、无硬件调用。
pub struct PlatformEncoder;

impl PlatformEncoder {
    /// 构造桩实例。
    ///
    /// # Arguments
    /// * 无。
    ///
    /// # Returns
    /// 无状态桩实例。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new() -> Self {
        Self
    }
}

// =====** VideoEncoder 实现 =====**

impl VideoEncoder for PlatformEncoder {
    /// 探测硬件能力。
    ///
    /// # Arguments
    /// * `_codecs` - 待探测格式（被忽略）。
    ///
    /// # Returns
    /// 空列表（该平台无编码能力）。
    ///
    /// # Errors
    /// 不会失败。
    fn probe(&self, _codecs: &[CodecKind]) -> Vec<EncoderCaps> {
        // 平台没有后端，不存在可上报的能力。
        Vec::new()
    }

    /// 配置编码器（永远拒绝）。
    ///
    /// # Arguments
    /// * `_config` - 编码配置（被忽略）。
    ///
    /// # Returns
    /// 不会返回 `Ok`。
    ///
    /// # Errors
    /// 恒返回 [`CodecError::Unsupported`]。
    fn configure(&mut self, _config: VideoConfig) -> Result<(), CodecError> {
        Err(CodecError::Unsupported)
    }

    /// 提交一帧（永远拒绝）。
    ///
    /// # Arguments
    /// * `_frame` - 池化帧（被忽略）。
    /// * `_pts_ns` - 帧时间戳（被忽略）。
    /// * `_force_keyframe` - 是否强制 IDR（被忽略）。
    ///
    /// # Returns
    /// 不会返回 `Ok`。
    ///
    /// # Errors
    /// 恒返回 [`CodecError::Unsupported`]。
    fn push(
        &mut self,
        _frame: PooledBuffer,
        _pts_ns: u64,
        _force_keyframe: bool,
    ) -> Result<(), CodecError> {
        Err(CodecError::Unsupported)
    }

    /// 非阻塞取出编码结果。
    ///
    /// # Arguments
    /// * 无。
    ///
    /// # Returns
    /// 恒返回 `None`。
    ///
    /// # Errors
    /// 不会失败。
    fn poll_packet(&mut self) -> Option<EncodedPacket> {
        None
    }

    /// 运行期重配（永远拒绝）。
    ///
    /// # Arguments
    /// * `_config` - 新编码配置（被忽略）。
    ///
    /// # Returns
    /// 不会返回 `Ok`。
    ///
    /// # Errors
    /// 恒返回 [`CodecError::Unsupported`]。
    fn reconfigure(&mut self, _config: VideoConfig) -> Result<(), CodecError> {
        Err(CodecError::Unsupported)
    }

    /// 请求关键帧（空操作）。
    ///
    /// # Arguments
    /// * 无。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    fn request_keyframe(&mut self) {
        // 无编码器可置位，保留空操作以维持契约。
    }
}
