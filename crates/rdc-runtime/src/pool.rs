//! rdc-runtime / 帧内存池
//!
//! 层级：L2 能力层
//! 用途：固定块池，引用计数借用/归还，禁止逐帧 `Vec` 分配（docs/01 §3.1）
//! 调用方：rdc-capture（借出写帧）、rdc-codec（借出/归还）、rdc-core
//!
//! 实现说明：S3a 起为真实实现。空闲块存放在 `Mutex<Vec<Vec<u8>>>` 中，
//! 借出/归还只在采集线程与编码线程各自发生，锁竞争极低；S3b 若需要严格无锁，
//! 可在不改调用方接口的前提下替换为 SPSC 空闲队列。

use std::sync::{Arc, Mutex};

// =====** PooledBuffer =====**

/// 从内存池借出的帧缓冲。
///
/// # 所有权
/// 由 [`FramePool::acquire`] 创建，最后一个引用析构时自动归还到池；
/// **禁止**手动 `free`，也禁止跨进程/跨 FFI 传递（docs/01 §3.2 读图要点 3）。
pub struct PooledBuffer {
    /// 借出的字节缓冲；`None` 只出现在 `Drop` 取走数据的一瞬间。
    data: Option<Vec<u8>>,
    /// 归还目标池。
    owner: Arc<PoolState>,
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
        self.data.as_deref().unwrap_or(&[])
    }

    /// 可写访问缓冲内容（采集侧填帧使用）。
    ///
    /// # Returns
    /// 可写字节切片，长度等于池创建时声明的块大小。
    ///
    /// # Errors
    /// 不会失败。
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        self.data.as_deref_mut().unwrap_or(&mut [])
    }

    /// 缓冲字节长度。
    ///
    /// # Returns
    /// 池创建时声明的块大小。
    ///
    /// # Errors
    /// 不会失败。
    pub fn len(&self) -> usize {
        self.data.as_ref().map_or(0, Vec::len)
    }

    /// 缓冲是否为空。
    ///
    /// # Returns
    /// 块大小为 0 时返回 `true`（正常池不会出现）。
    ///
    /// # Errors
    /// 不会失败。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Drop for PooledBuffer {
    /// 归还缓冲：只在析构时发生，保证不会出现悬垂引用。
    ///
    /// 为什么在 Drop 里归还：帧的所有权在 capture → codec → 编码输出间流转，
    /// 用 RAII 保证任何提前返回/出错路径都会把块送回池，避免泄漏。
    fn drop(&mut self) {
        if let Some(buf) = self.data.take() {
            // 归还失败（池锁被 panic 污染）时直接丢弃该块：宁可少一块，不 panic 采集线程。
            if let Ok(mut free) = self.owner.free.lock() {
                free.push(buf);
            }
        }
    }
}

// =====** PoolState =====**

/// 池的共享内部状态（`Arc` 被所有借出缓冲共享）。
struct PoolState {
    /// 单块字节数。
    block_bytes: usize,
    /// 总块数。
    blocks: usize,
    /// 空闲块。
    free: Mutex<Vec<Vec<u8>>>,
}

// =====** FramePool =====**

/// 固定块数的帧内存池。
///
/// 默认 8 块，按最大分辨率预分配；容量创建后不可变，避免运行期内存增长。
pub struct FramePool {
    /// 共享内部状态。
    state: Arc<PoolState>,
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
        // 一次性预分配全部块：运行期不再扩容，保证采集线程无分配抖动。
        let free = (0..blocks).map(|_| vec![0u8; block_bytes]).collect();
        Self {
            state: Arc::new(PoolState {
                block_bytes,
                blocks,
                free: Mutex::new(free),
            }),
        }
    }

    /// 借出一块缓冲；池空时返回 `None`，由调用方丢帧而不是等待。
    ///
    /// # Returns
    /// 成功返回 [`PooledBuffer`]；无空闲块返回 `None`。
    ///
    /// # Errors
    /// 不阻塞、不失败，池空用 `None` 表达。
    pub fn acquire(&self) -> Option<PooledBuffer> {
        let buf = self.state.free.lock().ok()?.pop()?;
        Some(PooledBuffer {
            data: Some(buf),
            owner: Arc::clone(&self.state),
        })
    }

    /// 块总数（用于统计 `pool_exhausted_count`）。
    ///
    /// # Returns
    /// 创建时声明的块数。
    ///
    /// # Errors
    /// 不会失败。
    pub fn capacity(&self) -> usize {
        self.state.blocks
    }

    /// 单块字节数。
    ///
    /// # Returns
    /// 创建时声明的块大小。
    ///
    /// # Errors
    /// 不会失败。
    pub fn block_bytes(&self) -> usize {
        self.state.block_bytes
    }

    /// 当前空闲块数（诊断用）。
    ///
    /// # Returns
    /// 空闲块数量；锁被污染时返回 0。
    ///
    /// # Errors
    /// 不会失败。
    pub fn free_blocks(&self) -> usize {
        self.state.free.lock().map(|f| f.len()).unwrap_or(0)
    }
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 借出数量不得超过容量，归还后可再次借出。
    #[test]
    fn acquire_and_return_roundtrip() {
        let pool = FramePool::new(2, 16);
        let a = pool.acquire().expect("第一块可借");
        let b = pool.acquire().expect("第二块可借");
        assert!(pool.acquire().is_none(), "两块借完后池应为空");
        drop(a);
        assert_eq!(pool.free_blocks(), 1);
        let c = pool.acquire().expect("归还后可再次借出");
        assert_eq!(c.len(), 16);
        drop((b, c));
        assert_eq!(pool.free_blocks(), 2);
    }

    /// 写入内容后仍然读得到，说明复用的是同一块内存。
    #[test]
    fn borrowed_buffer_is_writable_and_readable() {
        let pool = FramePool::new(1, 8);
        let mut buf = pool.acquire().expect("可借");
        buf.as_mut_slice()[0] = 0xAB;
        assert_eq!(buf.as_slice()[0], 0xAB);
        assert!(!buf.is_empty());
    }

    /// 池参数为 0 必须 panic（编程错误，而不是运行期静默降级）。
    #[test]
    #[should_panic(expected = "内存池参数必须为正")]
    fn zero_capacity_panics() {
        let _ = FramePool::new(0, 16);
    }
}
