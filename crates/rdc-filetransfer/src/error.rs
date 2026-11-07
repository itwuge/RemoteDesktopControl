//! rdc-filetransfer / 文件传输错误类型
//!
//! 层级：L3 交互层
//! 用途：描述分块计划、块校验、缺块与整文件校验的失败原因
//! 调用方：rdc-filetransfer 各模块、rdc-core（收敛为 RdcError）、rdc-bridge（错误码映射）

use thiserror::Error;

// =====** FileTransferError =====**

/// 文件传输错误。
///
/// 设计要点：`ChunkHashMismatch` 可重传该块（调用方按 docs/10 最多 3 次）；
/// `InvalidChunk*` 属协议错误，直接中止本次传输；`Io` 保留系统错误原文。
#[derive(Debug, Error)]
pub enum FileTransferError {
    /// 磁盘读写失败。
    #[error("文件读写失败：{0}")]
    Io(#[from] std::io::Error),
    /// 分块大小必须为正。
    #[error("分块大小必须大于 0")]
    InvalidChunkSize,
    /// 分块下标越界。
    #[error("分块下标越界：{index} >= {count}")]
    ChunkIndexOutOfRange {
        /// 请求的下标。
        index: u32,
        /// 总分块数。
        count: u32,
    },
    /// 分块长度或偏移与计划不符。
    #[error("分块 {index} 长度/偏移与计划不符：期望 offset={expected_offset} len={expected_len}")]
    InvalidChunkLayout {
        /// 分块下标。
        index: u32,
        /// 计划偏移。
        expected_offset: u64,
        /// 计划长度。
        expected_len: u64,
    },
    /// 分块内容与自带哈希不符（传输损坏或对端错误）。
    #[error("分块 {0} 哈希不匹配")]
    ChunkHashMismatch(u32),
    /// 仍有分块未收到。
    #[error("仍有 {0} 个分块未收到")]
    MissingChunks(u32),
    /// 整文件哈希与发送端声明不一致。
    #[error("整文件哈希不匹配")]
    FileHashMismatch,
    /// 续传状态长度与分块数不符。
    #[error("续传状态长度与分块数不符")]
    MalformedState,
}
