//! rdc-update / GitHub Releases 访问
//!
//! 层级：L2 能力层
//! 用途：读取最新 Release（版本、说明、资产列表），下载资产字节/文件。
//! 调用方：stage、rdc-updater CLI。
//!
//! 网络策略：
//! - 连接 15s、读写 60s 超时，避免 UI 长时间无响应；
//! - 失败最多重试 3 次（0.3s/0.6s 退避），仍失败返回固定错误码；
//! - 响应体有上限（默认 512 KiB 文本、200 MiB 安装包），防内存被撑爆。

use std::fs::File;
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

use serde::Deserialize;
use sha2::{Digest, Sha256};
use ureq::Agent;

use crate::error::{codes, Result, UpdateError};

/// 请求 User-Agent：GitHub API 强制要求。
const USER_AGENT: &str = "RDC-Updater/1.0";
/// 文本响应上限（Release JSON / 清单 / 签名）。
pub const MAX_TEXT_BYTES: usize = 512 * 1024;
/// 安装包上限：200 MiB，超过视为异常资产。
pub const MAX_INSTALLER_BYTES: usize = 200 * 1024 * 1024;
/// 失败重试次数。
const RETRY_ATTEMPTS: u32 = 3;

/// 构建 HTTP 客户端（超时设置与重试策略配套）。
///
/// # Returns
/// 可复用的 [`Agent`]（内部连接池）。
///
/// # Errors
/// 不会失败。
pub fn build_agent() -> Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(15))
        .timeout_read(Duration::from_secs(60))
        .timeout_write(Duration::from_secs(60))
        .build()
}

/// 重试包装：指数退避，最后一次错误原样返回。
///
/// # Arguments
/// * `operation` - 可重入的请求闭包
///
/// # Returns
/// 首次成功的结果。
///
/// # Errors
/// 连续失败时返回最后一次错误。
fn with_retry<T>(mut operation: impl FnMut() -> Result<T>) -> Result<T> {
    let mut last: Option<UpdateError> = None;
    for attempt in 0..RETRY_ATTEMPTS {
        match operation() {
            Ok(value) => return Ok(value),
            Err(error) => {
                last = Some(error);
                // 最后一次失败后不再 sleep，避免无意义等待。
                if attempt + 1 < RETRY_ATTEMPTS {
                    std::thread::sleep(Duration::from_millis(300 * (1 << attempt)));
                }
            }
        }
    }
    Err(last.unwrap_or_else(|| {
        UpdateError::new(codes::NET_REQUEST, "网络请求失败（未知原因）")
    }))
}

/// 读取 URL 文本内容。
///
/// # Arguments
/// * `agent` - HTTP 客户端
/// * `url` - 目标地址（https）
/// * `token` - GitHub Token（私有仓库需要）
/// * `max_bytes` - 响应体上限
///
/// # Returns
/// UTF-8 文本。
///
/// # Errors
/// 网络失败/状态码非 2xx/超限时返回对应错误码。
pub fn fetch_text(
    agent: &Agent,
    url: &str,
    token: Option<&str>,
    max_bytes: usize,
) -> Result<String> {
    let bytes = fetch_bytes(agent, url, token, max_bytes)?;
    String::from_utf8(bytes).map_err(|error| {
        UpdateError::new(
            codes::JSON_INVALID,
            format!("响应不是合法 UTF-8：{error}"),
        )
    })
}

/// 读取 URL 字节内容（带重试）。
///
/// # Arguments
/// * `agent` - HTTP 客户端
/// * `url` - 目标地址
/// * `token` - GitHub Token
/// * `max_bytes` - 响应体上限
///
/// # Returns
/// 原始字节。
///
/// # Errors
/// 网络失败/状态码非 2xx/超限时返回对应错误码。
pub fn fetch_bytes(
    agent: &Agent,
    url: &str,
    token: Option<&str>,
    max_bytes: usize,
) -> Result<Vec<u8>> {
    with_retry(|| {
        let mut request = agent
            .get(url)
            .set("User-Agent", USER_AGENT)
            .set("Accept", "application/vnd.github+json");
        if let Some(value) = token {
            if !value.trim().is_empty() {
                request = request.set("Authorization", &format!("Bearer {value}"));
            }
        }
        let response = request.call().map_err(map_ureq_error)?;
        let mut reader = response.into_reader().take(max_bytes as u64 + 1);
        let mut buffer = Vec::new();
        reader.read_to_end(&mut buffer)?;
        if buffer.len() > max_bytes {
            return Err(UpdateError::new(
                codes::NET_TOO_LARGE,
                format!("响应体超过上限 {max_bytes} 字节：{url}"),
            ));
        }
        Ok(buffer)
    })
}

/// 下载文件并计算 SHA256（带重试，失败会重建目标文件）。
///
/// # Arguments
/// * `agent` - HTTP 客户端
/// * `url` - 资产地址
/// * `token` - GitHub Token
/// * `destination` - 落盘路径
/// * `max_bytes` - 大小上限
///
/// # Returns
/// `(字节数, 小写十六进制 SHA256)`。
///
/// # Errors
/// 网络/写盘/超限失败时返回对应错误码。
pub fn download_to(
    agent: &Agent,
    url: &str,
    token: Option<&str>,
    destination: &Path,
    max_bytes: usize,
) -> Result<(u64, String)> {
    with_retry(|| {
        let mut request = agent
            .get(url)
            .set("User-Agent", USER_AGENT)
            .set("Accept", "application/octet-stream");
        if let Some(value) = token {
            if !value.trim().is_empty() {
                request = request.set("Authorization", &format!("Bearer {value}"));
            }
        }
        let response = request.call().map_err(map_ureq_error)?;
        let mut reader = response.into_reader();
        let mut file = File::create(destination)?;
        let mut hasher = Sha256::new();
        let mut total: u64 = 0;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            total += read as u64;
            // 边下边限流：不能等落盘完再判断，否则磁盘先被写满。
            if total > max_bytes as u64 {
                return Err(UpdateError::new(
                    codes::NET_TOO_LARGE,
                    format!("安装包超过上限 {max_bytes} 字节"),
                ));
            }
            hasher.update(&buffer[..read]);
            file.write_all(&buffer[..read])?;
        }
        file.flush()?;
        let hash = hasher
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        Ok((total, hash))
    })
}

/// 把 ureq 错误映射为固定错误码。
///
/// # Arguments
/// * `error` - ureq 错误
///
/// # Returns
/// 自更新错误。
///
/// # Errors
/// 不会失败。
fn map_ureq_error(error: ureq::Error) -> UpdateError {
    match error {
        ureq::Error::Status(code, response) => {
            let reason = response.status_text().to_owned();
            UpdateError::new(
                codes::NET_STATUS,
                format!("HTTP {code} {reason}"),
            )
        }
        ureq::Error::Transport(transport) => UpdateError::new(
            codes::NET_REQUEST,
            format!("网络请求失败：{transport}"),
        ),
    }
}

// =====** Release 解析 =====**

/// Release 资产。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseAsset {
    /// 资产名。
    pub name: String,
    /// 下载地址。
    pub url: String,
    /// 字节数（0 表示未知）。
    pub size: u64,
}

/// Release 摘要。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseInfo {
    /// 标签，如 `v0.1.2`。
    pub tag: String,
    /// 更新说明（Markdown）。
    pub body: String,
    /// 资产列表。
    pub assets: Vec<ReleaseAsset>,
}

impl ReleaseInfo {
    /// 按名称查找资产。
    ///
    /// # Arguments
    /// * `name` - 资产名（精确匹配）
    ///
    /// # Returns
    /// 找到时返回资产引用。
    ///
    /// # Errors
    /// 不会失败。
    pub fn asset(&self, name: &str) -> Option<&ReleaseAsset> {
        self.assets.iter().find(|asset| asset.name == name)
    }
}

/// GitHub Release 原始 JSON（只取需要的字段）。
#[derive(Debug, Deserialize)]
struct RawRelease {
    #[serde(default)]
    tag_name: String,
    #[serde(default)]
    body: String,
    #[serde(default)]
    assets: Vec<RawAsset>,
}

/// 资产原始 JSON。
#[derive(Debug, Deserialize)]
struct RawAsset {
    #[serde(default)]
    name: String,
    #[serde(default)]
    browser_download_url: String,
    #[serde(default)]
    size: u64,
}

/// 读取最新 Release。
///
/// # Arguments
/// * `agent` - HTTP 客户端
/// * `api_url` - Release API 地址（如 `https://api.github.com/repos/o/r/releases/latest`）
/// * `token` - GitHub Token（私有仓库需要）
///
/// # Returns
/// 解析后的 release。
///
/// # Errors
/// 网络失败：[`codes::NET_REQUEST`]；JSON 非法：[`codes::JSON_INVALID`]。
pub fn fetch_release(agent: &Agent, api_url: &str, token: Option<&str>) -> Result<ReleaseInfo> {
    let text = fetch_text(agent, api_url, token, MAX_TEXT_BYTES)?;
    let raw: RawRelease = serde_json::from_str(&text).map_err(|error| {
        UpdateError::new(
            codes::JSON_INVALID,
            format!("Release JSON 解析失败：{error}"),
        )
    })?;
    if raw.tag_name.trim().is_empty() {
        return Err(UpdateError::new(
            codes::JSON_INVALID,
            "Release 缺少 tag_name",
        ));
    }
    Ok(ReleaseInfo {
        tag: raw.tag_name,
        body: raw.body,
        assets: raw
            .assets
            .into_iter()
            .map(|asset| ReleaseAsset {
                name: asset.name,
                url: asset.browser_download_url,
                size: asset.size,
            })
            .collect(),
    })
}
