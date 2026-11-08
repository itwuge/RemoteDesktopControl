//! rdc-filetransfer / 接收端状态机
//!
//! 层级：L3 交互层
//! 用途：按分块计划把收到的分块写入本地文件（先校验后落盘），记录已收位图，
//!       收齐后做整文件 SHA-256 比对；位图可编解码用于重连续传。
//! 调用方：rdc-core 的文件接收任务（收到分块 → accept → 收齐后 finish）
//!
//! 断点续传：重连时用 [`decode_state`](crate::decode_state) 解出对端已收位图，
//! 交给 [`FileReceiver::resume`] 重建接收端，已收块再次到达按 `Duplicate` 忽略。

use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use crate::digest::hash_file;
use crate::error::FileTransferError;
use crate::plan::{Chunk, ChunkPlan, Progress};

// =====** AcceptOutcome =====**

/// 分块接收结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcceptOutcome {
    /// 首次接收成功，附带最新进度。
    Accepted(Progress),
    /// 该块此前已收（重传/重复包），未重复写入、未重复计数。
    Duplicate,
}

// =====** FileReceiver =====**

/// 文件接收端。
pub struct FileReceiver {
    /// 落盘路径。
    path: PathBuf,
    /// 已打开的目标文件。
    file: File,
    /// 分块计划。
    plan: ChunkPlan,
    /// 已收分块位图。
    received: Vec<bool>,
    /// 已收块数。
    done: u32,
    /// 已收字节数（按计划长度累加）。
    done_bytes: u64,
}

impl FileReceiver {
    /// 新建接收端（截断/覆盖目标文件，按总长预分配）。
    ///
    /// # Arguments
    /// * `path` - 落盘路径
    /// * `total_bytes` - 发送端声明（`FileSender::plan().total_bytes()`）
    /// * `chunk_size` - 分块大小，必须与发送端一致
    ///
    /// # Returns
    /// 未收任何分块的接收端。
    ///
    /// # Errors
    /// 创建文件失败返回 [`FileTransferError::Io`]；块大小为 0 返回
    /// [`FileTransferError::InvalidChunkSize`]。
    pub fn create(
        path: impl AsRef<Path>,
        total_bytes: u64,
        chunk_size: u64,
    ) -> Result<Self, FileTransferError> {
        let plan = ChunkPlan::new(total_bytes, chunk_size)?;
        let path = path.as_ref().to_path_buf();
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)?;
        file.set_len(total_bytes)?;
        Ok(Self {
            path,
            file,
            plan,
            received: vec![false; plan.chunk_count() as usize],
            done: 0,
            done_bytes: 0,
        })
    }

    /// 按已收位图恢复接收端（重连续传）。
    ///
    /// # Arguments
    /// * `path` - 此前落盘的路径（不存在则报 `Io`）
    /// * `total_bytes` - 文件总长
    /// * `chunk_size` - 分块大小
    /// * `received` - 已收位图（长度必须等于分块数）
    ///
    /// # Returns
    /// 已知部分进度的接收端，后续只接受缺失块。
    ///
    /// # Errors
    /// 打开失败返回 [`FileTransferError::Io`]；位图长度不符返回
    /// [`FileTransferError::MalformedState`]。
    pub fn resume(
        path: impl AsRef<Path>,
        total_bytes: u64,
        chunk_size: u64,
        received: &[bool],
    ) -> Result<Self, FileTransferError> {
        let plan = ChunkPlan::new(total_bytes, chunk_size)?;
        let count = plan.chunk_count();
        if received.len() != count as usize {
            return Err(FileTransferError::MalformedState);
        }
        let path = path.as_ref().to_path_buf();
        let file = OpenOptions::new().write(true).open(&path)?;
        file.set_len(total_bytes)?;
        let mut done = 0;
        let mut done_bytes = 0;
        for index in 0..count {
            if received[index as usize] {
                done += 1;
                done_bytes += plan.chunk_len(index).unwrap_or(0);
            }
        }
        Ok(Self {
            path,
            file,
            plan,
            received: received.to_vec(),
            done,
            done_bytes,
        })
    }

    /// 分块计划。
    pub fn plan(&self) -> &ChunkPlan {
        &self.plan
    }

    /// 落盘路径。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 已收位图（可直接交给 [`encode_state`](crate::encode_state) 续传）。
    pub fn received(&self) -> &[bool] {
        &self.received
    }

    /// 当前进度快照。
    pub fn progress(&self) -> Progress {
        Progress {
            chunks_done: self.done,
            chunks_total: self.plan.chunk_count(),
            bytes_done: self.done_bytes,
            bytes_total: self.plan.total_bytes(),
        }
    }

    /// 接收一个分块：校验布局 → 校验块哈希 → 已收则返回 `Duplicate` → 写入并计数。
    ///
    /// # Arguments
    /// * `chunk` - 收到的分块（发送端 [`Chunk`] 或按线格式重建的等价结构）
    ///
    /// # Returns
    /// [`AcceptOutcome::Accepted`]（首次接收）或 [`AcceptOutcome::Duplicate`]。
    ///
    /// # Errors
    /// 下标越界返回 [`FileTransferError::ChunkIndexOutOfRange`]；偏移/长度与计划不符
    /// 返回 [`FileTransferError::InvalidChunkLayout`]；内容哈希不符返回
    /// [`FileTransferError::ChunkHashMismatch`]（该块不计入已收，调用方可重传）。
    pub fn accept(&mut self, chunk: &Chunk) -> Result<AcceptOutcome, FileTransferError> {
        let count = self.plan.chunk_count();
        if chunk.index >= count {
            return Err(FileTransferError::ChunkIndexOutOfRange {
                index: chunk.index,
                count,
            });
        }
        let expected_offset = self.plan.chunk_offset(chunk.index).unwrap_or(0);
        let expected_len = self.plan.chunk_len(chunk.index).unwrap_or(0);
        if chunk.offset != expected_offset || chunk.data.len() as u64 != expected_len {
            return Err(FileTransferError::InvalidChunkLayout {
                index: chunk.index,
                expected_offset,
                expected_len,
            });
        }
        if !chunk.verify() {
            return Err(FileTransferError::ChunkHashMismatch(chunk.index));
        }
        if self.received[chunk.index as usize] {
            return Ok(AcceptOutcome::Duplicate);
        }
        self.file.seek(SeekFrom::Start(expected_offset))?;
        self.file.write_all(&chunk.data)?;
        self.received[chunk.index as usize] = true;
        self.done += 1;
        self.done_bytes += expected_len;
        Ok(AcceptOutcome::Accepted(self.progress()))
    }

    /// 收尾：确认无缺块，落盘并对整文件做 SHA-256 比对。
    ///
    /// # Arguments
    /// * `expected` - 发送端声明的整文件 SHA-256；`None` 表示只计算不比对
    ///
    /// # Returns
    /// 本地文件的 SHA-256（可回传给发送端做双向确认）。
    ///
    /// # Errors
    /// 有缺块返回 [`FileTransferError::MissingChunks`]；哈希与 `expected` 不符返回
    /// [`FileTransferError::FileHashMismatch`]；落盘/读取失败返回 `Io`。
    pub fn finish(&mut self, expected: Option<[u8; 32]>) -> Result<[u8; 32], FileTransferError> {
        let missing = self.received.iter().filter(|hit| !**hit).count() as u32;
        if missing > 0 {
            return Err(FileTransferError::MissingChunks(missing));
        }
        self.file.flush()?;
        self.file.sync_all()?;
        let actual = hash_file(&self.path)?;
        if let Some(wanted) = expected {
            if wanted != actual {
                return Err(FileTransferError::FileHashMismatch);
            }
        }
        Ok(actual)
    }
}

// =====** 自检 =====**

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sender::FileSender;

    /// 空文件：没有分块，finish 直接成功。
    #[test]
    fn empty_file_finishes_immediately() {
        let dir = crate::tests_support::temp_path("recv-empty");
        let path = dir.with_extension("dat");
        std::fs::write(&path, b"").expect("建空文件");
        let mut receiver = FileReceiver::create(&path, 0, 4).expect("接收端");
        let hash = receiver.finish(None).expect("收尾");
        assert_eq!(hash, crate::digest::hash_bytes(b""));
        let _ = std::fs::remove_file(&path); // gate-allow R-C-2: 用例收尾删除临时文件，失败不影响断言结论
    }

    /// 下一块再接收时，重复块返回 Duplicate 且不增加计数。
    #[test]
    fn duplicate_is_not_counted_twice() {
        let src = crate::tests_support::temp_path("recv-dup-src");
        std::fs::write(&src, b"abcdefghij").expect("写源");
        let dst = crate::tests_support::temp_path("recv-dup-dst");
        let mut sender = FileSender::open(&src, 4).expect("发送端");
        let mut receiver = FileReceiver::create(&dst, 10, 4).expect("接收端");
        let first = sender.next_chunk().expect("读块").expect("有块");
        assert!(matches!(
            receiver.accept(&first).expect("接收"),
            AcceptOutcome::Accepted(_)
        ));
        assert_eq!(
            receiver.accept(&first).expect("重复接收"),
            AcceptOutcome::Duplicate
        );
        assert_eq!(receiver.progress().chunks_done, 1);
        for _ in 0..2 {
            let chunk = sender.next_chunk().expect("读块").expect("有块");
            receiver.accept(&chunk).expect("接收");
        }
        receiver.finish(Some(sender.file_hash())).expect("收尾");
        let _ = std::fs::remove_file(&src); // gate-allow R-C-2: 用例收尾删除临时文件，失败不影响断言结论
        let _ = std::fs::remove_file(&dst); // gate-allow R-C-2: 用例收尾删除临时文件，失败不影响断言结论
    }
}
