//! rdc-cli · 命令行自检与验证工具
//!
//! 层级：L4 验证入口（无 UI）
//! 用途：`doctor` 环境自检、`selftest` 冒烟、`demo` S2 本机回环验证；
//!       `host`/`connect`/`bench` 后续阶段使用。
//! 调用方：开发者、CI、打包产物的用户自助诊断（docs/08 §5 产物附加要求）

use std::error::Error;
use std::process::ExitCode;
use std::time::Duration;

// =====** 模块声明 =====**

mod demo;
// S3a（10s 1080p60 采集+MFT 编码+MP4 回读）与 S4 输入/按键演示依赖 Media Foundation
// 与 Win32 光标/低级键盘钩子，属于 Windows 专属验证路径；其它平台在下方给出明确提示。
#[cfg(target_os = "windows")]
mod s3a;
mod s3b;
mod s3b3;
mod s3b4;
#[cfg(target_os = "windows")]
mod s4demo;
mod s5_5;
mod walldemo;

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
    // 先装日志守卫：各子命令与底层 crate 都走统一的 tracing 通道，缺 subscriber 会导致日志静默丢失。
    let _diag = rdc_diag::init("rdc-cli");
    // 只做最小手工解析：对象是少量固定子命令，不引入 clap。
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("doctor") | None => run_doctor(),
        Some("selftest") => run_selftest(),
        Some("demo") => run_demo().await,
        // 夹具窗口是可选的一次性测试件，只有显式 `--fixture` 才创建。
        Some("s3a") => run_s3a(args.any(|arg| arg == "--fixture")),
        Some("s3b") => run_s3b(&mut args),
        Some("s3b3") => run_s3b3(&mut args),
        Some("s3b4") => run_s3b4(&mut args),
        Some("demo-input") => {
            #[cfg(target_os = "windows")]
            {
                run_s4_demo("demo-input", 60, s4demo::run_input()).await
            }
            #[cfg(not(target_os = "windows"))]
            {
                unsupported_demo("demo-input")
            }
        }
        Some("demo-keys") => {
            #[cfg(target_os = "windows")]
            {
                run_s4_demo("demo-keys", 60, s4demo::run_keys()).await
            }
            #[cfg(not(target_os = "windows"))]
            {
                unsupported_demo("demo-keys")
            }
        }
        Some("demo-clipboard") => {
            #[cfg(target_os = "windows")]
            {
                run_s4_demo("demo-clipboard", 60, s4demo::run_clipboard()).await
            }
            #[cfg(not(target_os = "windows"))]
            {
                unsupported_demo("demo-clipboard")
            }
        }
        Some("demo-file") => {
            #[cfg(target_os = "windows")]
            {
                run_s4_demo("demo-file", 120, s4demo::run_file()).await
            }
            #[cfg(not(target_os = "windows"))]
            {
                unsupported_demo("demo-file")
            }
        }
        Some("demo-wall") => run_s4_demo("demo-wall", 90, walldemo::run()).await,
        Some("s5-5") => run_s5_5(),
        Some(other) => {
            eprintln!("未知子命令: {other}");
            eprintln!(
                "可用: doctor | selftest | demo | demo-input | demo-keys | demo-clipboard | \
                 demo-file | demo-wall | s5-5 | s3a [--fixture] | s3b [--frames N] | \
                 s3b3 [--instances K] [--frames N] [--bitrate B] | \
                 s3b4 [host|ctrl] [--secs S]"
            );
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
    println!(
        "core={} platform={}",
        rdc_core::core_version(),
        rdc_core::platform_name()
    );
    println!("protocol={}", rdc_protocol::version_string());
    ExitCode::SUCCESS
}

/// S3a 采集与编码端到端实测：10 秒 1080p60，需真实桌面。
///
/// # Arguments
/// * `fixture` - 是否创建全屏动态夹具窗口（仅显式 `--fixture` 时为 true）。
///
/// # Returns
/// 实测通过返回 SUCCESS；失败或未达门槛返回 FAILURE。
///
/// # Errors
/// 不返回错误；通过退出码与 stderr 表达。
#[cfg(target_os = "windows")]
fn run_s3a(fixture: bool) -> ExitCode {
    match s3a::run(fixture) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("S3a 失败: {err}");
            ExitCode::FAILURE
        }
    }
}

/// 非 Windows 平台的 S3a 占位：编码路径依赖 Media Foundation，明确失败而非伪造通过。
#[cfg(not(target_os = "windows"))]
fn run_s3a(_fixture: bool) -> ExitCode {
    eprintln!("S3a 实测（MFT 硬件编码 + MP4 回读）目前仅支持 Windows");
    ExitCode::FAILURE
}

/// 非 Windows 平台的 S4 演示占位：演示断言依赖 Win32 光标/键盘钩子。
///
/// # Arguments
/// * `name` - 演示名。
///
/// # Returns
/// 固定返回 FAILURE。
///
/// # Errors
/// 不会失败。
#[cfg(not(target_os = "windows"))]
fn unsupported_demo(name: &str) -> ExitCode {
    eprintln!("{name} 演示（Win32 光标/键盘钩子探针）目前仅支持 Windows");
    ExitCode::FAILURE
}

/// S3b-4 端到端实测：本机两进程 UDP 回环，画内时间码回读求玻璃到玻璃延迟。
///
/// # Arguments
/// * `args` - `s3b4` 之后的参数（`host` / `ctrl` 或驱动参数 `--secs` / `--out`）。
///
/// # Returns
/// 全链路成功返回 SUCCESS，否则 FAILURE。
///
/// # Errors
/// 不返回错误；通过退出码与 stderr 表达。
fn run_s3b4(args: &mut impl Iterator<Item = String>) -> ExitCode {
    match s3b4::run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("S3b-4 失败: {err}");
            ExitCode::FAILURE
        }
    }
}

/// S3b-3 并行编码实测：K 个独立 MF 会话，测 push 成本与码率偏差。
///
/// # Arguments
/// * `args` - `s3b3` 之后的参数。
///
/// # Returns
/// 判据满足返回 SUCCESS，否则 FAILURE。
///
/// # Errors
/// 不返回错误；通过退出码与 stderr 表达。
fn run_s3b3(args: &mut impl Iterator<Item = String>) -> ExitCode {
    match s3b3::run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("S3b-3 失败: {err}");
            ExitCode::FAILURE
        }
    }
}

/// S3b-2 解码与上屏自检：合成码流 → MF 解码 → D3D11 窗口呈现。
///
/// # Arguments
/// * `args` - `s3b` 之后的参数（支持 `--frames N`）。
///
/// # Returns
/// 全部判据满足返回 SUCCESS，否则 FAILURE。
///
/// # Errors
/// 不返回错误；通过退出码与 stderr 表达。
fn run_s3b(args: &mut impl Iterator<Item = String>) -> ExitCode {
    match s3b::run(args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("S3b 失败: {err}");
            ExitCode::FAILURE
        }
    }
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

/// S5-5 可判定验证：授权链路、接管、踢出、免密。
///
/// # Returns
/// 全部判据通过返回 SUCCESS，否则 FAILURE。
///
/// # Errors
/// 不返回错误；通过退出码与 stderr 表达。
fn run_s5_5() -> ExitCode {
    match s5_5::run() {
        Ok(all_pass) => {
            if all_pass {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(err) => {
            eprintln!("S5-5 验证失败: {err}");
            ExitCode::FAILURE
        }
    }
}

/// S4 演示统一入口：带总超时，超时按失败处理。
///
/// # Arguments
/// * `name` - 演示名（用于日志）
/// * `timeout_secs` - 总超时秒数
/// * `task` - 演示 future
///
/// # Returns
/// 演示成功返回 SUCCESS，失败或超时返回 FAILURE。
///
/// # Errors
/// 不返回错误；通过退出码与 stderr 表达。
async fn run_s4_demo<F>(name: &str, timeout_secs: u64, task: F) -> ExitCode
where
    F: std::future::Future<Output = Result<(), Box<dyn Error + Send + Sync>>>,
{
    match tokio::time::timeout(Duration::from_secs(timeout_secs), task).await {
        Ok(Ok(())) => ExitCode::SUCCESS,
        Ok(Err(err)) => {
            eprintln!("{name} 失败: {err}");
            ExitCode::FAILURE
        }
        Err(_) => {
            eprintln!("{name} 失败: 超过 {timeout_secs} s 未完成");
            ExitCode::FAILURE
        }
    }
}
