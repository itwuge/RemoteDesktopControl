//! rdc-cli · 命令行自检与验证工具
//!
//! 层级：L4 验证入口（无 UI）
//! 用途：`doctor` 环境自检、`selftest` 冒烟、`demo` S2 本机回环验证；
//!       `host`/`connect`/`bench` 后续阶段使用。
//! 调用方：开发者、CI、打包产物的用户自助诊断（docs/08 §5 产物附加要求）

use std::process::ExitCode;
use std::time::Duration;

// =====** 模块声明 =====**

mod demo;

// =====** 入口 =====**

/// 进程入口：解析子命令并执行。
///
/// # Returns
/// 成功返回 `ExitCode::SUCCESS`，失败或未知子命令返回 `ExitCode::FAILURE`。
///
/// # Errors
/// 本函数不返回 `Result`；失败通过退出码表达，便于脚本判断。
#[tokio::main]
async fn main() -> ExitCode {
    // 只做最小手工解析：对象是少量固定子命令，不引入 clap。
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("doctor") | None => run_doctor(),
        Some("selftest") => run_selftest(),
        Some("demo") => run_demo().await,
        Some(other) => {
            eprintln!("未知子命令: {other}");
            eprintln!("可用: doctor | selftest | demo");
            ExitCode::FAILURE
        }
    }
}

// =====** 子命令实现 =====**

/// 打印 5 行环境自检结果。
///
/// # Returns
/// 全部通过返回 SUCCESS，任一失败返回 FAILURE。
///
/// # Errors
/// 不返回错误；通过退出码表达。
fn run_doctor() -> ExitCode {
    let checks = rdc_core::run_doctor();
    for check in &checks {
        println!("{}", check.line());
    }
    if checks.iter().all(|c| c.ok) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// 冒烟测试：打印核心版本与协议版本。
///
/// # Returns
/// 固定返回 SUCCESS。
///
/// # Errors
/// 不会失败。
fn run_selftest() -> ExitCode {
    println!("core={} platform={}", rdc_core::core_version(), rdc_core::platform_name());
    println!("protocol={}", rdc_protocol::version_string());
    ExitCode::SUCCESS
}

/// S2 本机回环 Demo：30 s 上限，超时按失败处理。
///
/// # Returns
/// Demo 成功返回 SUCCESS，失败或超时返回 FAILURE。
///
/// # Errors
/// 不返回错误；通过退出码与 stderr 表达。
async fn run_demo() -> ExitCode {
    match tokio::time::timeout(Duration::from_secs(30), demo::run()).await {
        Ok(Ok(())) => ExitCode::SUCCESS,
        Ok(Err(err)) => {
            eprintln!("Demo 失败: {err}");
            ExitCode::FAILURE
        }
        Err(_) => {
            eprintln!("Demo 失败: 超过 30 s 未完成（可能是某一步阻塞）");
            ExitCode::FAILURE
        }
    }
}
