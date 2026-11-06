//! rdc-transport / NACK 重传缓存
//!
//! 层级：L2 能力层
//! 用途：缓存最近发出的分片，按接收端反馈找出缺失序号，供发送端原样重发
//!       （docs/04 §3.2：保留最近约 250ms 已发包，环形覆盖）。
//! 调用方：rdc-core 的发送任务（发送后入缓存、收到 NACK 后取回重发）

use std::collections::{BTreeSet, HashSet, VecDeque};

// =====** NackCache =====**

/// 按序号保留已发送分片的环形缓存。
pub struct NackCache {
    /// 可保留的分片数量；0 表示禁用重传。
    capacity: usize,
    /// 按发送顺序保存的 (序号, 载荷)；超过容量时从队首淘汰。
    entries: VecDeque<(u32, Vec<u8>)>,
    /// 命中次数，用于观测重传是否真的被用上。
    hits: u64,
    /// 未命中次数（序号已过期或从未发送）。
    misses: u64,
}

impl NackCache {
    /// 构造缓存。
    ///
    /// # Arguments
    /// * `capacity` - 可保留的分片数量；0 表示禁用重传
    ///
    /// # Returns
    /// 空缓存实例。
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: VecDeque::new(),
            hits: 0,
            misses: 0,
        }
    }

    /// 记录一个已发送分片。
    ///
    /// 容量为 0 时直接忽略（等价于关闭重传）；缓存满时覆盖最旧的一条。
    ///
    /// # Arguments
    /// * `seq` - 分片序号
    /// * `payload` - 已发送的载荷，供重传时原样返回
    pub fn record(&mut self, seq: u32, payload: Vec<u8>) {
        if self.capacity == 0 {
            return;
        }
        self.entries.push_back((seq, payload));
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
    }

    /// 计算在接收端已收到的序号集合下缺失的序号。
    ///
    /// # Arguments
    /// * `received` - 接收端已确认收到的序号
    ///
    /// # Returns
    /// 缓存内未被确认的序号，按升序排列且去重。
    pub fn missing(&self, received: &[u32]) -> Vec<u32> {
        let acked: HashSet<u32> = received.iter().copied().collect();
        let mut gaps = BTreeSet::new();
        for (seq, _) in &self.entries {
            if !acked.contains(seq) {
                gaps.insert(*seq);
            }
        }
        gaps.into_iter().collect()
    }

    /// 按序号取回载荷用于重传。
    ///
    /// # Arguments
    /// * `seq` - 需要重传的分片序号
    ///
    /// # Returns
    /// 命中时返回载荷副本，已过期或从未记录时返回 `None`。
    pub fn fetch(&mut self, seq: u32) -> Option<Vec<u8>> {
        if let Some((_, payload)) = self.entries.iter().rev().find(|(s, _)| *s == seq) {
            self.hits += 1;
            return Some(payload.clone());
        }
        self.misses += 1;
        None
    }

    /// 当前缓存的分片数。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 缓存是否为空。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 命中次数。
    pub fn hits(&self) -> u64 {
        self.hits
    }

    /// 未命中次数。
    pub fn misses(&self) -> u64 {
        self.misses
    }
}

// =====** 自检 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 记录后能按序号取回原始载荷。
    #[test]
    fn record_then_fetch_returns_payload() {
        let mut cache = NackCache::new(8);
        cache.record(1, vec![1, 2, 3]);
        cache.record(2, vec![4, 5]);
        assert_eq!(cache.fetch(1), Some(vec![1, 2, 3]));
        assert_eq!(cache.fetch(2), Some(vec![4, 5]));
        assert_eq!(cache.hits(), 2);
        assert_eq!(cache.fetch(99), None);
        assert_eq!(cache.misses(), 1);
    }

    /// 超过容量时覆盖最旧的序号。
    #[test]
    fn overflow_evicts_oldest() {
        let mut cache = NackCache::new(2);
        cache.record(1, vec![1]);
        cache.record(2, vec![2]);
        cache.record(3, vec![3]);
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.fetch(1), None, "序号 1 应已被淘汰");
        assert_eq!(cache.fetch(3), Some(vec![3]));
    }

    /// 缺失序号 = 缓存内未被确认的序号，按升序且去重。
    #[test]
    fn missing_returns_unacked_in_order() {
        let mut cache = NackCache::new(8);
        for seq in [5, 3, 1, 4] {
            cache.record(seq, vec![seq as u8]);
        }
        assert_eq!(cache.missing(&[1, 4]), vec![3, 5]);
        assert!(cache.missing(&[1, 3, 4, 5]).is_empty());
    }

    /// 容量为 0 时完全不保留，等价于关闭重传。
    #[test]
    fn zero_capacity_disables_retransmit() {
        let mut cache = NackCache::new(0);
        cache.record(1, vec![1]);
        assert_eq!(cache.len(), 0);
        assert_eq!(cache.fetch(1), None);
        assert!(cache.missing(&[]).is_empty());
    }
}
