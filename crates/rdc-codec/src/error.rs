//! rdc-codec / 编解码错误类型
//!
//! 层级：L3 编解码层
//! 用途：描述硬件能力缺失、配置非法与运行期编解码失败的终态
//! 调用方：rdc-codec 各平台后端、rdc-core（收敛为 RdcError）、rdc-bridge（错误码映射）

use thiserror::Error;

// =====** CodecError =====**

/// 编解码阶段错误。
///
/// 设计要点：能力类错误（`HardwareUnsupported` / `Unsupported`）可触发软编降级；
/// 运行期错误（`EncoderFailure` / `DecoderFailure`）由 rdc-core 决定重建会话或终止。
#[derive(Debug, Error)]
pub enum CodecError {
    /// 平台无可用硬件编码器（或驱动未就绪）。
    #[error("硬件编码器不可用")]
    HardwareUnsupported,
    /// 配置非法（分辨率/帧率/码率/位深/色度超出能力）。
    #[error("编解码配置非法: {0}")]
    InvalidConfig(String),
    /// 编码器运行期失败。
    #[error("编码器失败: {0}")]
    EncoderFailure(String),
    /// 解码器运行期失败。
    #[error("解码器失败: {0}")]
    DecoderFailure(String),
    /// 当前平台不支持编解码能力。
    #[error("编解码能力不受支持")]
    Unsupported,
}

// =====** 诊断码映射 =====**

impl CodecError {
    /// 映射到 `RDC-*` 诊断码，供桥接层与 doctor 使用（docs/16 §3）。
    ///
    /// # Returns
    /// 码表内的静态字符串；新增变体必须同步补充映射与单测。
    pub fn diag_code(&self) -> &'static str {
        match self {
            Self::HardwareUnsupported => "RDC-COD-001",
            Self::InvalidConfig(_) => "RDC-COD-002",
            Self::EncoderFailure(_) => "RDC-COD-003",
            Self::DecoderFailure(_) => "RDC-DEC-002",
            Self::Unsupported => "RDC-COD-005",
        }
    }
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_maps_to_known_diag_code() {
        let cases = [
            CodecError::HardwareUnsupported,
            CodecError::InvalidConfig("fps=0".to_string()),
            CodecError::EncoderFailure("mf".to_string()),
            CodecError::DecoderFailure("mf".to_string()),
            CodecError::Unsupported,
        ];
        for error in cases {
            let code = error.diag_code();
            assert_eq!(code.len(), 11, "unexpected code shape: {code}");
            assert!(rdc_diag::error::is_known(code), "unknown diag code: {code}");
        }
    }
}
