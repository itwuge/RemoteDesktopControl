//! rdc-core / 安装目录与配置目录解析、旧配置迁移
//!
//! 层级：L3 编排层（与 UI 无关的纯文件系统逻辑）
//! 用途：把「配置跟安装目录放一起」落成唯一规则，并在首次启动时把
//!       `%APPDATA%\RDC` 的历史数据搬到 `<安装目录>\config`。
//! 调用方：rdc-bridge（FFI）、rdc-cli（诊断）、rdc-update（暂存与备份）。
//!
//! 规则（Windows）：
//! 1. 环境变量 `RDC_CONFIG_DIR` 优先（运维/测试覆盖）；
//! 2. 其次 `<exe 所在目录>\config`（安装后即 `C:\Program Files\RDC\config`）；
//! 3. 该目录不可创建/不可写时，回退 `%APPDATA%\RDC`，并记录 `RDC-CFG-PATH-001`；
//! 4. 其它平台沿用 `%APPDATA%`/XDG 旧规则，避免影响已验证的 Linux/macOS 口径。
//!
//! 迁移纪律：先备份、只复制不删除、失败留报告（`migration.json` + `migration.log`）。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

// =====** 常量 =====**

/// 迁移成功（含首次迁移与「无需迁移」）。
pub const CODE_MIG_OK: &str = "RDC-CFG-MIG-000";
/// 源目录不可读。
pub const CODE_MIG_SOURCE_UNREADABLE: &str = "RDC-CFG-MIG-001";
/// 目标目录无法创建或不可写。
pub const CODE_MIG_TARGET_UNWRITABLE: &str = "RDC-CFG-MIG-002";
/// 备份失败（此时不继续迁移，避免丢数据）。
pub const CODE_MIG_BACKUP_FAILED: &str = "RDC-CFG-MIG-003";
/// 复制文件失败。
pub const CODE_MIG_COPY_FAILED: &str = "RDC-CFG-MIG-004";
/// 迁移报告写入失败。
pub const CODE_MIG_REPORT_FAILED: &str = "RDC-CFG-MIG-005";
/// 安装目录不可写，已回退到旧配置目录（非迁移错误，仅提示）。
pub const CODE_PATH_FALLBACK: &str = "RDC-CFG-PATH-001";

/// 迁移报告文件名。
const MIGRATION_FILE: &str = "migration.json";
/// 迁移日志文件名。
const MIGRATION_LOG: &str = "migration.log";
/// 备份目录前缀。
const BACKUP_PREFIX: &str = "backup-";
/// 环境变量覆盖键。
const ENV_CONFIG_DIR: &str = "RDC_CONFIG_DIR";

// =====** 路径解析 =====**

/// 返回当前进程可执行文件所在目录；解析失败时退回当前工作目录。
///
/// # Returns
/// 绝对路径（尽力而为）。
///
/// # Errors
/// 不会失败：无法解析时使用 `.`，调用方仍可得到可用的相对路径。
pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 返回本机历史配置目录（仅 Windows 有值）。
///
/// # Returns
/// `%APPDATA%\RDC`；环境变量缺失时返回 `None`。
///
/// # Errors
/// 不会失败。
pub fn legacy_config_dir() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let appdata = std::env::var("APPDATA").ok()?;
    if appdata.trim().is_empty() {
        return None;
    }
    Some(PathBuf::from(appdata).join("RDC"))
}

/// 返回本进程使用的配置目录（进程内缓存）。
///
/// # Returns
/// 首次调用时按 [`resolve_config_dir`] 解析并缓存。
///
/// # Errors
/// 不会失败；目录创建失败时回退到可写位置。
pub fn config_dir() -> PathBuf {
    static CACHE: OnceLock<PathBuf> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let env_override = std::env::var(ENV_CONFIG_DIR).ok();
            let legacy = legacy_config_dir();
            let resolved = resolve_config_dir(
                &exe_dir(),
                legacy.as_deref(),
                env_override.as_deref(),
                cfg!(windows),
            );
            // 回退是异常路径：留下痕迹，便于用户把日志发回来定位权限问题。
            if let Some(legacy_path) = legacy.as_deref() {
                if resolved == legacy_path && resolved != exe_dir().join("config") {
                    eprintln!(
                        "[{CODE_PATH_FALLBACK}] 安装目录不可写，配置回退到 {}",
                        resolved.display()
                    );
                }
            }
            resolved
        })
        .clone()
}

/// 按显式参数解析配置目录（供测试与 CLI 复用，不读写缓存）。
///
/// # Arguments
/// * `exe_dir` - 可执行文件目录
/// * `legacy` - 历史配置目录（Windows 为 `%APPDATA%\RDC`）
/// * `env_override` - `RDC_CONFIG_DIR` 的值（可为空）
/// * `prefer_install_dir` - 是否启用「安装目录旁 config」规则（Windows 为 true）
///
/// # Returns
/// 解析后的配置目录；优先级见模块注释。
///
/// # Errors
/// 不会失败：所有候选都不可写时退回系统临时目录。
pub fn resolve_config_dir(
    exe_dir: &Path,
    legacy: Option<&Path>,
    env_override: Option<&str>,
    prefer_install_dir: bool,
) -> PathBuf {
    // 1. 环境变量：运维手册与自动化测试都靠它隔离数据。
    if let Some(value) = env_override {
        if !value.trim().is_empty() {
            return PathBuf::from(value);
        }
    }

    // 2. 安装目录旁的 config：这是产品口径（docs/09 目录约定 + 用户要求）。
    if prefer_install_dir {
        let candidate = exe_dir.join("config");
        // 探测真实写权限：Program Files 下若安装器漏授 ACL，这里会失败并回退。
        if ensure_writable_dir(&candidate) {
            return candidate;
        }
    }

    // 3. 历史目录：保证免安装/受限环境仍能启动。
    if let Some(path) = legacy {
        if ensure_writable_dir(path) {
            return path.to_path_buf();
        }
    }

    // 4. 最后兜底：临时目录（只保证可写，不保证跨重启持久）。
    std::env::temp_dir().join("rdc")
}

/// 确保目录存在且可写。
///
/// # Arguments
/// * `path` - 目标目录
///
/// # Returns
/// 可创建且可写返回 `true`。
///
/// # Errors
/// 不会返回错误：失败统一映射为 `false`，由调用方决定回退策略。
pub fn ensure_writable_dir(path: &Path) -> bool {
    if fs::create_dir_all(path).is_err() {
        return false;
    }
    // 用一次性探针文件验证真实写权限：只看元数据会被「只读 ACL」骗过去。
    let probe = path.join(format!(".rdc-write-test-{}", std::process::id()));
    match fs::write(&probe, b"ok") {
        Ok(()) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

// =====** 迁移报告 =====**

/// 旧配置迁移结果（同时作为 `migration.json` 的落盘结构）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MigrationReport {
    /// `ok` / `noop` / `already` / `error`。
    pub status: String,
    /// 错误码或 `RDC-CFG-MIG-000`。
    pub code: String,
    /// 供用户阅读的中文说明。
    pub message: String,
    /// 源目录（可能为空）。
    pub source: String,
    /// 目标目录。
    pub target: String,
    /// 备份目录（未备份时为空）。
    pub backup: String,
    /// 成功复制到目标的文件（相对路径）。
    pub files: Vec<String>,
    /// 目标已存在、未覆盖的文件（相对路径）。
    pub skipped: Vec<String>,
    /// 开始时间（Unix 毫秒）。
    pub started_at_ms: u64,
    /// 结束时间（Unix 毫秒）。
    pub finished_at_ms: u64,
}

impl MigrationReport {
    /// 构造一条结果记录。
    ///
    /// # Arguments
    /// * `status` - 状态字符串
    /// * `code` - 错误码/成功码
    /// * `message` - 中文说明
    /// * `source` - 源目录
    /// * `target` - 目标目录
    ///
    /// # Returns
    /// 时间戳已填好的报告。
    ///
    /// # Errors
    /// 不会失败。
    fn new(status: &str, code: &str, message: &str, source: &Path, target: &Path) -> Self {
        let now = now_ms();
        Self {
            status: status.to_owned(),
            code: code.to_owned(),
            message: message.to_owned(),
            source: source.display().to_string(),
            target: target.display().to_string(),
            backup: String::new(),
            files: Vec::new(),
            skipped: Vec::new(),
            started_at_ms: now,
            finished_at_ms: now,
        }
    }

    /// 是否为成功类结果（`ok` / `noop` / `already`）。
    ///
    /// # Returns
    /// 成功返回 `true`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn is_success(&self) -> bool {
        matches!(self.status.as_str(), "ok" | "noop" | "already")
    }
}

/// 当前 Unix 毫秒时间戳。
///
/// # Returns
/// 毫秒数；系统时钟早于 1970 时返回 0。
///
/// # Errors
/// 不会失败。
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 迁移报告落盘路径。
///
/// # Arguments
/// * `target` - 配置目录
///
/// # Returns
/// `<target>\migration.json`。
///
/// # Errors
/// 不会失败。
pub fn migration_report_path(target: &Path) -> PathBuf {
    target.join(MIGRATION_FILE)
}

// =====** 迁移主流程 =====**

/// 把历史配置迁移到当前配置目录（幂等）。
///
/// # Returns
/// 迁移报告；调用方应把 `is_success()==false` 的 `code` 展示给用户。
///
/// # Errors
/// 不会失败：所有错误都写进报告，保证启动流程不被阻断。
pub fn migrate_legacy_config() -> MigrationReport {
    let target = config_dir();
    let legacy = legacy_config_dir();
    migrate_legacy_config_to(&target, legacy.as_deref())
}

/// 按显式路径执行迁移（供测试复用，不依赖环境变量）。
///
/// # Arguments
/// * `target` - 目标配置目录
/// * `legacy` - 历史配置目录
///
/// # Returns
/// 迁移报告。
///
/// # Errors
/// 不会失败：错误写入报告。
pub fn migrate_legacy_config_to(target: &Path, legacy: Option<&Path>) -> MigrationReport {
    let source = legacy.unwrap_or_else(|| Path::new(""));
    // 目标与源相同：没有迁移的必要（也是防递归的保护）。
    if let Some(path) = legacy {
        if same_path(path, target) {
            return MigrationReport::new(
                "noop",
                CODE_MIG_OK,
                "配置目录与历史目录相同，无需迁移",
                path,
                target,
            );
        }
        if !path.is_dir() {
            return MigrationReport::new(
                "noop",
                CODE_MIG_OK,
                "未发现历史配置目录，无需迁移",
                path,
                target,
            );
        }
    } else {
        return MigrationReport::new(
            "noop",
            CODE_MIG_OK,
            "当前平台没有历史配置迁移项",
            source,
            target,
        );
    }
    let legacy_path = legacy.expect("上面已保证 legacy 非空");

    // 幂等：上次成功迁移过就不再重复复制（避免覆盖用户后来改过的设置）。
    if let Some(previous) = read_existing_report(target) {
        if previous.status == "ok" {
            return MigrationReport {
                message: "已完成过迁移，本次跳过".to_owned(),
                status: "already".to_owned(),
                code: CODE_MIG_OK.to_owned(),
                ..previous
            };
        }
    }

    let mut report = MigrationReport::new(
        "error",
        CODE_MIG_OK,
        "",
        legacy_path,
        target,
    );

    // 目标不可写：直接失败，绝不把数据搬进一个写不进去的位置。
    if !ensure_writable_dir(target) {
        report.code = CODE_MIG_TARGET_UNWRITABLE.to_owned();
        report.message = format!("目标配置目录不可写：{}", target.display());
        report.finished_at_ms = now_ms();
        let _ = append_log(target, &report);
        return report;
    }

    // 收集源文件；读不了源就报错，保留现场。
    let entries = match collect_relative_files(legacy_path) {
        Ok(files) => files,
        Err(error) => {
            report.code = CODE_MIG_SOURCE_UNREADABLE.to_owned();
            report.message = format!("读取历史配置失败：{error}");
            report.finished_at_ms = now_ms();
            let _ = append_log(target, &report);
            return report;
        }
    };
    if entries.is_empty() {
        report.status = "noop".to_owned();
        report.message = "历史配置目录为空，无需迁移".to_owned();
        report.finished_at_ms = now_ms();
        let _ = write_report(target, &report);
        return report;
    }

    // 备份先于任何写入：迁移失败时用户仍有完整原件可回退。
    let backup = legacy_path.join(format!("{BACKUP_PREFIX}{}", timestamp_tag()));
    if let Err(error) = copy_files(legacy_path, &backup, &entries) {
        report.code = CODE_MIG_BACKUP_FAILED.to_owned();
        report.message = format!("迁移前备份失败：{error}");
        report.finished_at_ms = now_ms();
        let _ = append_log(target, &report);
        return report;
    }
    report.backup = backup.display().to_string();

    // 逐文件复制：目标已存在就不覆盖（用户当前数据优先），只登记到 skipped。
    for relative in &entries {
        let name = relative.display().to_string();
        let destination = target.join(relative);
        if destination.exists() {
            report.skipped.push(name);
            continue;
        }
        if let Some(parent) = destination.parent() {
            if let Err(error) = fs::create_dir_all(parent) {
                report.code = CODE_MIG_COPY_FAILED.to_owned();
                report.message = format!("创建目录失败（{}）：{error}", parent.display());
                report.finished_at_ms = now_ms();
                let _ = write_report(target, &report);
                return report;
            }
        }
        match fs::copy(legacy_path.join(relative), &destination) {
            Ok(_) => report.files.push(name),
            Err(error) => {
                report.code = CODE_MIG_COPY_FAILED.to_owned();
                report.message = format!("复制 {name} 失败：{error}");
                report.finished_at_ms = now_ms();
                let _ = write_report(target, &report);
                return report;
            }
        }
    }

    report.status = "ok".to_owned();
    report.code = CODE_MIG_OK.to_owned();
    report.message = format!(
        "已迁移 {} 个文件；{} 个文件因目标已存在而保留（源文件未删除）",
        report.files.len(),
        report.skipped.len()
    );
    report.finished_at_ms = now_ms();

    // 报告写失败不影响已完成的迁移，但必须留下日志线索。
    if let Err(error) = write_report(target, &report) {
        report.code = CODE_MIG_REPORT_FAILED.to_owned();
        report.message = format!("{}；但迁移报告写入失败：{error}", report.message);
    }
    let _ = append_log(target, &report);
    report
}

// =====** 内部工具 =====**

/// 判断两个路径是否指向同一位置（大小写不敏感仅对 Windows 生效）。
///
/// # Arguments
/// * `a` / `b` - 待比较路径
///
/// # Returns
/// 相同返回 `true`。
///
/// # Errors
/// 不会失败。
fn same_path(a: &Path, b: &Path) -> bool {
    let a = a.to_string_lossy().replace('/', "\\").to_lowercase();
    let b = b.to_string_lossy().replace('/', "\\").to_lowercase();
    a == b
}

/// 生成 Unix 秒级时间标签（稳定、无时区依赖，便于日志对时）。
///
/// # Returns
/// 文件名安全的十进制时间字符串。
///
/// # Errors
/// 不会失败。
fn timestamp_tag() -> String {
    format!("{}", now_ms() / 1000)
}

/// 递归收集文件相对路径，跳过历史备份目录与临时文件。
///
/// # Arguments
/// * `root` - 源根目录
///
/// # Returns
/// 按字典序排序的相对路径列表。
///
/// # Errors
/// 读取目录失败时返回 [`io::Error`]。
fn collect_relative_files(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    collect_into(root, root, &mut out)?;
    out.sort();
    Ok(out)
}

/// 递归收集实现。
///
/// # Arguments
/// * `root` - 源根目录
/// * `current` - 当前遍历目录
/// * `out` - 输出相对路径
///
/// # Returns
/// 成功返回 `Ok(())`。
///
/// # Errors
/// 读取目录失败时返回错误。
fn collect_into(root: &Path, current: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        // 备份目录自身不参与迁移，否则每次迁移都会把旧备份再搬一遍。
        if name.starts_with(BACKUP_PREFIX) || name.ends_with(".tmp") {
            continue;
        }
        if path.is_dir() {
            collect_into(root, &path, out)?;
        } else if let Ok(relative) = path.strip_prefix(root) {
            out.push(relative.to_path_buf());
        }
    }
    Ok(())
}

/// 把一组相对路径从 `root` 复制到 `destination`。
///
/// # Arguments
/// * `source_root` - 源根目录
/// * `destination` - 目标根目录
/// * `files` - 相对路径列表
///
/// # Returns
/// 成功返回 `Ok(())`。
///
/// # Errors
/// 任一文件复制失败时返回错误。
fn copy_files(source_root: &Path, destination: &Path, files: &[PathBuf]) -> io::Result<()> {
    for relative in files {
        let target = destination.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(source_root.join(relative), &target)?;
    }
    Ok(())
}

/// 读取已有的迁移报告。
///
/// # Arguments
/// * `target` - 配置目录
///
/// # Returns
/// 文件存在且可解析时返回报告。
///
/// # Errors
/// 不会返回错误：读取/解析失败一律视为「没有报告」。
fn read_existing_report(target: &Path) -> Option<MigrationReport> {
    let raw = fs::read_to_string(migration_report_path(target)).ok()?;
    serde_json::from_str(&raw).ok()
}

/// 写入迁移报告。
///
/// # Arguments
/// * `target` - 配置目录
/// * `report` - 报告
///
/// # Returns
/// 成功返回 `Ok(())`。
///
/// # Errors
/// 目录不存在或写盘失败时返回错误。
fn write_report(target: &Path, report: &MigrationReport) -> io::Result<()> {
    fs::create_dir_all(target)?;
    let json = serde_json::to_string_pretty(report).map_err(io::Error::other)?;
    let path = migration_report_path(target);
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, json)?;
    fs::rename(temp, path)
}

/// 追加一行迁移日志（JSON Lines），失败时静默（日志不能反过来阻断迁移）。
///
/// # Arguments
/// * `target` - 配置目录
/// * `report` - 报告
///
/// # Returns
/// 成功返回 `Ok(())`。
///
/// # Errors
/// 写盘失败时返回错误，由调用方忽略。
fn append_log(target: &Path, report: &MigrationReport) -> io::Result<()> {
    fs::create_dir_all(target)?;
    let line = serde_json::to_string(report).map_err(io::Error::other)?;
    let path = target.join(MIGRATION_LOG);
    let mut existing = fs::read_to_string(&path).unwrap_or_default();
    existing.push_str(&line);
    existing.push('\n');
    fs::write(path, existing)
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 每次测试使用独立临时目录，避免并行用例互相污染。
    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rdc-paths-{tag}-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("创建测试目录");
        dir
    }

    /// 环境变量优先于安装目录。
    #[test]
    fn env_override_wins() {
        let root = temp_root("env");
        let resolved = resolve_config_dir(&root.join("exe"), None, Some("D:/rdc-cfg"), true);
        assert_eq!(resolved, PathBuf::from("D:/rdc-cfg"));
    }

    /// Windows 规则：优先安装目录旁的 config。
    #[test]
    fn install_dir_config_is_preferred() {
        let root = temp_root("install");
        let exe = root.join("exe");
        fs::create_dir_all(&exe).expect("创建 exe 目录");
        let legacy = root.join("legacy");
        let resolved = resolve_config_dir(&exe, Some(&legacy), None, true);
        assert_eq!(resolved, exe.join("config"));
        assert!(resolved.is_dir());
    }

    /// 安装目录不可写时回退历史目录。
    #[test]
    fn falls_back_to_legacy_when_install_dir_readonly() {
        let root = temp_root("fallback");
        // 用一个文件占住 `config` 名称，创建目录必然失败。
        let exe = root.join("exe");
        fs::create_dir_all(&exe).expect("创建 exe 目录");
        fs::write(exe.join("config"), b"occupied").expect("占位文件");
        let legacy = root.join("legacy");
        let resolved = resolve_config_dir(&exe, Some(&legacy), None, true);
        assert_eq!(resolved, legacy);
    }

    /// 迁移必须复制全部文件、生成备份与报告，且不删除源文件。
    #[test]
    fn migration_copies_backs_up_and_keeps_source() {
        let root = temp_root("migrate");
        let legacy = root.join("legacy");
        fs::create_dir_all(&legacy).expect("创建源目录");
        fs::write(legacy.join("settings.json"), b"{\"allow_relay\":false}").expect("写设置");
        fs::write(legacy.join("identity.seed"), b"seed").expect("写身份");
        fs::create_dir_all(legacy.join("nested")).expect("创建子目录");
        fs::write(legacy.join("nested").join("devices.json"), b"[]").expect("写设备");

        let target = root.join("target");
        let report = migrate_legacy_config_to(&target, Some(&legacy));

        assert_eq!(report.status, "ok", "报告：{report:?}");
        assert_eq!(report.files.len(), 3);
        assert!(target.join("settings.json").is_file());
        assert!(target.join("nested").join("devices.json").is_file());
        assert!(Path::new(&report.backup).is_dir());
        assert!(legacy.join("settings.json").is_file(), "源文件必须保留");
        assert!(migration_report_path(&target).is_file());
        let raw = fs::read_to_string(migration_report_path(&target)).expect("读报告");
        let parsed: MigrationReport = serde_json::from_str(&raw).expect("解析报告");
        assert_eq!(parsed.status, "ok");
        assert_eq!(parsed.code, CODE_MIG_OK);
    }

    /// 第二次迁移应识别为已完成并保留用户后来修改的设置。
    #[test]
    fn migration_is_idempotent_and_never_overwrites() {
        let root = temp_root("idempotent");
        let legacy = root.join("legacy");
        fs::create_dir_all(&legacy).expect("创建源目录");
        fs::write(legacy.join("settings.json"), b"old").expect("写源设置");
        let target = root.join("target");

        let first = migrate_legacy_config_to(&target, Some(&legacy));
        assert_eq!(first.status, "ok");
        fs::write(target.join("settings.json"), b"new").expect("写新设置");

        let second = migrate_legacy_config_to(&target, Some(&legacy));
        assert_eq!(second.status, "already");
        assert_eq!(
            fs::read(target.join("settings.json")).expect("读"),
            b"new",
            "不得覆盖用户当前设置"
        );
    }

    /// 用真实旧数据做人工验证（默认忽略）。
    ///
    /// 运行方式：设置 `RDC_MIG_LEGACY`（真实 `%APPDATA%\RDC`）与 `RDC_MIG_TARGET`
    /// （测试用 config 目录）后执行
    /// `cargo test -p rdc-core migrate_real_legacy_data -- --ignored --nocapture`。
    #[test]
    #[ignore]
    fn migrate_real_legacy_data() {
        let legacy = std::env::var("RDC_MIG_LEGACY").expect("需要 RDC_MIG_LEGACY");
        let target = std::env::var("RDC_MIG_TARGET").expect("需要 RDC_MIG_TARGET");
        let report = migrate_legacy_config_to(Path::new(&target), Some(Path::new(&legacy)));
        println!("{}", serde_json::to_string_pretty(&report).expect("序列化报告"));
        assert!(report.is_success(), "迁移失败：{report:?}");
        // 逐文件核对哈希，确认搬移内容与源一致。
        for rel in &report.files {
            let src = Path::new(&legacy).join(rel);
            let dst = Path::new(&target).join(rel);
            assert!(dst.is_file(), "目标缺少 {rel}");
            assert_eq!(
                fs::read(&src).expect("读源"),
                fs::read(&dst).expect("读目标"),
                "{rel} 内容不一致"
            );
        }
    }
}
