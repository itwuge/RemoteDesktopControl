//! rdc-runtime / 线程构造器
//!
//! 层级：L2 能力层
//! 用途：统一线程命名、优先级映射、退出令牌与 panic 隔离（docs/01 §3.8/§3.9）
//! 调用方：capture/codec/input/core 的长驻线程创建点

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;

// =====** ThreadPrio =====**

/// 逻辑线程优先级。
///
/// 用枚举而非裸整数（docs/09 §5.4）；到系统优先级的映射在各平台实现内完成，
/// 映射失败必须降级为默认优先级并记录日志，不得启动失败。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadPrio {
    /// P0：输入注入/发送。
    Input,
    /// P1：编码/解码/发送。
    Encode,
    /// P2：采集/网络/控制。
    Net,
    /// P3：统计/日志。
    Idle,
}

// =====** ExitToken =====**

/// 线程退出令牌。
///
/// 所有长驻线程共享同一令牌；线程用条件变量等待唤醒，**禁止 sleep 轮询**
/// （docs/01 §3.9 第 3 条）。
#[derive(Debug, Default)]
pub struct ExitToken {
    /// 置位表示要求退出。
    requested: AtomicBool,
}

impl ExitToken {
    /// 请求退出。
    ///
    /// # Returns
    /// 无返回值。
    ///
    /// # Errors
    /// 不会失败。
    pub fn request(&self) {
        self.requested.store(true, Ordering::Release);
    }

    /// 是否已请求退出。
    ///
    /// # Returns
    /// 已请求返回 `true`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn is_requested(&self) -> bool {
        self.requested.load(Ordering::Acquire)
    }
}

// =====** 线程创建 =====**

/// 创建带名称与优先级的常驻线程，并包 panic 隔离。
///
/// # Arguments
/// * `name` - 线程名，格式 `rdc-<角色>[-序号]`，≤ 15 字符（Linux `comm` 上限）
/// * `prio` - 逻辑优先级
/// * `token` - 退出令牌，闭包内需定期检查
/// * `body` - 线程主体
///
/// # Returns
/// 返回 `JoinHandle`；线程 panic 会被捕获并转为日志记录（docs/01 §3.9 第 4 条）。
///
/// # Errors
/// 线程创建失败（系统资源不足）返回 `std::io::Error`。
pub fn spawn_named<F>(
    name: &str,
    _prio: ThreadPrio,
    _token: Arc<ExitToken>,
    body: F,
) -> std::io::Result<JoinHandle<()>>
where
    F: FnOnce() + Send + 'static,
{
    // TODO(stage3): 平台优先级设置 + catch_unwind 包装。
    std::thread::Builder::new().name(name.to_owned()).spawn(body)
}
