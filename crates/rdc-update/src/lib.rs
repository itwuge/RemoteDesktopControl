//! rdc-update / 软件自更新能力层
//!
//! 层级：L2 能力层（文件系统 + 网络 + 密码学校验，不含 UI）
//! 用途：从 GitHub Releases 读取版本与资产，验证 Ed25519 发布清单与 SHA256，
//!       下载暂存，调用安装器升级，失败时回滚到旧版本。
//! 调用方：`rdc-updater` CLI（Flutter 设置页通过进程调用）、发布流水线（sign/genkey）。
//!
//! 信任模型：
//! - 发布公钥**编译期内置**（[`RELEASE_PUBLIC_KEY_B64`]），可通过环境变量覆盖用于测试；
//! - 清单签名覆盖清单原始字节，安装包哈希写在已签名清单里；
//! - `.sha256` 资产必须存在且与清单一致（第二道交叉校验）；
//! - 任何一项校验失败都不得执行安装包。

pub mod apply;
pub mod error;
pub mod github;
pub mod hash;
pub mod manifest;
pub mod stage;
pub mod version;

pub use apply::{
    apply_update, backup_install_dir, read_build_version, restore_backup, ApplyOutcome,
    CODE_APPLY_OK,
};
pub use error::{codes, Result, UpdateError};
pub use manifest::{Artifact, UpdateManifest};
pub use stage::{check, download, staging_root, StagedUpdate, UpdateCheck};

/// 默认 Release 仓库（`owner/name`）。
///
/// **这是 owner/name 的唯一声明处**：上线前只改这一行，或设置环境变量
/// `RDC_UPDATE_REPO=owner/name`；完整 API 地址仍可用 `RDC_UPDATE_API`
/// 覆盖（优先级最高，便于自建镜像/测试服务器）。
pub const DEFAULT_REPO_SLUG: &str = "rdc-team/RemoteDesktopControl";

/// 由仓库 slug 拼接 GitHub Releases latest 接口地址。
///
/// # Arguments
/// * `slug` - `owner/name` 形式的仓库标识
///
/// # Returns
/// GitHub API 地址（不含 token）。
///
/// # Errors
/// 不会失败（slug 为空时会在请求阶段报错，便于暴露配置问题）。
pub fn release_api_for_repo(slug: &str) -> String {
    format!(
        "https://api.github.com/repos/{}/releases/latest",
        slug.trim()
    )
}

/// 发布清单验签公钥（Base64，32 字节 Ed25519）。
///
/// 该常量由 `rdc-updater genkey` 生成后人工写入；私钥种子只存放在 CI Secret
/// `RDC_RELEASE_SEED_B64` 与离线备份中，禁止提交到仓库。
/// 空字符串表示“开发构建未配置发布公钥”，此时所有更新都会被拒绝（安全默认）。
pub const RELEASE_PUBLIC_KEY_B64: &str = "+Xs4sevWTxyTADWQ3LLjlyglnsD4TpFurtFMuql8dTA=";

/// 当前编译进二进制的版本号（来源：根 `Cargo.toml` 的 workspace.package.version）。
///
/// # Returns
/// 语义化版本字符串。
///
/// # Errors
/// 不会失败。
pub fn current_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// 解析实际使用的 Release API 地址。
///
/// # Arguments
/// * `explicit` - 调用方显式传入的地址（优先级最高）
///
/// # Returns
/// 生效的 API 地址。
///
/// # Errors
/// 不会失败。
pub fn resolve_api_url(explicit: Option<&str>) -> String {
    if let Some(value) = explicit {
        if !value.trim().is_empty() {
            return value.trim().to_owned();
        }
    }
    if let Ok(value) = std::env::var("RDC_UPDATE_API") {
        if !value.trim().is_empty() {
            return value.trim().to_owned();
        }
    }
    if let Ok(value) = std::env::var("RDC_UPDATE_REPO") {
        if !value.trim().is_empty() {
            return release_api_for_repo(&value);
        }
    }
    release_api_for_repo(DEFAULT_REPO_SLUG)
}

/// 解析实际使用的发布公钥。
///
/// # Arguments
/// * `explicit` - 调用方显式传入的公钥（优先级最高）
///
/// # Returns
/// 生效的公钥（可能为空，表示未配置）。
///
/// # Errors
/// 不会失败。
pub fn resolve_public_key(explicit: Option<&str>) -> String {
    if let Some(value) = explicit {
        if !value.trim().is_empty() {
            return value.trim().to_owned();
        }
    }
    if let Ok(value) = std::env::var("RDC_UPDATE_PUBLIC_KEY") {
        if !value.trim().is_empty() {
            return value.trim().to_owned();
        }
    }
    RELEASE_PUBLIC_KEY_B64.trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// owner/name 只在 DEFAULT_REPO_SLUG 一处声明，拼出的地址必须符合 GitHub latest 接口。
    #[test]
    fn repo_slug_builds_latest_api_url() {
        assert_eq!(
            release_api_for_repo("acme/RemoteDesktopControl"),
            "https://api.github.com/repos/acme/RemoteDesktopControl/releases/latest"
        );
        // 默认值必须能被解析成合法地址（不依赖网络）。
        assert!(release_api_for_repo(DEFAULT_REPO_SLUG).starts_with("https://api.github.com/repos/"));
        assert!(release_api_for_repo(DEFAULT_REPO_SLUG).ends_with("/releases/latest"));
    }

    /// 显式传入的地址优先级最高：便于测试服务器/镜像覆盖，不写死在代码深处。
    #[test]
    fn explicit_api_url_wins() {
        assert_eq!(
            resolve_api_url(Some("http://127.0.0.1:9000/release")),
            "http://127.0.0.1:9000/release"
        );
    }
}
