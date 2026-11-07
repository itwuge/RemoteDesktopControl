//! rdc-update / 语义化版本比较
//!
//! 层级：L2 能力层
//! 用途：判断远端版本是否高于本端；拒绝非法版本号（避免“升级到垃圾版本”）。
//! 调用方：rdc-update 的 stage/CLI、Flutter 更新服务（通过 CLI 间接使用）。

use semver::Version;

use crate::error::{codes, Result, UpdateError};

/// 解析版本文本（容忍前缀 `v`，GitHub tag 常见写法）。
///
/// # Arguments
/// * `input` - 版本文本，如 `v0.1.2` / `0.1.2-beta.1`
///
/// # Returns
/// 解析后的 [`Version`]。
///
/// # Errors
/// 非法版本返回 [`codes::VER_INVALID`]。
pub fn parse(input: &str) -> Result<Version> {
    let trimmed = input.trim();
    let stripped = trimmed.strip_prefix('v').unwrap_or(trimmed);
    Version::parse(stripped).map_err(|error| {
        UpdateError::new(
            codes::VER_INVALID,
            format!("版本号非法：{input}（{error}）"),
        )
    })
}

/// 判断 `latest` 是否比 `current` 新。
///
/// # Arguments
/// * `latest` - 远端版本
/// * `current` - 本端版本
///
/// # Returns
/// 远端更新返回 `true`；相同或更低返回 `false`。
///
/// # Errors
/// 任一版本非法时返回 [`codes::VER_INVALID`]。
pub fn is_newer(latest: &str, current: &str) -> Result<bool> {
    Ok(parse(latest)? > parse(current)?)
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 常规递增必须成立。
    #[test]
    fn compares_numeric_segments() {
        assert!(is_newer("0.1.2", "0.1.1").expect("比较"));
        assert!(is_newer("0.2.0", "0.1.9").expect("比较"));
        assert!(!is_newer("0.1.1", "0.1.1").expect("比较"));
        assert!(!is_newer("0.1.0", "0.1.1").expect("比较"));
    }

    /// 预发布版本低于同号正式版，并容忍 `v` 前缀。
    #[test]
    fn handles_prerelease_and_prefix() {
        assert!(is_newer("v1.0.0", "1.0.0-beta").expect("比较"));
        assert!(!is_newer("1.0.0-beta", "1.0.0").expect("比较"));
    }

    /// 非法版本必须报固定错误码。
    #[test]
    fn rejects_invalid_version() {
        let error = parse("not-a-version").expect_err("必须失败");
        assert_eq!(error.code, codes::VER_INVALID);
    }
}
