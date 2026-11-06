//! rdc-transport / NACK 重传缓存
//!
//! 层级：L2 能力层
//! 用途：缓存已发送分片，按接收端反馈找出缺失序号并取回原始载荷用于重传
//! 调用方：rdc-core 的发送任务（发送后入缓存、收到 NACK 后取回重发）、
//!         接收任务（生成缺失序号列表）

// =====** NackCache =====**

/// 按序号保留已发送分片的缓存。
///
/// S1 只记录容量：底层结构（环形数组 vs 哈希表）取决于 S2 的 NACK 实测分布，
/// 先固定对外行为，避免过早绑定实现。
pub struct NackCache {
    /// 缓存容量，生产配置取 `params::NACK_CACHE_LEN`。
    capacity: usize,
}

impl NackCache {
    /// 构造缓存。
    ///
    /// # Arguments
    /// * `capacity` - 可保留的分片数量；0 表示禁用重传
    ///
    /// # Returns
    /// 空缓存实例。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new(capacity: usize) -> Self {
        Self { capacity }
    }

    /// 记录一个已发送分片。
    ///
    /// # Arguments
    /// * `seq` - 分片序号
    /// * `payload` - 已发送的载荷，供重传时原样返回
    ///
    /// # Errors
    /// 不会失败；缓存满时按序号覆盖最旧分片。
    ///
    /// # Panics
    /// S1 空壳：实现落地前调用会 panic。
    pub fn record(&mut self, _seq: u32, _payload: Vec<u8>) {
        todo!("S1 空壳：NACK 记录在 S2 实现")
    }

    /// 计算在接收端已收到的序号集合下缺失的序号。
    ///
    /// # Arguments
    /// * `received` - 接收端已确认收到的序号
    ///
    /// # Returns
    /// 缓存内未被确认的序号，按升序排列。
    ///
    /// # Errors
    /// 不会失败。
    ///
    /// # Panics
    /// S1 空壳：实现落地前调用会 panic。
    pub fn missing(&self, _received: &[u32]) -> Vec<u32> {
        todo!("S1 空壳：NACK 缺失计算在 S2 实现")
    }

    /// 按序号取回载荷用于重传。
    ///
    /// # Arguments
    /// * `seq` - 需要重传的分片序号
    ///
    /// # Returns
    /// 命中时返回载荷副本，已过期或从未记录时返回 `None`。
    ///
    /// # Errors
    /// 不会失败。
    ///
    /// # Panics
    /// S1 空壳：实现落地前调用会 panic。
    pub fn fetch(&self, _seq: u32) -> Option<Vec<u8>> {
        todo!("S1 空壳：NACK 取回在 S2 实现")
    }
}
