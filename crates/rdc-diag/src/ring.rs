//! ============================================================
//! rdc-diag · 定长环形缓冲
//!
//! 层级：L1 基础设施（rdc-diag 内部模块）。
//! 用途：保留最近 N 条已通过过滤的人读日志，崩溃时 dump；容量固定，不随负载增长。
//! 调用方：record.rs（每条事件写入）、panic_hook.rs（崩溃 dump）。
//! 容量：`RDC_LOG_RING` 可覆盖，默认 512 行（`0` 表示禁用）。
//! ============================================================

// =====** 模块导入 =====**

use std::collections::VecDeque;
use std::sync::{Mutex, MutexGuard, OnceLock};

// =====** 常量与全局实例 =====**

/// 默认保留行数：覆盖崩溃前几秒的高频事件，同时避免放大内存。
pub const DEFAULT_CAPACITY: usize = 512;

/// 进程级实例；首次访问时按环境变量构造。
static GLOBAL: OnceLock<RingBuffer> = OnceLock::new();

// =====** 环形缓冲 =====**

/// 定长、线程安全的日志环形缓冲；满时丢最旧行。
pub struct RingBuffer {
    /// 最大保留行数；0 表示禁用。
    capacity: usize,
    /// 已保留的行，按时间升序。
    lines: Mutex<VecDeque<String>>,
}

impl RingBuffer {
    /// 创建指定容量的缓冲。
    ///
    /// # Arguments
    /// * `capacity` - 最大行数；`0` 表示禁用（不保留任何行）。
    pub fn new(capacity: usize) -> Self {
        RingBuffer {
            capacity,
            lines: Mutex::new(VecDeque::with_capacity(capacity.min(1024))),
        }
    }

    /// 追加一行；超过容量时丢弃最旧一行。
    ///
    /// # Arguments
    /// * `line` - 已格式化的单行日志（不含换行符）。
    pub fn push(&self, line: String) {
        if self.capacity == 0 {
            return;
        }
        let mut guard = self.lock();
        if guard.len() >= self.capacity {
            // 丢弃最旧一行：用 `_removed` 显式接收，避免 must_use 告警。
            let _removed = guard.pop_front();
        }
        guard.push_back(line);
    }

    /// 按时间升序返回最近 `limit` 行。
    ///
    /// # Arguments
    /// * `limit` - 最多返回行数；超过当前行数时返回全部。
    pub fn dump(&self, limit: usize) -> Vec<String> {
        let guard = self.lock();
        let start = guard.len().saturating_sub(limit);
        guard.iter().skip(start).cloned().collect()
    }

    /// 当前保留行数。
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// 是否没有任何保留行。
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 取锁；互斥锁被 panic 污染时继续使用内部数据，不因日志设施 panic。
    fn lock(&self) -> MutexGuard<'_, VecDeque<String>> {
        self.lines
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

// =====** 全局入口 =====**

/// 返回进程级环形缓冲（容量由 `RDC_LOG_RING` 决定）。
pub fn global() -> &'static RingBuffer {
    GLOBAL.get_or_init(|| RingBuffer::new(capacity_from_env()))
}

/// 读取 `RDC_LOG_RING`；非法值回退默认容量。
fn capacity_from_env() -> usize {
    match std::env::var("RDC_LOG_RING") {
        Ok(raw) => match raw.trim().parse::<usize>() {
            Ok(value) => value,
            Err(_) => DEFAULT_CAPACITY,
        },
        Err(_) => DEFAULT_CAPACITY,
    }
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_only_newest_lines() {
        let buffer = RingBuffer::new(3);
        for index in 0..5 {
            buffer.push(format!("line-{}", index));
        }
        assert_eq!(buffer.len(), 3);
        assert_eq!(buffer.dump(3), vec!["line-2", "line-3", "line-4"]);
    }

    #[test]
    fn dump_limit_is_respected() {
        let buffer = RingBuffer::new(8);
        buffer.push("only".to_string());
        assert_eq!(buffer.dump(10), vec!["only"]);
    }

    #[test]
    fn zero_capacity_disables_storage() {
        let buffer = RingBuffer::new(0);
        buffer.push("ignored".to_string());
        assert!(buffer.is_empty());
    }
}
