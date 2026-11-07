//! rdc-update / 升级执行与失败回滚
//!
//! 层级：L2 能力层
//! 用途：升级前备份当前程序目录 → 静默启动安装器 → 校验安装后版本 →
//!       失败时用备份恢复旧版本（必要时以管理员身份启动助手）。
//! 调用方：rdc-updater CLI（apply / restore 子命令）。
//!
//! 关键约束：
//! - 配置目录 `config` 永不进入备份、永不被恢复覆盖（用户设置有更高优先级）；
//! - 备份目录放在 `%LOCALAPPDATA%\RDC\updates`，普通用户可写，无需提权；
//! - 恢复失败必须返回 `RDC-UPD-ROLLBACK-001`，并保留备份路径供人工处理。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::error::{codes, Result, UpdateError};
use crate::stage::{self, StagedUpdate};

/// 升级成功码（失败码见 [`codes`]）。
pub const CODE_APPLY_OK: &str = "RDC-UPD-APPLY-000";

/// 升级结果（可直接序列化给 UI）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApplyOutcome {
    /// 是否已成功升级到目标版本。
    pub applied: bool,
    /// 是否执行过回滚。
    pub rolled_back: bool,
    /// 结果码。
    pub code: String,
    /// 中文说明。
    pub message: String,
    /// 备份目录（未备份时为空）。
    pub backup_dir: String,
    /// 升级前版本。
    pub old_version: String,
    /// 目标版本。
    pub new_version: String,
    /// 安装器退出码（未启动时为 -1）。
    pub installer_exit_code: i32,
}

/// 恢复统计。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestoreStats {
    /// 从备份复制的文件数。
    pub restored: usize,
    /// 从安装目录删除的新版本文件数。
    pub deleted: usize,
}

/// 读取 `BUILD_INFO.txt` 中的版本号。
///
/// # Arguments
/// * `install_dir` - 安装目录
///
/// # Returns
/// 找到 `version=` 行时返回版本字符串。
///
/// # Errors
/// 不会失败：文件缺失或没有该行时返回 `None`。
pub fn read_build_version(install_dir: &Path) -> Option<String> {
    let text = fs::read_to_string(install_dir.join("BUILD_INFO.txt")).ok()?;
    for line in text.lines() {
        if let Some(value) = line.trim().strip_prefix("version=") {
            let value = value.trim();
            if !value.is_empty() {
                return Some(value.to_owned());
            }
        }
    }
    None
}

/// 等待指定进程退出（应用必须先退出，安装器才能覆盖正在运行的 exe）。
///
/// # Arguments
/// * `pid` - 应用进程 ID
/// * `timeout` - 最长等待时间
///
/// # Returns
/// 进程已退出或本来就不存在时返回 `Ok(())`。
///
/// # Errors
/// 等待超时返回 [`codes::APPLY_LAUNCH`]，避免在有文件锁的情况下硬装。
pub fn wait_for_pid_exit(pid: u32, timeout: std::time::Duration) -> Result<()> {
    #[cfg(windows)]
    {
        use windows::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
        use windows::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};

        // 进程已退出时 OpenProcess 会失败；这属于“已经满足条件”，不能当错误。
        let handle = match unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) } {
            Ok(handle) => handle,
            Err(_) => return Ok(()),
        };
        let wait = unsafe {
            WaitForSingleObject(handle, timeout.as_millis().min(u32::MAX as u128) as u32)
        };
        let _ = unsafe { CloseHandle(handle) };
        if wait == WAIT_OBJECT_0 {
            Ok(())
        } else {
            Err(UpdateError::new(
                codes::APPLY_LAUNCH,
                format!("等待应用退出超时（{timeout:?}），已取消安装"),
            ))
        }
    }
    #[cfg(not(windows))]
    {
        let _ = (pid, timeout);
        Ok(())
    }
}

/// 判断目录是否可直接写入（决定是否需要提权）。
///
/// # Arguments
/// * `directory` - 目标目录
///
/// # Returns
/// 可写返回 `true`。
///
/// # Errors
/// 不会失败。
pub fn install_dir_writable(directory: &Path) -> bool {
    if !directory.is_dir() {
        return false;
    }
    let probe = directory.join(format!(".rdc-update-probe-{}", std::process::id()));
    match fs::write(&probe, b"ok") {
        Ok(()) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// 备份安装目录（排除 `config`）。
///
/// # Arguments
/// * `install_dir` - 安装目录
/// * `version` - 当前版本（用于目录命名）
///
/// # Returns
/// 备份目录路径。
///
/// # Errors
/// 安装目录不存在或复制失败时返回 [`codes::IO`]。
pub fn backup_install_dir(install_dir: &Path, version: &str) -> Result<PathBuf> {
    backup_install_dir_to(install_dir, &stage::staging_root(), version)
}

/// 备份到指定根目录（测试注入用）。
///
/// # Arguments
/// * `install_dir` - 安装目录
/// * `root` - 备份根目录
/// * `version` - 当前版本
///
/// # Returns
/// 备份目录路径。
///
/// # Errors
/// 同 [`backup_install_dir`]。
pub fn backup_install_dir_to(
    install_dir: &Path,
    root: &Path,
    version: &str,
) -> Result<PathBuf> {
    if !install_dir.is_dir() {
        return Err(UpdateError::new(
            codes::IO,
            format!("安装目录不存在：{}", install_dir.display()),
        ));
    }
    let target = root.join(format!(
        "backup-{}-{}",
        version,
        stage::unix_seconds()
    ));
    let _ = fs::remove_dir_all(&target);
    copy_tree(install_dir, &target, true)?;
    Ok(target)
}

/// 用备份恢复安装目录（排除 `config`，并删除备份中不存在的新文件）。
///
/// # Arguments
/// * `backup` - 备份目录
/// * `install_dir` - 安装目录
///
/// # Returns
/// 恢复/删除的文件数。
///
/// # Errors
/// 备份不存在或复制失败时返回 [`codes::IO`]；调用方应映射为
/// [`codes::ROLLBACK_FAILED`] 并提示备份路径。
pub fn restore_backup(backup: &Path, install_dir: &Path) -> Result<RestoreStats> {
    if !backup.is_dir() {
        return Err(UpdateError::new(
            codes::IO,
            format!("备份目录不存在：{}", backup.display()),
        ));
    }
    fs::create_dir_all(install_dir)?;
    let mut restored = 0usize;
    let mut deleted = 0usize;

    let backup_files = collect_relative_files(backup, false)?;
    let backup_set: std::collections::HashSet<PathBuf> =
        backup_files.iter().cloned().collect();

    // 1. 删除新版本带入、但旧版本没有的文件（否则会出现“新旧混装”）。
    for relative in collect_relative_files(install_dir, true)? {
        if !backup_set.contains(&relative) {
            let path = install_dir.join(&relative);
            if fs::remove_file(&path).is_ok() {
                deleted += 1;
            }
        }
    }

    // 2. 复制备份文件覆盖回安装目录。
    for relative in &backup_files {
        let destination = install_dir.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(backup.join(relative), &destination)?;
        restored += 1;
    }

    Ok(RestoreStats { restored, deleted })
}

/// 执行升级（备份 → 安装 → 校验 → 必要时回滚）。
///
/// # Arguments
/// * `install_dir` - 安装目录（通常为 exe 所在目录）
/// * `staged` - 已校验的暂存更新
/// * `old_version` - 升级前版本
///
/// # Returns
/// 升级结果；失败时 `rolled_back` 表示是否已恢复旧版本。
///
/// # Errors
/// 不会返回错误：所有失败都写入 [`ApplyOutcome`]，保证 UI 能展示完整结论。
pub fn apply_update(
    install_dir: &Path,
    staged: &StagedUpdate,
    old_version: &str,
) -> ApplyOutcome {
    let mut outcome = ApplyOutcome {
        applied: false,
        rolled_back: false,
        code: codes::IO.to_owned(),
        message: String::new(),
        backup_dir: String::new(),
        old_version: old_version.to_owned(),
        new_version: staged.version.clone(),
        installer_exit_code: -1,
    };

    // 升级前复检：暂存目录可能被替换，必须在执行前再算一次哈希。
    if let Err(error) = stage::verify_staged(staged) {
        outcome.code = error.code.to_owned();
        outcome.message = error.message;
        return outcome;
    }

    let backup = match backup_install_dir(install_dir, old_version) {
        Ok(path) => path,
        Err(error) => {
            outcome.code = error.code.to_owned();
            outcome.message = format!("升级前备份失败，已取消安装：{}", error.message);
            return outcome;
        }
    };
    outcome.backup_dir = backup.display().to_string();

    let installer = Path::new(&staged.installer_path);
    match run_installer(installer, install_dir) {
        Ok(code) => outcome.installer_exit_code = code,
        Err(error) => {
            // 安装器没起来（例如用户取消 UAC）：程序文件未被改动，无需回滚。
            outcome.code = error.code.to_owned();
            outcome.message = format!(
                "{}；备份保留在 {}",
                error.message,
                backup.display()
            );
            return outcome;
        }
    }

    // 以 BUILD_INFO.txt 为版本真源：安装器写完它才算真正升级成功。
    let installed = read_build_version(install_dir);
    let version_ok = installed
        .as_deref()
        .map(|value| value.trim() == staged.version.trim())
        .unwrap_or(false);

    if version_ok {
        outcome.applied = true;
        outcome.code = CODE_APPLY_OK.to_owned();
        outcome.message = format!(
            "已升级到 {}（安装器退出码 {}）",
            staged.version, outcome.installer_exit_code
        );
        // 升级成功后清理同版本之外的旧暂存，避免磁盘越滚越大。
        let _ = stage::cleanup_staging(&staged.version);
        return outcome;
    }

    outcome.code = if outcome.installer_exit_code != 0 {
        codes::APPLY_EXIT.to_owned()
    } else {
        codes::APPLY_VERSION.to_owned()
    };
    outcome.message = format!(
        "安装后版本校验失败（期望 {}，实际 {}，安装器退出码 {}），开始回滚",
        staged.version,
        installed.unwrap_or_else(|| "未知".to_owned()),
        outcome.installer_exit_code
    );

    match rollback(install_dir, &backup) {
        Ok(()) => {
            outcome.rolled_back = true;
            outcome.message.push_str("；已恢复到旧版本");
        }
        Err(error) => {
            outcome.code = codes::ROLLBACK_FAILED.to_owned();
            outcome.message.push_str(&format!(
                "；回滚失败（{}），请用备份目录手动恢复：{}",
                error.message,
                backup.display()
            ));
        }
    }
    outcome
}

/// 回滚：可写目录直接恢复，Program Files 等只读目录走提权助手。
///
/// # Arguments
/// * `install_dir` - 安装目录
/// * `backup` - 备份目录
///
/// # Returns
/// 成功返回 `Ok(())`。
///
/// # Errors
/// 恢复失败时返回 [`codes::ROLLBACK_FAILED`]。
fn rollback(install_dir: &Path, backup: &Path) -> Result<()> {
    if install_dir_writable(install_dir) {
        restore_backup(backup, install_dir)
            .map(|_| ())
            .map_err(|error| {
                UpdateError::new(
                    codes::ROLLBACK_FAILED,
                    format!("恢复备份失败：{}", error.message),
                )
            })
    } else {
        run_helper_elevated(backup, install_dir)
    }
}

// =====** 进程启动 =====**

/// 启动安装器并等待退出。
///
/// # Arguments
/// * `installer` - 安装包路径
/// * `install_dir` - 安装目录（决定是否需要提权）
///
/// # Returns
/// 安装器退出码。
///
/// # Errors
/// 提权被取消、进程无法启动时返回 [`codes::APPLY_LAUNCH`]。
fn run_installer(installer: &Path, install_dir: &Path) -> Result<i32> {
    #[cfg(windows)]
    {
        // 以「安装目标目录是否可写」判断提权，而不是安装包所在目录：
        // 安装包总在用户可写的暂存目录里，按它判断会漏掉 Program Files 需要提权的情况。
        if install_dir_writable(install_dir) {
            let status = Command::new(installer)
                .arg("/S")
                .status()
                .map_err(|error| {
                    UpdateError::new(
                        codes::APPLY_LAUNCH,
                        format!("启动安装器失败：{error}"),
                    )
                })?;
            return Ok(status.code().unwrap_or(-1));
        }
        shell_execute_and_wait(installer, "/S")
    }
    #[cfg(not(windows))]
    {
        let _ = (installer, install_dir);
        Err(UpdateError::new(
            codes::APPLY_UNSUPPORTED,
            "当前平台不支持自动安装（仅 Windows 提供安装包升级）",
        ))
    }
}

/// 以管理员身份启动 `rdc-updater restore` 助手。
///
/// # Arguments
/// * `backup` - 备份目录
/// * `install_dir` - 安装目录
///
/// # Returns
/// 助手成功返回 `Ok(())`。
///
/// # Errors
/// 提权被取消或助手退出码非 0 时返回 [`codes::ROLLBACK_FAILED`]。
fn run_helper_elevated(backup: &Path, install_dir: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        let helper = std::env::current_exe().map_err(|error| {
            UpdateError::new(
                codes::ROLLBACK_FAILED,
                format!("无法定位回滚助手：{error}"),
            )
        })?;
        let parameters = format!(
            "restore --backup \"{}\" --install-dir \"{}\"",
            backup.display(),
            install_dir.display()
        );
        let code = shell_execute_and_wait(&helper, &parameters).map_err(|error| {
            UpdateError::new(
                codes::ROLLBACK_FAILED,
                format!("提权回滚失败：{}", error.message),
            )
        })?;
        if code != 0 {
            return Err(UpdateError::new(
                codes::ROLLBACK_FAILED,
                format!("回滚助手退出码 {code}"),
            ));
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = (backup, install_dir);
        Err(UpdateError::new(
            codes::ROLLBACK_FAILED,
            "当前平台不支持提权回滚",
        ))
    }
}

/// 通过 ShellExecuteExW 以 `runas` 启动进程并等待退出。
///
/// # Arguments
/// * `file` - 可执行文件
/// * `parameters` - 命令行参数
///
/// # Returns
/// 进程退出码。
///
/// # Errors
/// UAC 被取消（错误码 1223）或进程无法创建时返回 [`codes::APPLY_LAUNCH`]。
#[cfg(windows)]
fn shell_execute_and_wait(file: &Path, parameters: &str) -> Result<i32> {
    use std::os::windows::ffi::OsStrExt;

    use windows::core::PCWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{
        GetExitCodeProcess, WaitForSingleObject, INFINITE,
    };
    use windows::Win32::UI::Shell::{
        ShellExecuteExW, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW,
    };

    // UTF-16 + NUL：Win32 只接受宽字符，必须补终止符。
    let verb: Vec<u16> = "runas\0".encode_utf16().collect();
    let file_w: Vec<u16> = file
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let params_w: Vec<u16> = parameters
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();

    let mut info = SHELLEXECUTEINFOW::default();
    info.cbSize = std::mem::size_of::<SHELLEXECUTEINFOW>() as u32;
    info.fMask = SEE_MASK_NOCLOSEPROCESS;
    info.lpVerb = PCWSTR(verb.as_ptr());
    info.lpFile = PCWSTR(file_w.as_ptr());
    info.lpParameters = PCWSTR(params_w.as_ptr());
    // SW_SHOWNORMAL = 1：安装器为静默模式，窗口不会弹出。
    info.nShow = 1;

    unsafe {
        ShellExecuteExW(&mut info).map_err(|error| {
            UpdateError::new(
                codes::APPLY_LAUNCH,
                format!("提权启动失败（错误码 {}）：{error}", error.code().0),
            )
        })?;
        if info.hProcess.is_invalid() {
            return Err(UpdateError::new(
                codes::APPLY_LAUNCH,
                "提权启动未返回进程句柄",
            ));
        }
        // 必须等安装器结束：否则会读到旧版本 BUILD_INFO 触发误回滚。
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code: u32 = 0;
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        Ok(code as i32)
    }
}

// =====** 文件树工具 =====**

/// 递归复制目录。
///
/// # Arguments
/// * `source` - 源目录
/// * `destination` - 目标目录
/// * `skip_config` - 是否跳过根目录下的 `config`
///
/// # Returns
/// 成功返回 `Ok(())`。
///
/// # Errors
/// 读写失败时返回 [`io::Error`]。
fn copy_tree(source: &Path, destination: &Path, skip_config: bool) -> io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let name = entry.file_name();
        if skip_config && name.eq_ignore_ascii_case("config") {
            continue;
        }
        let from = entry.path();
        let to = destination.join(&name);
        if from.is_dir() {
            copy_tree(&from, &to, false)?;
        } else {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

/// 收集目录下文件相对路径。
///
/// # Arguments
/// * `root` - 根目录
/// * `skip_config` - 是否跳过根目录下的 `config`
///
/// # Returns
/// 相对路径列表（已排序）。
///
/// # Errors
/// 读取目录失败时返回 [`io::Error`]。
fn collect_relative_files(root: &Path, skip_config: bool) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    collect_into(root, root, skip_config, &mut out)?;
    out.sort();
    Ok(out)
}

/// 递归收集实现。
///
/// # Arguments
/// * `root` - 根目录
/// * `current` - 当前目录
/// * `skip_config` - 是否跳过根目录下的 `config`
/// * `out` - 输出
///
/// # Returns
/// 成功返回 `Ok(())`。
///
/// # Errors
/// 读取目录失败时返回错误。
fn collect_into(
    root: &Path,
    current: &Path,
    skip_config: bool,
    out: &mut Vec<PathBuf>,
) -> io::Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let name = entry.file_name();
        if skip_config
            && current == root
            && name.eq_ignore_ascii_case("config")
        {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            collect_into(root, &path, skip_config, out)?;
        } else if let Ok(relative) = path.strip_prefix(root) {
            out.push(relative.to_path_buf());
        }
    }
    Ok(())
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造测试用安装目录。
    fn fake_install(root: &Path) -> PathBuf {
        let install = root.join("install");
        fs::create_dir_all(install.join("data")).expect("创建安装目录");
        fs::write(install.join("rdc.exe"), b"old-exe").expect("写 exe");
        fs::write(install.join("BUILD_INFO.txt"), b"version=0.1.1\ncommit=abc\n")
            .expect("写版本");
        fs::write(install.join("data").join("app.so"), b"old-data").expect("写数据");
        fs::create_dir_all(install.join("config")).expect("创建配置目录");
        fs::write(install.join("config").join("settings.json"), b"{\"keep\":true}")
            .expect("写设置");
        install
    }

    /// 备份必须排除 config；恢复必须删掉新版本文件并保留 config。
    #[test]
    fn backup_and_restore_round_trip() {
        let root = std::env::temp_dir().join(format!(
            "rdc-apply-{}-{}",
            std::process::id(),
            stage::unix_seconds()
        ));
        let _ = fs::remove_dir_all(&root);
        let install = fake_install(&root);

        let backup = backup_install_dir_to(&install, &root, "0.1.1").expect("备份");
        assert!(backup.join("rdc.exe").is_file());
        assert!(!backup.join("config").exists(), "config 不得进备份");

        // 模拟升级：覆盖 exe/版本，并新增一个文件。
        fs::write(install.join("rdc.exe"), b"new-exe").expect("覆盖 exe");
        fs::write(install.join("BUILD_INFO.txt"), b"version=0.1.2\n").expect("覆盖版本");
        fs::write(install.join("new-only.txt"), b"new").expect("新增文件");
        fs::write(install.join("config").join("settings.json"), b"{\"keep\":\"new\"}")
            .expect("修改设置");

        let stats = restore_backup(&backup, &install).expect("恢复");
        assert!(stats.restored >= 3);
        assert_eq!(fs::read(install.join("rdc.exe")).expect("读 exe"), b"old-exe");
        assert_eq!(read_build_version(&install).as_deref(), Some("0.1.1"));
        assert!(!install.join("new-only.txt").exists(), "新文件必须被删除");
        assert_eq!(
            fs::read(install.join("config").join("settings.json")).expect("读设置"),
            b"{\"keep\":\"new\"}",
            "恢复不得覆盖用户配置"
        );

        let _ = fs::remove_dir_all(&root);
    }

    /// 版本解析必须只认 `version=` 行。
    #[test]
    fn reads_build_version() {
        let root = std::env::temp_dir().join(format!(
            "rdc-buildinfo-{}-{}",
            std::process::id(),
            stage::unix_seconds()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("创建目录");
        fs::write(root.join("BUILD_INFO.txt"), b"commit=x\nversion=1.2.3\n").expect("写");
        assert_eq!(read_build_version(&root).as_deref(), Some("1.2.3"));
        let _ = fs::remove_dir_all(&root);
    }
}
