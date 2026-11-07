//! ============================================================
//! rdc-diag · Rust panic hook（崩溃不静默）
//!
//! 层级：L1 基础设施（rdc-diag 内部模块）。
//! 用途：panic 时输出线程名、位置、载荷与可选回溯，并 dump 环形缓冲最近日志。
//! 调用方：lib.rs 的 init（仅进程入口）。
//! 边界：只安装一次；仍调用原有 hook，保留 std 默认输出，不改变 panic 语义。
//! ============================================================

// =====** 模块导入 =====**

use std::backtrace::Backtrace;
use std::panic;
use std::sync::Once;
use std::thread;

use crate::record;
use crate::ring;

// =====** 常量 =====**

/// 崩溃 dump 默认带回的日志行数；够覆盖崩溃前几秒，又不至于淹没输出。
const RECENT_LINES: usize = 64;

/// 安装只执行一次的开关。
static INSTALL: Once = Once::new();

// =====** 安装入口 =====**

/// 安装 panic hook；重复调用只安装一次。
pub fn install() {
    INSTALL.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            let thread_name = thread::current().name().unwrap_or("<unnamed>").to_string();
            let location = match info.location() {
                Some(value) => format!("{}:{}:{}", value.file(), value.line(), value.column()),
                None => "<unknown>".to_string(),
            };
            let mut report = String::with_capacity(512);
            report.push_str(&format!(
                "{} ERROR [diag] panic.thread thread={} location={} payload={}",
                record::now_utc(),
                thread_name,
                location,
                payload_text(info.payload())
            ));
            if backtrace_enabled() {
                report.push_str(&format!("\n{}", Backtrace::force_capture()));
            } else {
                report.push_str("\nset RUST_BACKTRACE=1 to capture a backtrace");
            }
            let recent = ring::global().dump(RECENT_LINES);
            report.push_str(&format!(
                "\n--- rdc-diag recent logs ({}) ---",
                recent.len()
            ));
            for line in recent {
                report.push('\n');
                report.push_str(&line);
            }
            report.push_str("\n--- end rdc-diag panic dump ---");
            record::write_stderr(&report);
            // 保留原有 hook：std 会继续打印默认 panic 信息，行为不变。
            previous(info);
        }));
    });
}

// =====** 内部工具 =====**

/// `RUST_BACKTRACE` 非 `0` 时捕获回溯。
fn backtrace_enabled() -> bool {
    match std::env::var("RUST_BACKTRACE") {
        Ok(value) => value != "0",
        Err(_) => false,
    }
}

/// 提取 panic 载荷文本；非字符串载荷给出明确标记。
///
/// 只接收 `&dyn Any`，避免直接命名 `PanicHookInfo`（该类型 1.81 才稳定，而 workspace MSRV 是 1.80）。
fn payload_text(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(value) = payload.downcast_ref::<&str>() {
        return value.to_string();
    }
    if let Some(value) = payload.downcast_ref::<String>() {
        return value.clone();
    }
    "<non-string panic payload>".to_string()
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_is_idempotent() {
        // 连续安装两次不应 panic，也不应叠加多个 hook。
        install();
        install();
    }
}
