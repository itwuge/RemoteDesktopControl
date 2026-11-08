//! rdc-update / 发布清单模型与验签
//!
//! 层级：L2 能力层
//! 用途：定义 `update-manifest.json` 的字段，并用 Ed25519（复用 rdc-security 的
//!       身份签名实现）对“清单原始字节”做 detached 验签。
//! 调用方：stage（下载前校验）、rdc-updater CLI 的 sign 子命令。
//!
//! 安全口径：
//! - 签名覆盖清单的**原始字节**，不做任何重新序列化（避免字段顺序/空格导致签名失效）；
//! - 安装包名禁止包含路径分隔符（阻断 `..\` 穿越）；
//! - 安装包哈希必须与清单一致，`.sha256` 资产只是第二道交叉校验。

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Deserialize, Serialize};

use crate::error::{codes, Result, UpdateError};
use crate::hash::is_sha256_hex;
use crate::version;

/// 清单中的安装包条目。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Artifact {
    /// 文件名（不含路径），如 `RDC-0.1.2-windows-x64-setup.exe`。
    pub name: String,
    /// SHA256（小写十六进制，64 字符）。
    pub sha256: String,
    /// 字节数（0 表示未提供，只做展示）。
    #[serde(default)]
    pub size: u64,
    /// 备用下载地址（可空；实际优先使用 Release 资产地址）。
    #[serde(default)]
    pub url: String,
}

/// 更新清单。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateManifest {
    /// 产品标识，必须为 `RDC`。
    pub product: String,
    /// 目标版本。
    pub version: String,
    /// 安装包条目。
    pub installer: Artifact,
    /// 更新说明（可空）。
    #[serde(default)]
    pub notes: String,
    /// 允许直接升级的最低版本（可空表示不限制）。
    #[serde(default)]
    pub min_supported_version: String,
}

/// 解析并验证清单。
///
/// # Arguments
/// * `bytes` - 清单原始字节（必须与签名时完全一致）
/// * `signature_b64` - detached 签名（Base64，64 字节）
/// * `public_key_b64` - 发布公钥（Base64，32 字节）
///
/// # Returns
/// 验签且字段合法时返回清单。
///
/// # Errors
/// - 未配置公钥：[`codes::SIG_KEY_MISSING`]
/// - 签名格式非法：[`codes::SIG_INVALID`]
/// - 验签失败：[`codes::SIG_UNVERIFIED`]
/// - JSON/字段非法：[`codes::MANIFEST_INVALID`]
pub fn parse_and_verify(
    bytes: &[u8],
    signature_b64: &str,
    public_key_b64: &str,
) -> Result<UpdateManifest> {
    if public_key_b64.trim().is_empty() {
        return Err(UpdateError::new(
            codes::SIG_KEY_MISSING,
            "本端未配置发布公钥，拒绝安装任何更新",
        ));
    }
    // 先单独校验 Base64：把“格式错”和“验签不通过”区分开，排查更快。
    if STANDARD.decode(signature_b64.trim()).is_err() {
        return Err(UpdateError::new(
            codes::SIG_INVALID,
            "更新清单签名不是合法 Base64",
        ));
    }
    rdc_security::verify_signature_b64(public_key_b64, bytes, signature_b64.trim()).map_err(
        |error| {
            UpdateError::new(
                codes::SIG_UNVERIFIED,
                format!("更新清单验签失败（{error}）"),
            )
        },
    )?;

    let manifest: UpdateManifest = serde_json::from_slice(bytes).map_err(|error| {
        UpdateError::new(
            codes::MANIFEST_INVALID,
            format!("更新清单 JSON 解析失败：{error}"),
        )
    })?;
    validate(&manifest)?;
    Ok(manifest)
}

/// 校验清单字段的业务约束。
///
/// # Arguments
/// * `manifest` - 待校验清单
///
/// # Returns
/// 合法返回 `Ok(())`。
///
/// # Errors
/// 任一字段非法时返回 [`codes::MANIFEST_INVALID`]。
pub fn validate(manifest: &UpdateManifest) -> Result<()> {
    if !manifest.product.eq_ignore_ascii_case("RDC") {
        return Err(UpdateError::new(
            codes::MANIFEST_INVALID,
            format!("清单产品标识不是 RDC：{}", manifest.product),
        ));
    }
    version::parse(&manifest.version)?;
    let name = &manifest.installer.name;
    if name.trim().is_empty()
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
    {
        return Err(UpdateError::new(
            codes::MANIFEST_INVALID,
            format!("安装包文件名非法：{name}"),
        ));
    }
    let sha = manifest.installer.sha256.trim().to_lowercase();
    if !is_sha256_hex(&sha) {
        return Err(UpdateError::new(
            codes::MANIFEST_INVALID,
            "清单中的安装包 SHA256 格式非法",
        ));
    }
    if !manifest.installer.url.is_empty()
        && !manifest.installer.url.starts_with("https://")
    {
        return Err(UpdateError::new(
            codes::MANIFEST_INVALID,
            "清单下载地址必须是 https",
        ));
    }
    Ok(())
}

/// 用私钥种子对清单字节签名（发布流水线使用）。
///
/// # Arguments
/// * `bytes` - 清单原始字节
/// * `seed` - 32 字节 Ed25519 种子
///
/// # Returns
/// Base64 签名。
///
/// # Errors
/// 不会失败。
pub fn sign_bytes(bytes: &[u8], seed: &[u8; 32]) -> String {
    let identity = rdc_security::IdentityKeyPair::from_seed(seed);
    identity.sign_b64(bytes)
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造合法清单字节。
    fn sample() -> Vec<u8> {
        serde_json::to_vec(&UpdateManifest {
            product: "RDC".to_owned(),
            version: "0.1.2".to_owned(),
            installer: Artifact {
                name: "RDC-0.1.2-windows-x64-setup.exe".to_owned(),
                sha256: "a".repeat(64),
                size: 10,
                url: String::new(),
            },
            notes: "修几个问题".to_owned(),
            min_supported_version: String::new(),
        })
        .expect("序列化")
    }

    /// 正确签名必须通过，篡改一个字节必须失败。
    #[test]
    fn verifies_and_detects_tamper() {
        let identity = rdc_security::IdentityKeyPair::generate();
        let bytes = sample();
        let sig = identity.sign_b64(&bytes);
        let manifest = parse_and_verify(&bytes, &sig, &identity.public_key_b64())
            .expect("验签通过");
        assert_eq!(manifest.version, "0.1.2");

        let mut tampered = bytes.clone();
        tampered[0] ^= 1;
        let error = parse_and_verify(&tampered, &sig, &identity.public_key_b64())
            .expect_err("篡改必须失败");
        assert_eq!(error.code, codes::SIG_UNVERIFIED);
    }

    /// 未配置公钥时必须拒绝，且错误码可区分。
    #[test]
    fn missing_key_is_rejected() {
        let error = parse_and_verify(&sample(), "AAAA", "").expect_err("必须失败");
        assert_eq!(error.code, codes::SIG_KEY_MISSING);
    }

    /// 路径穿越文件名必须被拒绝。
    #[test]
    fn rejects_path_traversal_name() {
        let mut manifest: UpdateManifest =
            serde_json::from_slice(&sample()).expect("解析");
        manifest.installer.name = "..\\evil.exe".to_owned();
        let error = validate(&manifest).expect_err("必须失败");
        assert_eq!(error.code, codes::MANIFEST_INVALID);
    }
}
