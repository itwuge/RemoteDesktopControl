//! rdc-protocol / 解码队列（深度 2，覆盖最旧）
//!
//! 层级：L2 能力层（纯数据结构）
//! 用途：定义“解码队列只保留最新 2 帧”的容量策略与覆盖计数；
//!       队列满时**覆盖最旧帧**而不是阻塞采集/网络线程（docs/04 §5）。
//! 调用方：rdc-core（解码调度）、rdc-codec（解码线程）、rdc-bridge（统计上报）

use crate::header::DECODE_QUEUE_DEPTH;

// =====** DecodeQueue =====**

/// 有界解码队列。
///
/// 为什么用覆盖而不是丢弃新帧：解码队列积压意味着解码速度跟不上，若继续排队，
/// 延迟会持续增长；覆盖最旧帧可以保证“看到的是最新画面”，代价是掉帧，
/// 掉帧通过 `overwrite_count` 暴露出来。
#[derive(Debug, Clone)]
pub struct DecodeQueue<T> {
    /// 容量（默认 [`DECODE_QUEUE_DEPTH`]）。
    capacity: usize,
    /// 待解码项，队首最旧。
    items: std::collections::VecDeque<T>,
    /// 覆盖最旧帧的累计次数（统计项 `decode_overwrite_count`）。
    overwrite_count: u64,
}

impl<T> DecodeQueue<T> {
    /// 创建指定容量的队列。
    ///
    /// # Arguments
    /// * `capacity` - 容量；传 0 会被提升为 1，避免空队列无法入队
    ///
    /// # Returns
    /// 空队列。
    ///
    /// # Errors
    /// 不会失败。
    pub fn with_capacity(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            capacity,
            items: std::collections::VecDeque::with_capacity(capacity),
            overwrite_count: 0,
        }
    }

    /// 按协议默认深度（2 帧）创建队列。
    ///
    /// # Returns
    /// 容量为 [`DECODE_QUEUE_DEPTH`] 的空队列。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new() -> Self {
        Self::with_capacity(DECODE_QUEUE_DEPTH)
    }

    /// 入队一项；队列满时覆盖最旧项。
    ///
    /// # Arguments
    /// * `item` - 待解码帧
    ///
    /// # Returns
    /// 被覆盖的最旧项；未发生覆盖时返回 `None`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn push(&mut self, item: T) -> Option<T> {
        if self.items.len() >= self.capacity {
            // 先弹出最旧项再入队，保证任意时刻长度不超过 capacity。
            let evicted = self.items.pop_front();
            self.overwrite_count = self.overwrite_count.saturating_add(1);
            self.items.push_back(item);
            evicted
        } else {
            self.items.push_back(item);
            None
        }
    }

    /// 取出最旧项（解码线程消费）。
    ///
    /// # Returns
    /// 空队列返回 `None`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn pop_front(&mut self) -> Option<T> {
        self.items.pop_front()
    }

    /// 查看最旧项的引用（不消费）。
    ///
    /// # Returns
    /// 空队列返回 `None`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn front(&self) -> Option<&T> {
        self.items.front()
    }

    /// 当前长度。
    ///
    /// # Returns
    /// 0~capacity。
    ///
    /// # Errors
    /// 不会失败。
    pub fn len(&self) -> usize {
        self.items.len()
    }

    /// 是否为空。
    ///
    /// # Returns
    /// 无待解码帧返回 `true`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// 累计覆盖次数（统计项 `decode_overwrite_count`）。
    ///
    /// # Returns
    /// 单调递增计数。
    ///
    /// # Errors
    /// 不会失败。
    pub fn overwrite_count(&self) -> u64 {
        self.overwrite_count
    }
}

impl<T> Default for DecodeQueue<T> {
    /// 使用协议默认深度创建队列。
    fn default() -> Self {
        Self::new()
    }
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 深度必须固定为 2（协议常量），覆盖最旧并计数。
    #[test]
    fn overwrites_oldest_and_counts() {
        let mut q = DecodeQueue::new();
        assert_eq!(q.push(1), None);
        assert_eq!(q.push(2), None);
        assert_eq!(q.len(), 2);
        assert_eq!(q.push(3), Some(1));
        assert_eq!(q.len(), 2);
        assert_eq!(q.overwrite_count(), 1);
        assert_eq!(q.push(4), Some(2));
        assert_eq!(q.overwrite_count(), 2);
        assert_eq!(q.pop_front(), Some(3));
        assert_eq!(q.pop_front(), Some(4));
        assert!(q.is_empty());
    }

    /// 容量为 0 必须被提升为 1，否则永远无法入队。
    #[test]
    fn zero_capacity_becomes_one() {
        let mut q = DecodeQueue::<u8>::with_capacity(0);
        assert_eq!(q.push(1), None);
        assert_eq!(q.push(2), Some(1));
        assert_eq!(q.len(), 1);
    }
}
