//! rdc-filetransfer / 分块计划与进度
//!
//! 层级：L3 交互层
//! 用途：把文件总长切成固定大小分块（默认 4 MiB，对齐 docs/10 §5），提供偏移/长度换算
//!       与进度结构；分块对象自带 SHA-256。
//! 调用方：rdc-filetransfer 的发送端、接收端与状态编解码

use crate::digest::hash_bytes;
use crate::error::FileTransferError;

/// 默认分块大小：4 MiB（docs/10 §5）。
pub const DEFAULT_CHUNK_SIZE: u64 = 4 << 20;

// =====** ChunkPlan =====**

/// 分块计划：总长 + 块大小，推导块数、偏移与每块长度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkPlan {
    /// 文件总字节数。
    total_bytes: u64,
    /// 每块字节数（最后一块可能更短）。
    chunk_size: u64,
}

impl ChunkPlan {
    /// 构造计划。
    ///
    /// # Arguments
    /// * `total_bytes` - 文件总长度（可为 0）
    /// * `chunk_size` - 每块字节数，必须大于 0
    ///
    /// # Returns
    /// 分块计划。
    ///
    /// # Errors
    /// `chunk_size` 为 0 时返回 [`FileTransferError::InvalidChunkSize`]。
    pub fn new(total_bytes: u64, chunk_size: u64) -> Result<Self, FileTransferError> {
        if chunk_size == 0 {
            return Err(FileTransferError::InvalidChunkSize);
        }
        Ok(Self {
            total_bytes,
            chunk_size,
        })
    }

    /// 文件总字节数。
    pub fn total_bytes(&self) -> u64 {
        self.total_bytes
    }

    /// 每块字节数。
    pub fn chunk_size(&self) -> u64 {
        self.chunk_size
    }

    /// 分块数；空文件为 0。
    pub fn chunk_count(&self) -> u32 {
        if self.total_bytes == 0 {
            0
        } else {
            ((self.total_bytes - 1) / self.chunk_size + 1) as u32
        }
    }

    /// 第 `index` 块的起始偏移。
    ///
    /// # Returns
    /// 合法下标返回偏移，越界返回 `None`。
    pub fn chunk_offset(&self, index: u32) -> Option<u64> {
        if index >= self.chunk_count() {
            return None;
        }
        Some(index as u64 * self.chunk_size)
    }

    /// 第 `index` 块的字节数（末块可能小于 `chunk_size`）。
    ///
    /// # Returns
    /// 合法下标返回长度，越界返回 `None`。
    pub fn chunk_len(&self, index: u32) -> Option<u64> {
        let offset = self.chunk_offset(index)?;
        Some((self.total_bytes - offset).min(self.chunk_size))
    }
}

// =====** Progress =====**

/// 传输进度快照（进度回调参数）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    /// 已完成分块数。
    pub chunks_done: u32,
    /// 总分块数。
    pub chunks_total: u32,
    /// 已完成字节数（按计划长度累加，非实际写入量）。
    pub bytes_done: u64,
    /// 文件总字节数。
    pub bytes_total: u64,
}

impl Progress {
    /// 完成比例 `0.0..=1.0`；总长为 0 时按 1.0（空文件视为已完成）。
    pub fn fraction(&self) -> f32 {
        if self.bytes_total == 0 {
            1.0
        } else {
            (self.bytes_done as f64 / self.bytes_total as f64) as f32
        }
    }
}

// =====** Chunk =====**

/// 一个文件分块：下标、偏移、内容与内容 SHA-256。
///
/// 发送端读盘后填充；接收端写入前必须调用 [`Chunk::verify`]，
/// 校验失败按 [`FileTransferError::ChunkHashMismatch`] 丢弃并要求重传。
pub struct Chunk {
    /// 分块下标（从 0 开始）。
    pub index: u32,
    /// 文件内偏移。
    pub offset: u64,
    /// 分块内容。
    pub data: Vec<u8>,
    /// `data` 的 SHA-256。
    pub hash: [u8; 32],
}

impl Chunk {
    /// 构造分块并计算哈希。
    ///
    /// # Arguments
    /// * `index` - 分块下标
    /// * `offset` - 文件内偏移
    /// * `data` - 分块内容
    ///
    /// # Returns
    /// 带哈希的分块。
    pub fn new(index: u32, offset: u64, data: Vec<u8>) -> Self {
        let hash = hash_bytes(&data);
        Self {
            index,
            offset,
            data,
            hash,
        }
    }

    /// 校验内容与自带哈希是否一致。
    ///
    /// # Returns
    /// 一致返回 `true`。
    pub fn verify(&self) -> bool {
        hash_bytes(&self.data) == self.hash
    }
}

// =====** 自检 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 空文件不产生分块。
    #[test]
    fn empty_file_has_no_chunks() {
        let plan = ChunkPlan::new(0, 10).expect("计划");
        assert_eq!(plan.chunk_count(), 0);
        assert_eq!(plan.chunk_offset(0), None);
        assert_eq!(plan.chunk_len(0), None);
    }

    /// 不足一块 → 1 块且长度等于总长。
    #[test]
    fn single_partial_chunk() {
        let plan = ChunkPlan::new(7, 10).expect("计划");
        assert_eq!(plan.chunk_count(), 1);
        assert_eq!(plan.chunk_offset(0), Some(0));
        assert_eq!(plan.chunk_len(0), Some(7));
        assert_eq!(plan.chunk_len(1), None);
    }

    /// 正好整数倍 → 不产生空尾块。
    #[test]
    fn exact_multiple_has_no_empty_tail() {
        let plan = ChunkPlan::new(30, 10).expect("计划");
        assert_eq!(plan.chunk_count(), 3);
        assert_eq!(plan.chunk_len(2), Some(10));
        assert_eq!(plan.chunk_len(3), None);
    }

    /// 末块长度正确，偏移连续。
    #[test]
    fn last_chunk_is_shorter() {
        let plan = ChunkPlan::new(31, 10).expect("计划");
        assert_eq!(plan.chunk_count(), 4);
        assert_eq!(plan.chunk_offset(3), Some(30));
        assert_eq!(plan.chunk_len(3), Some(1));
    }

    /// 块大小为 0 被拒绝。
    #[test]
    fn zero_chunk_size_is_rejected() {
        assert!(matches!(
            ChunkPlan::new(10, 0),
            Err(FileTransferError::InvalidChunkSize)
        ));
    }

    /// 进度比例与空文件特例。
    #[test]
    fn progress_fraction() {
        let p = Progress {
            chunks_done: 1,
            chunks_total: 4,
            bytes_done: 25,
            bytes_total: 100,
        };
        assert!((p.fraction() - 0.25).abs() < f32::EPSILON);
        let empty = Progress {
            chunks_done: 0,
            chunks_total: 0,
            bytes_done: 0,
            bytes_total: 0,
        };
        assert_eq!(empty.fraction(), 1.0);
    }

    /// 分块哈希可自校验，篡改后失败。
    #[test]
    fn chunk_verify_detects_tampering() {
        let mut chunk = Chunk::new(0, 0, b"hello".to_vec());
        assert!(chunk.verify());
        chunk.data[0] = b'x';
        assert!(!chunk.verify());
    }
}
