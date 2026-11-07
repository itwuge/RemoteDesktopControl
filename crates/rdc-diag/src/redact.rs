//! ============================================================
//! rdc-diag · 敏感字段脱敏
//!
//! 层级：L1 基础设施（rdc-diag 内部模块）。
//! 用途：统一过滤密钥/令牌/剪贴板正文等敏感内容，并对 IP 打码、对摘要取前 8 位，
//!       防止日志把凭据或用户内容带出去（docs/16 §7.3）。
//! 调用方：各 crate 在写日志前调用；新增字段必须过 `SENSITIVE_KEYS`，否则 code review 拒。
//! 纪律：本模块只做“能不能写”的判定与替换，不做加密，也不保证可逆。
//! ============================================================

// =====** 模块导入 =====**

use sha2::{Digest, Sha256};

// =====** 敏感 key 表 =====**

/// 命中即禁止记录原文的 key 片段（大小写不敏感，子串匹配）。
///
/// 采用子串匹配而不是全等：字段名常见 `access_token`、`peer_private_key` 这类组合。
pub const SENSITIVE_KEYS: &[&str] = &[
    "password",
    "passwd",
    "secret",
    "token",
    "authorization",
    "private_key",
    "pairing_code",
    "ticket",
    "clipboard",
    "clipboard_text",
    "content",
    "pixel",
    "ocr_text",
];

/// 脱敏后的占位符；固定三字符，避免通过长度侧信道推断原文。
pub const MASK: &str = "***";

// =====** 判定与替换 =====**

/// 判断字段名是否敏感（大小写不敏感子串匹配）。
pub fn is_sensitive_key(key: &str) -> bool {
    let lowered = key.to_ascii_lowercase();
    SENSITIVE_KEYS
        .iter()
        .any(|pattern| lowered.contains(pattern))
}

/// 对单个字段值做脱敏。
///
/// # Arguments
/// * `key` - 字段名；命中 [`SENSITIVE_KEYS`] 时直接替换为 [`MASK`]。
/// * `value` - 原始值；非敏感字段仍会做 IP 打码。
pub fn redact(key: &str, value: &str) -> String {
    if is_sensitive_key(key) {
        return MASK.to_string();
    }
    mask_ip(value)
}

/// 对 IP 地址打码。
///
/// * IPv4：保留前两段（`192.168.x.x`）；
/// * IPv6：保留前两组（`2001:db8::x`）；
/// * 非 IP 文本原样返回（避免误伤普通字段）。
pub fn mask_ip(value: &str) -> String {
    let text = value.trim();
    if is_ipv4(text) {
        let parts: Vec<&str> = text.split('.').collect();
        return format!("{}.{}.x.x", parts[0], parts[1]);
    }
    if text.contains(':') && text.split(':').count() >= 3 && is_ipv6_chars(text) {
        let parts: Vec<&str> = text.split(':').collect();
        // 保留前两组；其余用 `x` 占位，保持“是 IPv6”的可读性。
        return format!("{}:{}::x", parts[0], parts[1]);
    }
    value.to_string()
}

/// 计算摘要并只保留前 8 位十六进制（剪贴板/文件内容只允许写这个）。
pub fn short_hash(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    let mut text = String::with_capacity(8);
    for byte in digest.iter().take(4) {
        // 固定两位小写十六进制，保证长度恒为 8。
        text.push_str(&format!("{:02x}", byte));
    }
    text
}

// =====** 内部判定 =====**

/// 严格 IPv4 判定：四段十进制、每段 0..=255。
fn is_ipv4(text: &str) -> bool {
    let parts: Vec<&str> = text.split('.').collect();
    if parts.len() != 4 {
        return false;
    }
    parts.iter().all(|part| {
        !part.is_empty() && part.len() <= 3 && part.parse::<u16>().map(|v| v <= 255).unwrap_or(false)
    })
}

/// IPv6 字符集判定：只允许十六进制与冒号（含 `::`）。
fn is_ipv6_chars(text: &str) -> bool {
    text.chars()
        .all(|ch| ch.is_ascii_hexdigit() || ch == ':' || ch == '.' || ch == '[' || ch == ']')
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensitive_keys_cover_tokens_and_clipboard() {
        assert!(is_sensitive_key("access_token"));
        assert!(is_sensitive_key("Authorization"));
        assert!(is_sensitive_key("clipboard_text"));
        assert!(is_sensitive_key("peer_private_key"));
        assert!(!is_sensitive_key("frame_id"));
    }

    #[test]
    fn sensitive_value_never_leaks() {
        let secret = "Bearer abcdef123456";
        let masked = redact("authorization", secret);
        assert_eq!(masked, MASK);
        assert!(!masked.contains("abcdef"));
    }

    #[test]
    fn ipv4_is_masked_to_two_octets() {
        assert_eq!(mask_ip("192.168.31.77"), "192.168.x.x");
        assert_eq!(redact("peer", "10.0.0.5"), "10.0.x.x");
    }

    #[test]
    fn ipv6_is_masked_to_two_groups() {
        assert_eq!(mask_ip("2001:db8:1234:5678::1"), "2001:db8::x");
    }

    #[test]
    fn non_ip_text_is_untouched() {
        assert_eq!(mask_ip("capture.dxgi"), "capture.dxgi");
        assert_eq!(mask_ip("999.1.1.1"), "999.1.1.1");
    }

    #[test]
    fn short_hash_is_eight_hex_chars() {
        let hash = short_hash("clipboard body");
        assert_eq!(hash.len(), 8);
        assert!(hash.chars().all(|ch| ch.is_ascii_hexdigit()));
        assert_eq!(hash, short_hash("clipboard body"));
    }
}
