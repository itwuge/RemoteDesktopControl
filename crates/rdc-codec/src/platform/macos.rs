//! rdc-codec / macOS 平台编码器后端（S1 空壳）
//!
//! 层级：L3 编解码层 · 平台适配
//! 用途：macOS 硬件编码入口；stage3 落地 VideoToolbox
//! 调用方：rdc-codec::platform::create_encoder（由 rdc-ctl 在会话建立时调用）

use rdc_runtime::PooledBuffer;

use crate::encoder::{CodecKind, EncodedPacket, EncoderCaps, VideoConfig, VideoEncoder};
use crate::error::CodecError;

// =====** PlatformEncoder =====**

/// macOS 硬件编码器（S1 空壳，未创建任何硬件会话）。
///
/// TODO(stage3)：通过 VideoToolbox 的 `VTCompressionSession` 编码，输入为 CVPixelBuffer，
/// 低时延需关闭帧重排并设置 `kVTCompressionPropertyKey_RealTime`。
pub struct PlatformEncoder {
    /// 最近一次配置；S1 只记录，stage3 才用于创建硬件编码会话。
    config: Option<VideoConfig>,
}

impl PlatformEncoder {
    /// 构造未配置的 macOS 编码器。
    ///
    /// # Arguments
    /// * 无。
    ///
    /// # Returns
    /// 空壳实例；硬件会话在 [`VideoEncoder::configure`] 中创建。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new() -> Self {
        Self { config: None }
    }
}

// =====** VideoEncoder 实现 =====**

impl VideoEncoder for PlatformEncoder {
    /// 探测硬件能力。
    ///
    /// # Arguments
    /// * `_codecs` - 待探测格式（S1 未使用）。
    ///
    /// # Returns
    /// S1 固定为空列表；TODO(stage3) 用 `VTCopySupportedPropertyDictionaryForEncoder` 查询。
    ///
    /// # Errors
    /// 不会失败。
    fn probe(&self, _codecs: &[CodecKind]) -> Vec<EncoderCaps> {
        Vec::new()
    }

    /// 配置编码器。
    ///
    /// # Arguments
    /// * `config` - 编码配置（S1 仅保存）。
    ///
    /// # Returns
    /// S1 恒返回 `Ok(())`。
    ///
    /// # Errors
    /// S1 不会失败；stage3 在会话创建失败时返回 [`CodecError`]。
    fn configure(&mut self, config: VideoConfig) -> Result<(), CodecError> {
        // S1 只保存配置，真实 VTCompressionSession 创建推迟到 stage3。
        self.config = Some(config);
        Ok(())
    }

    /// 提交一帧到编码器。
    ///
    /// # Arguments
    /// * `_frame` - 池化帧（S1 未使用）。
    /// * `_pts_ns` - 帧时间戳（S1 未使用）。
    /// * `_force_keyframe` - 是否强制 IDR（S1 未使用）。
    ///
    /// # Returns
    /// stage3 之前不会正常返回。
    ///
    /// # Errors
    /// stage3 返回提交失败。
    fn push(
        &mut self,
        _frame: PooledBuffer,
        _pts_ns: u64,
        _force_keyframe: bool,
    ) -> Result<(), CodecError> {
        // TODO(stage3)：把帧包装为 CVPixelBuffer 并用 VTCompressionSessionEncodeFrame 提交。
        todo!("TODO(stage3)：macOS 硬件编码提交在 stage3 实现")
    }

    /// 非阻塞取出一包编码结果。
    ///
    /// # Arguments
    /// * 无。
    ///
    /// # Returns
    /// 未配置或无输出时返回 `None`。
    ///
    /// # Errors
    /// 不会失败。
    fn poll_packet(&mut self) -> Option<EncodedPacket> {
        // 未配置时不存在编码输出，直接返回 None。
        if self.config.is_none() {
            return None;
        }
        // TODO(stage3)：从输出回调队列取走 CMSampleBuffer。
        None
    }

    /// 运行期重配。
    ///
    /// # Arguments
    /// * `_config` - 新编码配置（S1 未使用）。
    ///
    /// # Returns
    /// stage3 之前不会正常返回。
    ///
    /// # Errors
    /// stage3 返回重配失败或能力不支持。
    fn reconfigure(&mut self, _config: VideoConfig) -> Result<(), CodecError> {
        // TODO(stage3)：通过 VTSessionSetProperty 下发码率/帧率等参数。
        todo!("TODO(stage3)：macOS 编码器重配在 stage3 实现")
    }

    /// 请求关键帧。
    ///
    /// # Arguments
    /// * 无。
    ///
    /// # Returns
    /// stage3 之前不会正常返回。
    ///
    /// # Errors
    /// stage3 之前不会正常返回。
    fn request_keyframe(&mut self) {
        // TODO(stage3)：置位强制 IDR 标志，由下一帧 push 兑现。
        todo!("TODO(stage3)：macOS 关键帧请求在 stage3 实现")
    }
}
