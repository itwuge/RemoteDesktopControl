//! rdc-runtime / 帧内存池
//!
//! 层级：L2 能力层
//! 用途：固定块池，引用计数借用/归还，禁止逐帧 `Vec` 分配（docs/01 §3.1）
//! 调用方：rdc-capture（借出写帧）、rdc-codec（借出/归还）、rdc-core

// =====** PooledBuffer =====**

/// 从内存池借出的帧缓冲。
///
/// # 所有权
/// 由 [`FramePool::acquire`] 创建，最后一个引用析构时自动归还到池；
/// **禁止**手动 `free`，也禁止跨进程/跨 FFI 传递（docs/01 §3.2 读图要点 3）。
pub struct PooledBuffer {
    /// 独占的字节缓冲视图；drop 时归还。
    data: Vec<u8>,
}

impl PooledBuffer {
    /// 只读访问缓冲内容。
    ///
    /// # Returns
    /// 字节切片，长度等于池创建时声明的块大小。
    ///
    /// # Errors
    /// 不会失败。
    pub fn as_slice(&self) -> &[u8] {
        &self.data
    }
}

// =====** FramePool =====**

/// 固定块数的帧内存池。
///
/// 默认 8 块，按最大分辨率预分配；容量创建后不可变，避免运行期内存增长。
pub struct FramePool {
    /// 块大小（字节）。
    block_bytes: usize,
    /// 总块数。
    blocks: usize,
}

impl FramePool {
    /// 创建内存池。
    ///
    /// # Arguments
    /// * `blocks` - 块数，默认 8（docs/01 §3.1）
    /// * `block_bytes` - 单块字节数，按最大分辨率与色度格式预计算
    ///
    /// # Returns
    /// 可直接借用的池。
    ///
    /// # Errors
    /// `blocks == 0` 或 `block_bytes == 0` 时 panic：容量来自编译期常量，属编程错误。
    ///
    /// # Panics
    /// 参数为 0 时 panic，正常路径不可能触发。
    pub fn new(blocks: usize, block_bytes: usize) -> Self {
        assert!(blocks > 0 && block_bytes > 0, "内存池参数必须为正");
        Self { block_bytes, blocks }
    }

    /// 借出一块缓冲；池空时返回 `None`，由调用方丢帧而不是等待。
    ///
    /// # Returns
    /// 成功返回 [`PooledBuffer`]；无空闲块返回 `None`。
    ///
    /// # Errors
    /// 不阻塞、不失败，池空用 `None` 表达。
    pub fn acquire(&self) -> Option<PooledBuffer> {
        todo!("S1 空壳：内存池在 stage3 实现")
    }

    /// 块总数（用于统计 `pool_exhausted_count`）。
    ///
    /// # Returns
    /// 创建时声明的块数。
    ///
    /// # Errors
    /// 不会失败。
    pub fn capacity(&self) -> usize {
        self.blocks
    }

    /// 单块字节数。
    ///
    /// # Returns
    /// 创建时声明的块大小。
    ///
    /// # Errors
    /// 不会失败。
    pub fn block_bytes(&self) -> usize {
        self.block_bytes
    }
}
