//! rdc-update / 检查更新与下载暂存
//!
//! 层级：L2 能力层
//! 用途：读取 GitHub Release → 验签更新清单 → 比对版本 → 下载安装包 →
//!       SHA256 与 `.sha256` 双校验 → 落到暂存目录。
//! 调用方：rdc-updater CLI（由 Flutter 更新服务调用）。
//!
//! 暂存位置：`%LOCALAPPDATA%\RDC\updates\<版本>\`（普通用户可写，不需要管理员）。
//! 未经校验的文件只以 `.part` 存在，校验通过才改名为最终文件。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use ureq::Agent;

use crate::error::{codes, Result, UpdateError};
use crate::github;
use crate::hash::{sha256_file, sha256_hex};
use crate::manifest::{self, UpdateManifest};
use crate::version;

/// 清单资产名（发布流水线必须使用同一名称）。
pub const MANIFEST_ASSET: &str = "update-manifest.json";
/// 签名资产名。
pub const SIGNATURE_ASSET: &str = "update-manifest.json.sig";

/// 检查结果（可直接序列化给 UI）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateCheck {
    /// 本端版本。
    pub current_version: String,
    /// 远端版本。
    pub latest_version: String,
    /// 是否有更新。
    pub available: bool,
    /// GitHub 标签。
    pub tag: String,
    /// 更新说明。
    pub notes: String,
    /// 安装包文件名。
    pub installer_name: String,
    /// 安装包 SHA256（来自已验签清单）。
    pub installer_sha256: String,
    /// 安装包大小（0 表示未知）。
    pub installer_size: u64,
    /// 安装包下载地址。
    pub download_url: String,
    /// 清单要求的最低可升级版本（空表示不限制）。
    pub min_supported_version: String,
}

/// 已下载并校验通过的更新。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StagedUpdate {
    /// 目标版本。
    pub version: String,
    /// 安装包绝对路径。
    pub installer_path: String,
    /// 校验通过的 SHA256。
    pub sha256: String,
    /// 字节数。
    pub size: u64,
    /// 暂存目录。
    pub staging_dir: String,
}

/// 暂存根目录。
///
/// # Returns
/// Windows 为 `%LOCALAPPDATA%\RDC\updates`；可用 `RDC_UPDATE_STAGING_DIR` 覆盖。
///
/// # Errors
/// 不会失败。
pub fn staging_root() -> PathBuf {
    if let Ok(value) = std::env::var("RDC_UPDATE_STAGING_DIR") {
        if !value.trim().is_empty() {
            return PathBuf::from(value);
        }
    }
    if cfg!(windows) {
        if let Ok(local) = std::env::var("LOCALAPPDATA") {
            if !local.trim().is_empty() {
                return PathBuf::from(local).join("RDC").join("updates");
            }
        }
    }
    std::env::temp_dir().join("rdc-updates")
}

/// 检查是否有新版本。
///
/// # Arguments
/// * `api_url` - Release API 地址
/// * `token` - GitHub Token（私有仓库需要）
/// * `current_version` - 本端版本
/// * `public_key_b64` - 发布公钥（Base64）
///
/// # Returns
/// 检查结果；即使没有新版本也返回清单信息（便于展示最新版本号）。
///
/// # Errors
/// 网络失败、清单缺失、验签失败、版本非法时返回固定错误码。
pub fn check(
    api_url: &str,
    token: Option<&str>,
    current_version: &str,
    public_key_b64: &str,
) -> Result<UpdateCheck> {
    // 本端版本先解析：非法版本不参与任何比较（防止字符串比较误判）。
    version::parse(current_version)?;
    let agent = github::build_agent();
    let verified = load_verified_manifest(&agent, api_url, token, public_key_b64)?;
    let release = &verified.release;
    let manifest = &verified.manifest;

    let installer = release.asset(&manifest.installer.name).ok_or_else(|| {
        UpdateError::new(
            codes::MANIFEST_INVALID,
            format!("Release 中缺少安装包资产：{}", manifest.installer.name),
        )
    })?;

    let available = version::is_newer(&manifest.version, current_version)?;
    Ok(UpdateCheck {
        current_version: current_version.to_owned(),
        latest_version: manifest.version.clone(),
        available,
        tag: release.tag.clone(),
        notes: manifest.notes.clone(),
        installer_name: manifest.installer.name.clone(),
        installer_sha256: manifest.installer.sha256.to_lowercase(),
        installer_size: if manifest.installer.size > 0 {
            manifest.installer.size
        } else {
            installer.size
        },
        download_url: installer.url.clone(),
        min_supported_version: manifest.min_supported_version.clone(),
    })
}

/// 下载并校验指定版本，落到暂存目录。
///
/// # Arguments
/// * `api_url` - Release API 地址
/// * `token` - GitHub Token
/// * `public_key_b64` - 发布公钥
/// * `expected_version` - 期望下载的版本（必须与清单一致）
///
/// # Returns
/// 已校验的暂存结果。
///
/// # Errors
/// 任何校验失败都会删除半成品并返回固定错误码。
pub fn download(
    api_url: &str,
    token: Option<&str>,
    public_key_b64: &str,
    expected_version: &str,
) -> Result<StagedUpdate> {
    let agent = github::build_agent();
    let verified = load_verified_manifest(&agent, api_url, token, public_key_b64)?;
    let release = &verified.release;
    let manifest = &verified.manifest;

    // 防降级/串包：调用方指定的版本必须与已验签清单完全一致。
    if version::parse(&manifest.version)? != version::parse(expected_version)? {
        return Err(UpdateError::new(
            codes::MANIFEST_INVALID,
            format!(
                "清单版本 {} 与请求版本 {expected_version} 不一致",
                manifest.version
            ),
        ));
    }

    let installer_asset = release.asset(&manifest.installer.name).ok_or_else(|| {
        UpdateError::new(
            codes::MANIFEST_INVALID,
            format!("Release 中缺少安装包资产：{}", manifest.installer.name),
        )
    })?;

    // `.sha256` 必须存在并交叉校验：这是除签名外的第二道锚点。
    let sha_name = format!("{}.sha256", manifest.installer.name);
    let sha_asset = release.asset(&sha_name).ok_or_else(|| {
        UpdateError::new(
            codes::HASH_MISSING,
            format!("Release 中缺少哈希文件：{sha_name}"),
        )
    })?;
    let sha_text = github::fetch_text(&agent, &sha_asset.url, token, github::MAX_TEXT_BYTES)?;
    let (file_hash, file_name) = parse_sha256_file(&sha_text)?;
    let manifest_hash = manifest.installer.sha256.trim().to_lowercase();
    if file_hash != manifest_hash {
        return Err(UpdateError::new(
            codes::HASH_FILE_INVALID,
            format!(
                "{} 中的哈希与更新清单不一致（文件 {file_hash} / 清单 {manifest_hash}）",
                sha_name
            ),
        ));
    }
    if let Some(name) = file_name {
        if name != manifest.installer.name {
            return Err(UpdateError::new(
                codes::HASH_FILE_INVALID,
                format!("{} 指向的文件名 {name} 与清单不一致", sha_name),
            ));
        }
    }

    // 暂存目录按版本隔离，便于回滚与排查；同名旧文件先清理。
    let staging_dir = staging_root().join(&manifest.version);
    fs::create_dir_all(&staging_dir)?;
    let final_path = staging_dir.join(&manifest.installer.name);
    let part_path = staging_dir.join(format!("{}.part", manifest.installer.name));
    let _ = fs::remove_file(&final_path);
    let _ = fs::remove_file(&part_path);

    let (size, actual_hash) = github::download_to(
        &agent,
        &installer_asset.url,
        token,
        &part_path,
        github::MAX_INSTALLER_BYTES,
    )?;
    if actual_hash != manifest_hash {
        // 校验失败立即删除：绝不把可疑文件留在可执行位置。
        let _ = fs::remove_file(&part_path);
        return Err(UpdateError::new(
            codes::HASH_MISMATCH,
            format!(
                "安装包 SHA256 不匹配（期望 {manifest_hash}，实际 {actual_hash}）"
            ),
        ));
    }
    fs::rename(&part_path, &final_path)?;

    // 保存清单与签名，便于事后审计“这个包当时依据什么被信任”。
    let _ = fs::write(staging_dir.join(MANIFEST_ASSET), &verified.manifest_bytes);
    let _ = fs::write(staging_dir.join(SIGNATURE_ASSET), &verified.signature);

    Ok(StagedUpdate {
        version: manifest.version.clone(),
        installer_path: final_path.display().to_string(),
        sha256: actual_hash,
        size,
        staging_dir: staging_dir.display().to_string(),
    })
}

/// 重新校验暂存文件（升级前复检，防止暂存目录被替换）。
///
/// # Arguments
/// * `staged` - 暂存结果
///
/// # Returns
/// 校验通过返回 `Ok(())`。
///
/// # Errors
/// 文件缺失或哈希不符时返回 [`codes::HASH_MISMATCH`]。
pub fn verify_staged(staged: &StagedUpdate) -> Result<()> {
    let path = Path::new(&staged.installer_path);
    if !path.is_file() {
        return Err(UpdateError::new(
            codes::IO,
            format!("暂存安装包不存在：{}", staged.installer_path),
        ));
    }
    let (size, hash) = sha256_file(path)?;
    if hash != staged.sha256.to_lowercase() {
        return Err(UpdateError::new(
            codes::HASH_MISMATCH,
            format!(
                "暂存安装包哈希变化（期望 {}，实际 {hash}）",
                staged.sha256
            ),
        ));
    }
    if staged.size != 0 && size != staged.size {
        return Err(UpdateError::new(
            codes::HASH_MISMATCH,
            format!("暂存安装包大小变化（期望 {}，实际 {size}）", staged.size),
        ));
    }
    Ok(())
}

/// 解析 `.sha256` 文件（兼容 `hash` / `hash  name` / `hash *name`）。
///
/// # Arguments
/// * `text` - 文件内容
///
/// # Returns
/// `(小写哈希, 可选文件名)`。
///
/// # Errors
/// 内容不是 64 位十六进制时返回 [`codes::HASH_FILE_INVALID`]。
pub fn parse_sha256_file(text: &str) -> Result<(String, Option<String>)> {
    let first = text
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    let mut parts = first.split_whitespace();
    let hash = parts.next().unwrap_or("").to_lowercase();
    if !crate::hash::is_sha256_hex(&hash) {
        return Err(UpdateError::new(
            codes::HASH_FILE_INVALID,
            format!(".sha256 文件格式非法：{first}"),
        ));
    }
    let name = parts.next().map(|value| {
        value.trim_start_matches('*').trim().to_owned()
    });
    Ok((hash, name))
}

/// 已验签的发布信息（避免重复拉取 Release 与清单）。
struct VerifiedRelease {
    /// Release 摘要。
    release: github::ReleaseInfo,
    /// 已验签清单。
    manifest: UpdateManifest,
    /// 清单原始字节（审计与暂存用）。
    manifest_bytes: Vec<u8>,
    /// 清单签名原文。
    signature: String,
}

/// 读取并验签发布清单。
///
/// # Arguments
/// * `agent` - HTTP 客户端
/// * `api_url` - Release API
/// * `token` - GitHub Token
/// * `public_key_b64` - 发布公钥
///
/// # Returns
/// 已验签的发布信息。
///
/// # Errors
/// 资产缺失/验签失败/版本不一致时返回固定错误码。
fn load_verified_manifest(
    agent: &Agent,
    api_url: &str,
    token: Option<&str>,
    public_key_b64: &str,
) -> Result<VerifiedRelease> {
    let release = github::fetch_release(agent, api_url, token)?;
    let (manifest_bytes, signature) = fetch_manifest_pair(agent, &release, token)?;
    let manifest = manifest::parse_and_verify(&manifest_bytes, &signature, public_key_b64)?;
    // 标签与清单版本必须一致：阻断“新标签带旧包”这种拼装方式。
    if version::parse(&release.tag)? != version::parse(&manifest.version)? {
        return Err(UpdateError::new(
            codes::VER_TAG_MISMATCH,
            format!(
                "Release 标签 {} 与清单版本 {} 不一致",
                release.tag, manifest.version
            ),
        ));
    }
    Ok(VerifiedRelease {
        release,
        manifest,
        manifest_bytes,
        signature,
    })
}

/// 下载清单与签名文件。
///
/// # Arguments
/// * `agent` - HTTP 客户端
/// * `release` - 已读取的 Release
/// * `token` - GitHub Token
///
/// # Returns
/// `(清单字节, 签名文本)`。
///
/// # Errors
/// 任一资产缺失/下载失败时返回固定错误码。
fn fetch_manifest_pair(
    agent: &Agent,
    release: &github::ReleaseInfo,
    token: Option<&str>,
) -> Result<(Vec<u8>, String)> {
    let manifest_asset = release.asset(MANIFEST_ASSET).ok_or_else(|| {
        UpdateError::new(
            codes::MANIFEST_MISSING,
            format!("Release 中缺少 {MANIFEST_ASSET}"),
        )
    })?;
    let signature_asset = release.asset(SIGNATURE_ASSET).ok_or_else(|| {
        UpdateError::new(
            codes::SIG_MISSING,
            format!("Release 中缺少 {SIGNATURE_ASSET}"),
        )
    })?;
    let bytes = github::fetch_bytes(
        agent,
        &manifest_asset.url,
        token,
        github::MAX_TEXT_BYTES,
    )?;
    let signature = github::fetch_text(
        agent,
        &signature_asset.url,
        token,
        github::MAX_TEXT_BYTES,
    )?;
    Ok((bytes, signature))
}

/// 清理指定版本以外的旧暂存目录（最多保留当前版本与 2 个备份目录）。
///
/// # Arguments
/// * `keep_version` - 需要保留的版本目录名
///
/// # Returns
/// 删除的目录数。
///
/// # Errors
/// 不会失败：清理属于尽力而为。
pub fn cleanup_staging(keep_version: &str) -> usize {
    let root = staging_root();
    let mut removed = 0usize;
    let mut backups: Vec<PathBuf> = Vec::new();
    let Ok(entries) = fs::read_dir(&root) else {
        return 0;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name == keep_version {
            continue;
        }
        if name.starts_with("backup-") {
            backups.push(path);
            continue;
        }
        // 只清理“看起来像版本号”的目录，避免误删用户手工放进来的文件。
        if version::parse(&name).is_ok() && fs::remove_dir_all(&path).is_ok() {
            removed += 1;
        }
    }
    // 备份按路径名排序（含时间戳），保留最近 2 个。
    backups.sort();
    while backups.len() > 2 {
        let path = backups.remove(0);
        if fs::remove_dir_all(path).is_ok() {
            removed += 1;
        }
    }
    removed
}

/// 计算内容哈希（测试与审计用）。
///
/// # Arguments
/// * `bytes` - 原始字节
///
/// # Returns
/// 小写十六进制 SHA256。
///
/// # Errors
/// 不会失败。
pub fn hash_bytes(bytes: &[u8]) -> String {
    sha256_hex(bytes)
}

/// 当前 Unix 秒（备份目录命名用）。
///
/// # Returns
/// 秒级时间戳。
///
/// # Errors
/// 不会失败。
pub fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
