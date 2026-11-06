//! rdc-protocol / 编码器 trait 与配置（接口冻结）
//!
//! 层级：L2 能力层（只定义契约，不实现任何平台编码 API）
//! 用途：冻结编码器输入/输出类型、配置字段与能力查询接口，S3 起由 rdc-codec 实现；
//!       先冻结可避免 S3 实现时改动协议层（docs/06 §2、docs/02 §5）。
//! 调用方：rdc-codec（实现 trait）、rdc-core（选择编码器与重配）、rdc-bridge（参数页）

use crate::error::ProtocolError;

// =====** 编码格式 =====**

/// 视频编码格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodecKind {
    /// H.264/AVC（兼容性兜底）。
    H264,
    /// H.265/HEVC（默认优先，需能力位支持）。
    Hevc,
}

// =====** 原始帧 =====**

/// 采集得到的原始帧（RGBA8，紧凑排布）。
///
/// 为什么在协议层定义原始帧：编码器 trait 的签名必须稳定，而平台采集实现
/// 各自返回不同格式，统一成 RGBA8 才能让 trait 与平台解耦。
#[derive(Debug, Clone)]
pub struct RawFrame {
    /// 帧号（采集侧递增，u16 回绕）。
    pub frame_id: u16,
    /// 采集时间戳（微秒，单调时钟），后续不得改写。
    pub capture_ts_us: u64,
    /// 宽（像素）。
    pub width: u32,
    /// 高（像素）。
    pub height: u32,
    /// RGBA8 像素数据，长度必须为 `width * height * 4`。
    pub rgba: Vec<u8>,
}

impl RawFrame {
    /// 校验像素缓冲区长度是否与宽高匹配。
    ///
    /// # Returns
    /// 匹配返回 `Ok(())`。
    ///
    /// # Errors
    /// 不匹配返回 [`ProtocolError::InvalidField`]，编码器不得读取越界数据。
    pub fn validate(&self) -> Result<(), ProtocolError> {
        let expected = self.width as usize * self.height as usize * 4;
        if self.rgba.len() != expected {
            return Err(ProtocolError::InvalidField("RGBA 缓冲区长度与宽高不匹配"));
        }
        Ok(())
    }
}

// =====** 编码配置 =====**

/// 编码器配置（一次会话内可热更新）。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EncoderConfig {
    /// 目标编码格式。
    pub codec: CodecKind,
    /// 宽（像素）。
    pub width: u32,
    /// 高（像素）。
    pub height: u32,
    /// 目标帧率。
    pub fps: u32,
    /// 目标码率（bps）；0 表示由码控自行决定。
    pub target_bitrate_bps: u32,
    /// GOP 长度（帧）；intra-refresh 开启时表示刷新周期。
    pub gop: u32,
    /// 是否启用 intra-refresh（低延迟场景下避免关键帧尖峰）。
    pub intra_refresh: bool,
    /// 是否开启低延迟调优（禁用 B 帧、零延迟）。
    pub low_latency: bool,
    /// 是否使用 4:4:4 色度（屏幕内容优先）。
    pub chroma_444: bool,
    /// 位深（8 或 10）。
    pub bit_depth: u8,
    /// temporal 层数（1~3）。
    pub temporal_layers: u8,
}

impl Default for EncoderConfig {
    /// 默认配置：HEVC、1080p30、8 Mbps、低延迟、8-bit、单层。
    fn default() -> Self {
        Self {
            codec: CodecKind::Hevc,
            width: 1920,
            height: 1080,
            fps: 30,
            target_bitrate_bps: 8_000_000,
            gop: 60,
            intra_refresh: false,
            low_latency: true,
            chroma_444: false,
            bit_depth: 8,
            temporal_layers: 1,
        }
    }
}

impl EncoderConfig {
    /// 校验配置是否自洽。
    ///
    /// # Returns
    /// 合法返回 `Ok(())`。
    ///
    /// # Errors
    /// 宽高为 0、位深不是 8/10、temporal_layers 超出 1~3 时返回
    /// [`ProtocolError::InvalidField`]。
    pub fn validate(&self) -> Result<(), ProtocolError> {
        if self.width == 0 || self.height == 0 {
            return Err(ProtocolError::InvalidField("编码宽高不能为 0"));
        }
        if self.bit_depth != 8 && self.bit_depth != 10 {
            return Err(ProtocolError::InvalidField("位深只能是 8 或 10"));
        }
        if !(1..=3).contains(&self.temporal_layers) {
            return Err(ProtocolError::InvalidField("temporal 层数必须在 1~3"));
        }
        Ok(())
    }

    /// 判断本配置是否需要某个能力位。
    ///
    /// # Returns
    /// 需要的能力位掩码（0 表示无需特殊能力）。
    ///
    /// # Errors
    /// 不会失败。
    pub fn required_capabilities(&self) -> u64 {
        let mut mask = 0;
        if self.codec == CodecKind::Hevc {
            mask |= crate::capabilities::bits::HEVC;
        }
        if self.chroma_444 {
            mask |= crate::capabilities::bits::CHROMA_444;
        }
        if self.bit_depth == 10 {
            mask |= crate::capabilities::bits::BIT_DEPTH_10;
        }
        if self.intra_refresh {
            mask |= crate::capabilities::bits::INTRA_REFRESH;
        }
        if self.temporal_layers > 1 {
            mask |= crate::capabilities::bits::TEMPORAL;
        }
        mask
    }
}

// =====** 编码输出 =====**

/// 编码后的一帧。
#[derive(Debug, Clone)]
pub struct EncodedFrame {
    /// 帧号（与输入 RawFrame 一致）。
    pub frame_id: u16,
    /// 采集时间戳（微秒），原样透传，禁止编码器改写。
    pub capture_ts_us: u64,
    /// 是否关键帧 / 可解码起点。
    pub keyframe: bool,
    /// temporal 层号（0~3）。
    pub temporal_id: u8,
    /// 编码数据（Annex-B 或容器约定的裸码流）。
    pub data: Vec<u8>,
    /// 编码耗时（毫秒）。
    pub encode_ms: f32,
}

// =====** Encoder trait =====**

/// 视频编码器契约。
///
/// 线程模型：编码器由单一编码线程独占（`&mut self`），不得跨线程共享；
/// 实现内部可以持有 GPU 句柄，但必须在该线程内创建与释放。
///
/// 为什么用 `&mut self` + 无 async：编码是 CPU/GPU 密集的同步调用，放进 async
/// 任务会阻塞 runtime；由上层用专用线程 + 有界队列调度。
pub trait Encoder: Send {
    /// 查询实现名称（如 `hevc-nvenc`），用于日志与统计。
    ///
    /// # Returns
    /// 静态字符串。
    fn name(&self) -> &'static str;

    /// 查询实现支持的能力位。
    ///
    /// # Returns
    /// 能力位掩码，与 [`Capabilities`] 的 [`crate::capabilities::bits`] 对齐。
    fn capabilities(&self) -> u64;

    /// 应用配置（首次调用等同初始化，后续调用为重配）。
    ///
    /// # Arguments
    /// * `config` - 目标配置；实现必须先 `validate()`
    ///
    /// # Returns
    /// 配置成功返回 `Ok(())`。
    ///
    /// # Errors
    /// 配置非法或硬件不支持时返回 [`ProtocolError::CodecUnsupported`] /
    /// [`ProtocolError::InvalidField`]。
    fn configure(&mut self, config: &EncoderConfig) -> Result<(), ProtocolError>;

    /// 编码一帧。
    ///
    /// # Arguments
    /// * `frame` - RGBA8 原始帧
    ///
    /// # Returns
    /// 编码结果（含耗时）。
    ///
    /// # Errors
    /// 未配置、像素格式不符、编码失败时返回 [`ProtocolError::CodecUnsupported`]
    /// 或 [`ProtocolError::State`]。
    fn encode(&mut self, frame: &RawFrame) -> Result<EncodedFrame, ProtocolError>;

    /// 请求下一帧编码为关键帧。
    ///
    /// # Returns
    /// 不会失败；实现应设置内部标志，在下一次 `encode` 生效。
    fn request_keyframe(&mut self);

    /// 冲刷编码器内部缓存的帧（重配或结束时调用）。
    ///
    /// # Returns
    /// 缓存中剩余帧的列表（可能为空）。
    ///
    /// # Errors
    /// 内部状态异常返回 [`ProtocolError::State`]。
    fn flush(&mut self) -> Result<Vec<EncodedFrame>, ProtocolError>;
}

// =====** 单元测试：冻结配置合法性 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 默认配置必须合法，且只要求 HEVC 能力位。
    #[test]
    fn default_config_is_valid() {
        let cfg = EncoderConfig::default();
        cfg.validate().expect("默认配置合法");
        assert_eq!(cfg.required_capabilities(), crate::capabilities::bits::HEVC);
    }

    /// 非法位深/层数必须被拒绝。
    #[test]
    fn invalid_config_is_rejected() {
        let cfg = EncoderConfig {
            bit_depth: 12,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
        let cfg = EncoderConfig {
            temporal_layers: 4,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
        let cfg = EncoderConfig {
            width: 0,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    /// 原始帧像素长度校验必须防止越界读取。
    #[test]
    fn raw_frame_length_is_validated() {
        let frame = RawFrame {
            frame_id: 1,
            capture_ts_us: 1,
            width: 2,
            height: 2,
            rgba: vec![0; 15],
        };
        assert!(frame.validate().is_err());
        let frame = RawFrame {
            rgba: vec![0; 16],
            ..frame
        };
        assert!(frame.validate().is_ok());
    }

    /// 10-bit + 444 必须同时要求两个能力位。
    #[test]
    fn required_capabilities_accumulate() {
        let cfg = EncoderConfig {
            bit_depth: 10,
            chroma_444: true,
            temporal_layers: 2,
            ..EncoderConfig::default()
        };
        let mask = cfg.required_capabilities();
        assert!(mask & crate::capabilities::bits::BIT_DEPTH_10 != 0);
        assert!(mask & crate::capabilities::bits::CHROMA_444 != 0);
        assert!(mask & crate::capabilities::bits::TEMPORAL != 0);
        assert!(mask & crate::capabilities::bits::HEVC != 0);
    }
}
