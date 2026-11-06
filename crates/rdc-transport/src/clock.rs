//! rdc-transport / 时钟偏移估计
//!
//! 层级：L2 能力层
//! 用途：估计本端与对端的时钟偏移，把对端时间戳换算到本端单调时钟；
//!       S1 只保留偏移字段与换算接口
//! 调用方：rdc-core（估计 RTT 与回放时序对齐）、rdc-transport 的 RTT 反馈路径

// =====** ClockEstimator =====**

/// 时钟偏移估计器。
pub struct ClockEstimator {
    /// 对端时钟相对本端的偏移（纳秒），正数表示对端时钟更快。
    offset_ns: i64,
}

impl ClockEstimator {
    /// 构造零偏移的估计器。
    ///
    /// # Returns
    /// 偏移为 0 的实例，首次样本到达前处于未知状态。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new() -> Self {
        Self { offset_ns: 0 }
    }

    /// 喂入一组时间戳样本更新偏移。
    ///
    /// # Arguments
    /// * `remote_ns` - 对端报文中携带的时间戳（纳秒）
    /// * `local_ns` - 收到报文时本端单调时钟读数（纳秒）
    ///
    /// # Returns
    /// 无返回值，结果通过 `offset_ns` 体现。
    ///
    /// # Errors
    /// 不会失败；异常样本（明显倒退）由实现内部丢弃。
    ///
    /// # Panics
    /// S1 空壳：实现落地前调用会 panic。
    pub fn on_remote_timestamp(&mut self, _remote_ns: u64, _local_ns: u64) {
        todo!("S1 空壳：时钟偏移估计在 S2 实现")
    }

    /// 返回当前偏移估计。
    ///
    /// # Returns
    /// 偏移量（纳秒）。
    ///
    /// # Errors
    /// 不会失败。
    pub fn offset_ns(&self) -> i64 {
        self.offset_ns
    }

    /// 把对端时间戳换算到本端时钟。
    ///
    /// # Arguments
    /// * `remote_ns` - 对端时间戳（纳秒）
    ///
    /// # Returns
    /// 换算后的本端时间戳（纳秒）。
    ///
    /// # Errors
    /// 不会失败；结果为负时返回 0（时间戳不允许回退到 u64 的负区间）。
    ///
    /// # Panics
    /// S1 空壳：实现落地前调用会 panic。
    pub fn to_local(&self, _remote_ns: u64) -> u64 {
        todo!("S1 空壳：时钟换算在 S2 实现")
    }
}
