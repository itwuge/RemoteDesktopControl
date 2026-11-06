//! rdc-codec / Windows 平台编码器后端（S1 空壳）
//!
//! 层级：L3 编解码层 · 平台适配
//! 用途：Windows 硬件编码入口；stage3 落地 NVENC / AMF / QSV 三种后端
//! 调用方：rdc-codec::platform::create_encoder（由 rdc-ctl 在会话建立时调用）

use rdc_runtime::PooledBuffer;

use crate::encoder::{CodecKind, EncodedPacket, EncoderCaps, VideoConfig, VideoEncoder};
use crate::error::CodecError;

// =====** PlatformEncoder =====**

/// Windows 硬件编码器（S1 空壳，未创建任何硬件会话）。
///
/// TODO(stage3)：按显卡厂商选择后端——NVIDIA 走 NVENC，AMD 走 AMF，Intel 走 QSV；
/// 三者都需 D3D11 纹理输入，避免 CPU 拷贝。
pub struct PlatformEncoder {
    /// 最近一次配置；S1 只记录，stage3 才用于创建硬件编码会话。
    config: Option<VideoConfig>,
}

impl PlatformEncoder {
    /// 构造未配置的 Windows 编码器。
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
    /// S1 固定为空列表；TODO(stage3) 查询 NVENC/AMF/QSV 的分辨率、4:4:4 与 10bit 能力。
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
    /// S1 不会失败；stage3 在硬件会话创建失败时返回 [`CodecError`]。
    fn configure(&mut self, config: VideoConfig) -> Result<(), CodecError> {
        // S1 只保存配置，真实硬件会话创建推迟到 stage3。
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
        // TODO(stage3)：把帧上传为 D3D11 纹理并提交 NVENC/AMF/QSV。
        todo!("TODO(stage3)：Windows 硬件编码提交在 stage3 实现")
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
        // 未配置时不存在编码输出，直接返回 None，避免读取未初始化的会话状态。
        if self.config.is_none() {
            return None;
        }
        // TODO(stage3)：轮询硬件编码器输出队列。
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
        // TODO(stage3)：仅在 supports_runtime_reconfigure 为真时下发新参数。
        todo!("TODO(stage3)：Windows 编码器重配在 stage3 实现")
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
        todo!("TODO(stage3)：Windows 关键帧请求在 stage3 实现")
    }
}
