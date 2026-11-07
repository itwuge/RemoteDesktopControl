//! rdc-filetransfer / 发送端状态机
//!
//! 层级：L3 交互层
//! 用途：按分块计划读盘、给出待发分块、记录已确认块并回调进度；重连后从已确认块之后继续。
//! 调用方：rdc-core 的文件发送任务（发送循环：next_chunk → 发网络 → confirm）
//!
//! 断点续传：对端把自己的已收位图回传 → [`FileSender::resume_with`] → 后续
//! [`FileSender::next_chunk`] 自动跳过这些块，不重发。

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use crate::digest::hash_file;
use crate::error::FileTransferError;
use crate::plan::{Chunk, ChunkPlan, Progress, DEFAULT_CHUNK_SIZE};

/// 进度回调（每次确认一块触发一次）。
pub type ProgressHook = Box<dyn FnMut(Progress) + Send>;

// =====** FileSender =====**

/// 文件发送端。
pub struct FileSender {
    /// 源文件路径（仅日志用）。
    path: PathBuf,
    /// 已打开的源文件。
    file: File,
    /// 分块计划。
    plan: ChunkPlan,
    /// 整文件 SHA-256（随 manifest 发给对端）。
    file_hash: [u8; 32],
    /// 已确认分块位图。
    confirmed: Vec<bool>,
    /// 顺序发包游标。
    cursor: u32,
    /// 已确认块数。
    done: u32,
    /// 已确认字节数。
    done_bytes: u64,
    /// 进度回调。
    hook: Option<ProgressHook>,
}

impl FileSender {
    /// 打开文件并计算整文件摘要。
    ///
    /// # Arguments
    /// * `path` - 源文件路径
    /// * `chunk_size` - 分块大小，传 0 使用 [`DEFAULT_CHUNK_SIZE`](crate::DEFAULT_CHUNK_SIZE)
    ///
    /// # Returns
    /// 尚未确认任何分块的发送端。
    ///
    /// # Errors
    /// 文件打开/读取失败返回 [`FileTransferError::Io`]。
    pub fn open(path: impl AsRef<Path>, chunk_size: u64) -> Result<Self, FileTransferError> {
        let path = path.as_ref().to_path_buf();
        let chunk_size = if chunk_size == 0 {
            DEFAULT_CHUNK_SIZE
        } else {
            chunk_size
        };
        let file = File::open(&path)?;
        let total = file.metadata()?.len();
        let plan = ChunkPlan::new(total, chunk_size)?;
        let file_hash = hash_file(&path)?;
        let count = plan.chunk_count();
        Ok(Self {
            path,
            file,
            plan,
            file_hash,
            confirmed: vec![false; count as usize],
            cursor: 0,
            done: 0,
            done_bytes: 0,
            hook: None,
        })
    }

    /// 挂进度回调（链式调用，替换旧回调）。
    ///
    /// # Arguments
    /// * `hook` - 每次确认分块时调用
    ///
    /// # Returns
    /// 自身，便于链式构造。
    pub fn with_progress(mut self, hook: impl FnMut(Progress) + Send + 'static) -> Self {
        self.hook = Some(Box::new(hook));
        self
    }

    /// 分块计划。
    pub fn plan(&self) -> &ChunkPlan {
        &self.plan
    }

    /// 源文件路径（日志用）。
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// 整文件 SHA-256（发送前随 manifest 一并下发，接收端据此最终比对）。
    pub fn file_hash(&self) -> [u8; 32] {
        self.file_hash
    }

    /// 取下一个待发分块（跳过已确认块）。
    ///
    /// # Returns
    /// 全部确认或文件为空时返回 `None`。
    ///
    /// # Errors
    /// 读盘失败返回 [`FileTransferError::Io`]。
    pub fn next_chunk(&mut self) -> Result<Option<Chunk>, FileTransferError> {
        while self.cursor < self.plan.chunk_count() {
            let index = self.cursor;
            self.cursor += 1;
            if !self.confirmed[index as usize] {
                return Ok(Some(self.read_chunk(index)?));
            }
        }
        Ok(None)
    }

    /// 按需重读某个分块（失败块重传用，不推进游标）。
    ///
    /// # Arguments
    /// * `index` - 分块下标
    ///
    /// # Returns
    /// 该分块内容与哈希。
    ///
    /// # Errors
    /// 下标越界返回 [`FileTransferError::ChunkIndexOutOfRange`]；读盘失败返回 `Io`。
    pub fn request_chunk(&mut self, index: u32) -> Result<Chunk, FileTransferError> {
        if index >= self.plan.chunk_count() {
            return Err(FileTransferError::ChunkIndexOutOfRange {
                index,
                count: self.plan.chunk_count(),
            });
        }
        self.read_chunk(index)
    }

    /// 记录分块确认结果。
    ///
    /// # Arguments
    /// * `index` - 分块下标
    /// * `ok` - 对端是否确认收到且校验通过
    ///
    /// # Returns
    /// 最新进度（`ok == false` 时只校验下标，计数不变）。
    ///
    /// # Errors
    /// 下标越界返回 [`FileTransferError::ChunkIndexOutOfRange`]。
    pub fn confirm(&mut self, index: u32, ok: bool) -> Result<Progress, FileTransferError> {
        let count = self.plan.chunk_count();
        if index >= count {
            return Err(FileTransferError::ChunkIndexOutOfRange { index, count });
        }
        let slot = &mut self.confirmed[index as usize];
        if ok && !*slot {
            *slot = true;
            self.done += 1;
            self.done_bytes += self.plan.chunk_len(index).unwrap_or(0);
            let progress = self.progress();
            if let Some(hook) = self.hook.as_mut() {
                hook(progress);
            }
        }
        Ok(self.progress())
    }

    /// 用对端回传的已收位图恢复发送进度（重连后调用）。
    ///
    /// # Arguments
    /// * `confirmed` - 长度为分块数的已收位图
    ///
    /// # Returns
    /// 无返回值；游标重置为 0，后续 [`Self::next_chunk`] 会跳过已确认块。
    ///
    /// # Errors
    /// 位图长度与分块数不符返回 [`FileTransferError::MalformedState`]。
    pub fn resume_with(&mut self, confirmed: &[bool]) -> Result<(), FileTransferError> {
        let count = self.plan.chunk_count();
        if confirmed.len() != count as usize {
            return Err(FileTransferError::MalformedState);
        }
        self.confirmed.copy_from_slice(confirmed);
        self.done = 0;
        self.done_bytes = 0;
        for index in 0..count {
            if self.confirmed[index as usize] {
                self.done += 1;
                self.done_bytes += self.plan.chunk_len(index).unwrap_or(0);
            }
        }
        self.cursor = 0;
        Ok(())
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

    /// 已确认位图。
    pub fn confirmed(&self) -> &[bool] {
        &self.confirmed
    }

    /// 读一个分块。
    fn read_chunk(&mut self, index: u32) -> Result<Chunk, FileTransferError> {
        let offset = self.plan.chunk_offset(index).ok_or_else(|| {
            FileTransferError::ChunkIndexOutOfRange {
                index,
                count: self.plan.chunk_count(),
            }
        })?;
        let len = self.plan.chunk_len(index).unwrap_or(0) as usize;
        self.file.seek(SeekFrom::Start(offset))?;
        let mut data = vec![0u8; len];
        self.file.read_exact(&mut data)?;
        Ok(Chunk::new(index, offset, data))
    }
}
