//! rdc-decode / 错误定义
//!
//! 层级：L3 编解码层
//! 用途：解码与呈现阶段的可分类错误，最终在 rdc-core 收敛为 `RdcError`。
//! 调用方：rdc-decode 内部各模块、rdc-core 的媒资任务、rdc-cli 的 S3b 命令

use thiserror::Error;

/// 解码/呈现错误。
#[derive(Debug, Error)]
pub enum DecodeError {
    /// 解码器不可用或初始化失败。
    #[error("解码器不可用: {0}")]
    DecoderUnavailable(String),
    /// 送包/取包过程中的失败。
    #[error("解码失败: {0}")]
    DecodeFailure(String),
    /// 呈现链（D3D11/窗口）初始化或运行时失败。
    #[error("呈现失败: {0}")]
    PresentFailure(String),
    /// 调用参数不合法。
    #[error("参数不合法: {0}")]
    InvalidArgument(String),
}
