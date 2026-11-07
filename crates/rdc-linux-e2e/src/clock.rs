//! rdc-linux-e2e / 单调时钟
//!
//! 用途：给「采集→编码→传输→解码→上屏」的每一跳打时间戳。
//! 为什么不用 `Instant`：跨进程比较需要同一时钟原语；Linux 上 `Instant` 与
//! `CLOCK_MONOTONIC` 同源，本模块直接读 `clock_gettime(CLOCK_MONOTONIC)`，
//! 让 host/client 两个进程的纳秒时间戳可直接相减。

/// 读取 `CLOCK_MONOTONIC` 纳秒时间戳。
///
/// # Returns
/// 单调递增的纳秒数；系统调用失败时返回 0（调用方按无效样本处理）。
///
/// # Errors
/// 不会失败。
pub fn mono_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY：ts 是合法的 timespec 指针，CLOCK_MONOTONIC 是该平台常量。
    let rc = unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    if rc != 0 {
        return 0;
    }
    (ts.tv_sec as u64) * 1_000_000_000 + ts.tv_nsec as u64
}
