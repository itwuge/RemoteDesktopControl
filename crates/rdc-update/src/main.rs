//! rdc-updater / 命令行入口
//!
//! 层级：L2 能力层（可执行程序）
//! 用途：把自更新能力暴露为稳定的命令行协议，供 Flutter 通过 `Process.run` 调用，
//!       避免把网络/验签逻辑放进 UI 进程；同时供发布流水线签名清单使用。
//! 输出约定：stdout 只输出单行 JSON，stderr 输出人读信息；退出码 0 表示成功。
//!
//! 子命令：
//! - `version`                                 打印当前版本
//! - `check --api URL [--current V]`           检查更新
//! - `download --api URL --version V`          下载并校验
//! - `apply --install-dir DIR --installer P --version V --sha256 H [--current V]`
//! - `restore --backup DIR --install-dir DIR`  手动/提权恢复
//! - `sign --file PATH [--seed-b64 S]`         发布流水线签名清单
//! - `genkey`                                  生成发布密钥对（仅首次使用）

use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_json::{json, Value};

use rdc_update::apply::{apply_update, restore_backup, ApplyOutcome};
use rdc_update::error::{codes, Result, UpdateError};
use rdc_update::manifest;
use rdc_update::stage::{self, StagedUpdate};
use rdc_update::{current_version, resolve_api_url, resolve_public_key};

/// 程序入口。
///
/// # Returns
/// 无返回值；退出码 0=成功、1=参数/执行失败、2=升级未成功（已回滚或未应用）。
///
/// # Errors
/// 不会 panic（参数缺失统一走错误输出）。
fn main() {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let Some(command) = arguments.first().cloned() else {
        print_error(&UpdateError::new(
            codes::ENV_MISSING,
            "缺少子命令（version/check/download/apply/restore/sign/genkey）",
        ));
        std::process::exit(1);
    };
    let rest = &arguments[1..];

    match dispatch(&command, rest) {
        Ok((value, exit_code)) => {
            println!("{}", value);
            std::process::exit(exit_code);
        }
        Err(error) => {
            print_error(&error);
            std::process::exit(1);
        }
    }
}

/// 派发子命令。
///
/// # Arguments
/// * `command` - 子命令名
/// * `args` - 子命令参数
///
/// # Returns
/// `(JSON 结果, 退出码)`。
///
/// # Errors
/// 参数缺失、网络、校验、升级失败时返回 [`UpdateError`]。
fn dispatch(command: &str, args: &[String]) -> Result<(Value, i32)> {
    match command {
        "version" => Ok((
            json!({"ok": true, "version": current_version()}),
            0,
        )),
        "check" => {
            let api = resolve_api_url(option(args, "--api").as_deref());
            let key = resolve_public_key(option(args, "--public-key").as_deref());
            let current =
                option(args, "--current").unwrap_or_else(|| current_version().to_owned());
            let token = resolve_token(args);
            let check = stage::check(&api, token.as_deref(), &current, &key)?;
            Ok((json!({"ok": true, "data": check}), 0))
        }
        "download" => {
            let api = resolve_api_url(option(args, "--api").as_deref());
            let key = resolve_public_key(option(args, "--public-key").as_deref());
            let version = required(args, "--version")?;
            let token = resolve_token(args);
            let staged =
                stage::download(&api, token.as_deref(), &key, &version)?;
            Ok((json!({"ok": true, "data": staged}), 0))
        }
        "apply" => {
            let install_dir = PathBuf::from(required(args, "--install-dir")?);
            let installer = required(args, "--installer")?;
            let version = required(args, "--version")?;
            let sha256 = required(args, "--sha256")?;
            let old_version =
                option(args, "--current").unwrap_or_else(|| current_version().to_owned());
            // 由正在运行的 GUI 拉起时，先等它退出：Windows 不允许覆盖运行中的 exe。
            if let Some(pid_text) = option(args, "--wait-pid") {
                let pid: u32 = pid_text.parse().map_err(|_| {
                    UpdateError::new(
                        codes::ENV_MISSING,
                        format!("--wait-pid 不是合法进程号：{pid_text}"),
                    )
                })?;
                rdc_update::apply::wait_for_pid_exit(
                    pid,
                    std::time::Duration::from_secs(180),
                )?;
            }
            let size = std::fs::metadata(&installer)
                .map(|meta| meta.len())
                .unwrap_or(0);
            let staged = StagedUpdate {
                version,
                installer_path: installer.clone(),
                sha256,
                size,
                staging_dir: Path::new(&installer)
                    .parent()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
            };
            let outcome: ApplyOutcome =
                apply_update(&install_dir, &staged, &old_version);
            // 后台安装没有 UI，把结论落盘，下一次启动可读取展示。
            let result_path = stage::staging_root().join("last-apply.json");
            if let Ok(json) = serde_json::to_vec_pretty(&outcome) {
                let _ = std::fs::write(&result_path, json);
            }
            let exit = if outcome.applied { 0 } else { 2 };
            Ok((json!({"ok": outcome.applied, "data": outcome}), exit))
        }
        "restore" => {
            let backup = PathBuf::from(required(args, "--backup")?);
            let install_dir = PathBuf::from(required(args, "--install-dir")?);
            let stats = restore_backup(&backup, &install_dir)?;
            Ok((json!({"ok": true, "data": stats}), 0))
        }
        "sign" => {
            let file = PathBuf::from(required(args, "--file")?);
            let seed_b64 = option(args, "--seed-b64")
                .or_else(|| std::env::var("RDC_RELEASE_SEED_B64").ok())
                .ok_or_else(|| {
                    UpdateError::new(
                        codes::ENV_MISSING,
                        "缺少签名种子：--seed-b64 或环境变量 RDC_RELEASE_SEED_B64",
                    )
                })?;
            let seed_bytes = STANDARD.decode(seed_b64.trim()).map_err(|error| {
                UpdateError::new(
                    codes::SIG_INVALID,
                    format!("签名种子不是合法 Base64：{error}"),
                )
            })?;
            let seed: [u8; 32] = seed_bytes.as_slice().try_into().map_err(|_| {
                UpdateError::new(
                    codes::SIG_INVALID,
                    "签名种子必须是 32 字节（Base64 解码后）",
                )
            })?;
            let bytes = std::fs::read(&file)?;
            let signature = manifest::sign_bytes(&bytes, &seed);
            let identity = rdc_security::IdentityKeyPair::from_seed(&seed);
            let sig_path = PathBuf::from(format!("{}.sig", file.display()));
            std::fs::write(&sig_path, &signature)?;
            Ok((
                json!({
                    "ok": true,
                    "signature_path": sig_path.display().to_string(),
                    "public_key_b64": identity.public_key_b64(),
                }),
                0,
            ))
        }
        "genkey" => {
            let identity = rdc_security::IdentityKeyPair::generate();
            Ok((
                json!({
                    "ok": true,
                    "public_key_b64": identity.public_key_b64(),
                    "seed_b64": STANDARD.encode(identity.to_seed_bytes()),
                    "warning": "私钥种子只能写入 CI Secret 与离线备份，禁止提交到仓库或日志",
                }),
                0,
            ))
        }
        other => Err(UpdateError::new(
            codes::ENV_MISSING,
            format!("未知子命令：{other}"),
        )),
    }
}

/// 读取 `--name value` 形式的参数。
///
/// # Arguments
/// * `args` - 参数切片
/// * `name` - 参数名
///
/// # Returns
/// 参数值。
///
/// # Errors
/// 不会失败。
fn option(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|value| value == name)
        .and_then(|index| args.get(index + 1))
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// 读取必填参数。
///
/// # Arguments
/// * `args` - 参数切片
/// * `name` - 参数名
///
/// # Returns
/// 参数值。
///
/// # Errors
/// 缺失时返回 [`codes::ENV_MISSING`]。
fn required(args: &[String], name: &str) -> Result<String> {
    option(args, name).ok_or_else(|| {
        UpdateError::new(codes::ENV_MISSING, format!("缺少必填参数 {name}"))
    })
}

/// 解析 GitHub Token（命令行优先，其次环境变量）。
///
/// # Arguments
/// * `args` - 参数切片
///
/// # Returns
/// Token（可能为空表示匿名访问）。
///
/// # Errors
/// 不会失败。
fn resolve_token(args: &[String]) -> Option<String> {
    option(args, "--token")
        .or_else(|| std::env::var("RDC_GITHUB_TOKEN").ok())
        .or_else(|| std::env::var("GITHUB_TOKEN").ok())
        .filter(|value| !value.trim().is_empty())
}

/// 输出错误 JSON（stdout 保持单行，stderr 留人读信息）。
///
/// # Arguments
/// * `error` - 错误
///
/// # Returns
/// 无返回值。
///
/// # Errors
/// 不会失败。
fn print_error(error: &UpdateError) {
    eprintln!("[rdc-updater] {error}");
    println!(
        "{}",
        json!({
            "ok": false,
            "code": error.code,
            "message": error.message,
        })
    );
}
