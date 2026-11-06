//! rdc-runtime / 无锁 SPSC 环形缓冲
//!
//! 层级：L2 能力层
//! 用途：阶段之间唯一的传递方式（docs/01 T3）：每环一个生产者一个消费者
//! 调用方：capture（写环 A）、codec（读环 A / 写环 B）、transport（读环 B）、core

// =====** SpscRing =====**

/// 单生产者单消费者环形缓冲。
///
/// # 所有权
/// - 生产者与消费者各自持有唯一句柄，`push`/`pop` 不得并发调用同一端；
/// - 跨线程传递时由调用方用 `std::thread::spawn` move 各自句柄（`Send` 需手动评审）；
/// - 内部使用原子量做序号交换，**禁止**在此基础上再加锁。
pub struct SpscRing<T> {
    /// 预分配的槽位；容量固定后不再增长，避免逐帧分配（docs/01 §3.1 帧内存池段）。
    slots: Vec<Option<T>>,
}

impl<T> SpscRing<T> {
    /// 创建固定容量的 SPSC 环。
    ///
    /// # Arguments
    /// * `capacity` - 环容量，创建后不可变；容量常量必须来自各 crate 的 `params.rs`
    ///
    /// # Returns
    /// 生产者端与消费者端两个句柄。
    ///
    /// # Errors
    /// `capacity == 0` 时 panic（调用方必须传常量，属编程错误）。
    ///
    /// # Panics
    /// 容量为 0 时 panic。容量来自编译期常量，正常路径不可能触发。
    pub fn spsc(capacity: usize) -> (Self, Self) {
        assert!(capacity > 0, "SPSC 环容量必须大于 0");
        // TODO(stage3): 分配真实双端句柄；S1 仅保留接口形状，保证 workspace 可编译。
        todo!("S1 空壳：SPSC 环在 stage3 实现")
    }

    /// 尝试写入一个元素。
    ///
    /// # Arguments
    /// * `item` - 待写入的元素（帧句柄或码流包）
    ///
    /// # Returns
    /// 写入成功返回 `Ok(())`；环满返回 `Err(item)`，由调用方按"丢旧留新"处理。
    ///
    /// # Errors
    /// 环满时返回被拒绝的元素，**不阻塞**（docs/01 T4）。
    pub fn push(&self, _item: T) -> Result<(), T> {
        todo!("S1 空壳：SPSC 环在 stage3 实现")
    }

    /// 尝试取出一个元素。
    ///
    /// # Returns
    /// 有数据返回 `Some`，空环返回 `None`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn pop(&self) -> Option<T> {
        todo!("S1 空壳：SPSC 环在 stage3 实现")
    }

    /// 当前积压元素数量（用于统计与 QoS 判断）。
    ///
    /// # Returns
    /// 近似值（读取时两端可能正在并发修改），仅用于观测。
    ///
    /// # Errors
    /// 不会失败。
    pub fn len(&self) -> usize {
        todo!("S1 空壳：SPSC 环在 stage3 实现")
    }
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// S1 仅验证类型可用；真实行为测试在 stage3 随实现补齐。
    #[test]
    fn ring_type_is_object_safe() {
        let _factory: fn(usize) -> (SpscRing<u8>, SpscRing<u8>) = SpscRing::spsc;
    }
}
