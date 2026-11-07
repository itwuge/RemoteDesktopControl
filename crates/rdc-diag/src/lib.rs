//! ============================================================
//! rdc-diag · 统一诊断基础设施（L1）
//!
//! 层级：L1 基础设施，不依赖任何业务 crate。
//! 用途：进程级日志初始化（幂等）、`RDC_LOG` 别名过滤、与旧 `eprintln!` 兼容的人读格式、
//!       崩溃快照环形缓冲；JSONL 落盘与 frame_trace 合流在后续里程碑补齐。
//! 调用方：仅进程入口（rdc-cli / rdc-bridge / rdc-signal / rdc-relay / Windows runner）。
//! 边界：库 crate 只使用 `tracing` 宏，不得自行初始化全局 subscriber（docs/16 §2.6）。
//! ============================================================

// =====** 模块声明 =====**

pub mod error;
pub mod filter;
pub mod frame_trace;
pub mod panic_hook;
pub mod platform_windows;
pub mod record;
pub mod redact;
pub mod ring;

// =====** 模块导入 =====**

use std::io;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

// =====** 全局状态 =====**

/// 进程内只初始化一次的诊断状态；`OnceLock` 保证重复 `init` 不再安装 subscriber。
static DIAG: OnceLock<Arc<DiagState>> = OnceLock::new();

/// 进程级诊断状态：服务名、生效过滤器与启动时刻。
struct DiagState {
    /// 进程入口传入的服务名（`cli` / `bridge` / `signal` / `relay`）。
    service: String,
    /// 实际生效的 `RDC_LOG` 表达式（已展开别名）。
    filter: String,
    /// 初始化时刻，用于 `uptime` 类事件。
    started: Instant,
}

// =====** 初始化守卫 =====**

/// 日志初始化守卫。
///
/// # 语义
/// * 由进程入口调用 [`init`] 获取；
/// * `drop` 时自动冲刷 sink，也可显式调用 [`LogGuard::flush`]；
/// * 多个 guard 共享同一底层状态，重复 drop 只是重复冲刷，不会重复安装 subscriber。
pub struct LogGuard {
    state: Arc<DiagState>,
}

impl LogGuard {
    /// 返回进程入口登记的服务名。
    pub fn service(&self) -> &str {
        &self.state.service
    }

    /// 返回实际生效的 `RDC_LOG` 表达式（别名已展开）。
    pub fn filter(&self) -> &str {
        &self.state.filter
    }

    /// 返回初始化至今的时长。
    pub fn uptime(&self) -> Duration {
        self.state.started.elapsed()
    }

    /// 冲刷日志 sink；进程退出前调用可保证尾部不丢。
    pub fn flush(&self) {
        record::flush();
    }
}

impl Drop for LogGuard {
    fn drop(&mut self) {
        // guard 可能先于进程结束被 drop；这里只做冲刷，不注销 subscriber。
        self.flush();
    }
}

// =====** 初始化入口 =====**

/// 初始化进程级日志与 panic hook；幂等。
///
/// # Arguments
/// * `service` - 进程名（`cli` / `bridge` / `signal` / `relay`），写入 `diag.start` 事件。
///
/// # Returns
/// 持有诊断状态的 [`LogGuard`]；进程退出前 drop 会自动冲刷。
///
/// # 过滤
/// `RDC_LOG` 未设置或为空时默认 `info`；语法与别名见 [`filter::parse`]。
/// 日志初始化失败（过滤器非法、subscriber 已存在）只降级并写一条 fallback，不 panic、不阻塞启动。
pub fn init(service: &str) -> LogGuard {
    let state = DIAG.get_or_init(|| {
        // 环境变量只在进程入口读一次，库 crate 不得自行读取做分支（docs/16 §2.2）。
        // 用 `ok()` 把“未设置”和“读取失败”统一视为未配置，由 filter::parse 使用默认值。
        let parsed = filter::parse(std::env::var("RDC_LOG").ok().as_deref());
        for alias in &parsed.unknown_aliases {
            record::write_fallback(
                "WARN",
                "diag",
                &format!(
                    "RDC_LOG alias '{}' not in alias table; treated as target prefix",
                    alias
                ),
            );
        }
        let env_filter = match tracing_subscriber::EnvFilter::try_new(&parsed.expression) {
            Ok(value) => value,
            Err(_) => {
                // 非法表达式不改变启动行为：退回默认过滤，并把原文写进 fallback。
                record::write_fallback(
                    "WARN",
                    "diag",
                    &format!(
                        "RDC_LOG='{}' invalid; fallback to '{}'",
                        parsed.expression,
                        filter::DEFAULT_FILTER
                    ),
                );
                tracing_subscriber::EnvFilter::new(filter::DEFAULT_FILTER)
            }
        };
        // 默认 sink 是 stderr：与旧 eprintln! 的可观察性一致，不依赖日志目录可写。
        let subscriber = tracing_subscriber::fmt()
            .event_format(record::HumanFormat)
            .with_env_filter(env_filter)
            .with_writer(io::stderr)
            .finish();
        match tracing::subscriber::set_global_default(subscriber) {
            Ok(()) => {}
            Err(error) => {
                // 宿主（Flutter 引擎或测试）已装 subscriber：保留它，仅提示一次。
                record::write_fallback(
                    "WARN",
                    "diag",
                    &format!("global subscriber already installed; keeping it: {}", error),
                );
            }
        }
        panic_hook::install();
        tracing::info!(
            target: "rdc_diag",
            event = "diag.start",
            service = service,
            filter = %parsed.expression,
            pid = std::process::id(),
            "diagnostics initialized"
        );
        Arc::new(DiagState {
            service: service.to_string(),
            filter: parsed.expression,
            started: Instant::now(),
        })
    });
    LogGuard {
        state: Arc::clone(state),
    }
}

/// 返回最近的环形缓冲日志行（崩溃反馈包 / `rdc-cli doctor` 使用）。
///
/// # Arguments
/// * `limit` - 最多返回的行数，按时间升序。
pub fn recent_logs(limit: usize) -> Vec<String> {
    ring::global().dump(limit)
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_is_idempotent() {
        let first = init("test-diag");
        let second = init("test-diag");
        // 两次调用必须共享同一状态：服务名与过滤器一致，且不 panic。
        assert_eq!(first.service(), "test-diag");
        assert_eq!(second.filter(), first.filter());
        second.flush();
    }

    #[test]
    fn recent_logs_are_bounded() {
        // 不依赖具体条数：只要请求上限生效，返回值长度不会超过上限。
        let lines = recent_logs(8);
        assert!(lines.len() <= 8);
    }
}
