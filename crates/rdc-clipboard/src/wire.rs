//! rdc-clipboard / 文本载荷线格式
//!
//! 层级：L3 交互层
//! 用途：把一段文本编成“kind + SHA-256 + 长度 + UTF-8”的自校验载荷，供 P1 剪贴板通道传输。
//! 调用方：rdc-clipboard 同步状态机、rdc-core（放入 `StreamKind::Clipboard` 队列）
//!
//! 布局（小端，共 `37 + len` 字节）：
//! ```text
//! 0        kind            u8     （0=文本，1=图片，2=文件，后两者本轮未实现）
//! 1..33    sha256(text)    32B
//! 33..37   payload_len     u32
//! 37..     utf-8 bytes     payload_len
//! ```
//! 自校验目的：先验长度再验哈希，损坏包在任何一层都能被拒绝，避免把乱码写进用户剪贴板。

use sha2::{Digest, Sha256};

use crate::error::ClipboardError;
use crate::text::MAX_TEXT_BYTES;

// =====** 常量 =====**

/// 文本载荷 kind。
pub const KIND_TEXT: u8 = 0;
/// 图片载荷 kind（未实现，仅占位）。
pub const KIND_IMAGE: u8 = 1;
/// 文件载荷 kind（未实现，仅占位）。
pub const KIND_FILES: u8 = 2;

/// SHA-256 长度。
pub const HASH_LEN: usize = 32;
/// 头部长度：kind + hash + 长度。
pub const HEADER_LEN: usize = 1 + HASH_LEN + 4;
/// 文本载荷最大字节数（头部 + 1 MiB 文本）。
pub const MAX_PAYLOAD_BYTES: usize = HEADER_LEN + MAX_TEXT_BYTES;

// =====** 摘要 =====**

/// 计算文本的 SHA-256。
///
/// # Arguments
/// * `bytes` - 已归一化为 LF 的 UTF-8 字节
///
/// # Returns
/// 32 字节摘要，用于去重与自校验。
///
/// # Errors
/// 不会失败。
pub fn digest(bytes: &[u8]) -> [u8; HASH_LEN] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

// =====** 编码 =====**

/// 编码文本载荷（内部算哈希）。
///
/// # Arguments
/// * `text` - 已归一化为 LF 的文本
///
/// # Returns
/// 自校验载荷，长度 `HEADER_LEN + text.len()`。
///
/// # Errors
/// 文本超过 [`MAX_TEXT_BYTES`] 时返回 [`ClipboardError::TooLarge`]。
pub fn encode_text(text: &str) -> Result<Vec<u8>, ClipboardError> {
    let hash = digest(text.as_bytes());
    encode_text_with_hash(text, &hash)
}

/// 编码文本载荷（复用已算好的哈希，避免同步状态机重复计算）。
///
/// # Arguments
/// * `text` - 已归一化为 LF 的文本
/// * `hash` - 该文本的 SHA-256
///
/// # Returns
/// 自校验载荷。
///
/// # Errors
/// 文本超过 [`MAX_TEXT_BYTES`] 时返回 [`ClipboardError::TooLarge`]。
pub(crate) fn encode_text_with_hash(
    text: &str,
    hash: &[u8; HASH_LEN],
) -> Result<Vec<u8>, ClipboardError> {
    let size = text.len();
    if size > MAX_TEXT_BYTES {
        return Err(ClipboardError::TooLarge {
            size,
            limit: MAX_TEXT_BYTES,
        });
    }
    let mut out = Vec::with_capacity(HEADER_LEN + size);
    out.push(KIND_TEXT);
    out.extend_from_slice(hash);
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(text.as_bytes());
    Ok(out)
}

// =====** 解码 =====**

/// 解码剪贴板载荷。
///
/// # Arguments
/// * `bytes` - 完整的单条载荷（不含传输层包头）
///
/// # Returns
/// `(文本, 哈希)`；文本为 LF 归一化后的 UTF-8。
///
/// # Errors
/// 长度不符/截断返回 [`ClipboardError::MalformedPayload`]；kind 为图片或文件返回
/// [`ClipboardError::Unsupported`]；哈希不符返回 [`ClipboardError::HashMismatch`]。
pub fn decode_text(bytes: &[u8]) -> Result<(String, [u8; HASH_LEN]), ClipboardError> {
    let (kind, rest) = bytes
        .split_first()
        .ok_or(ClipboardError::MalformedPayload)?;
    match *kind {
        KIND_TEXT => {}
        KIND_IMAGE | KIND_FILES => return Err(ClipboardError::Unsupported),
        _ => return Err(ClipboardError::MalformedPayload),
    }
    if rest.len() < HASH_LEN + 4 {
        return Err(ClipboardError::MalformedPayload);
    }
    // 上方已校验 `rest.len() >= HASH_LEN + 4`，该切片必为 HASH_LEN 字节，无失败分支。
    let mut hash = [0u8; HASH_LEN];
    hash.copy_from_slice(&rest[..HASH_LEN]);
    let len = u32::from_le_bytes([
        rest[HASH_LEN],
        rest[HASH_LEN + 1],
        rest[HASH_LEN + 2],
        rest[HASH_LEN + 3],
    ]) as usize;
    let body = &rest[HASH_LEN + 4..];
    if body.len() != len || len > MAX_TEXT_BYTES {
        return Err(ClipboardError::MalformedPayload);
    }
    let text = std::str::from_utf8(body)
        .map_err(|error| ClipboardError::MalformedText { source: error })?
        .to_owned();
    if digest(text.as_bytes()) != hash {
        return Err(ClipboardError::HashMismatch);
    }
    Ok((text, hash))
}

// =====** 自检 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 文本载荷往返一致，尾部长度与头部声明一致。
    #[test]
    fn text_roundtrip() {
        let text = "你好\nworld 🙂";
        let bytes = encode_text(text).expect("编码");
        assert_eq!(bytes.len(), HEADER_LEN + text.len());
        let (decoded, hash) = decode_text(&bytes).expect("解码");
        assert_eq!(decoded, text);
        assert_eq!(hash, digest(text.as_bytes()));
        // 再编一次必须逐字节一致。
        assert_eq!(encode_text(&decoded).expect("再编码"), bytes);
    }

    /// 空文本也是合法内容（用户复制空串不会发生，但解码不得 panic）。
    #[test]
    fn empty_text_roundtrip() {
        let bytes = encode_text("").expect("编码");
        let (decoded, _) = decode_text(&bytes).expect("解码");
        assert_eq!(decoded, "");
    }

    /// 篡改任意一字节 → 哈希不匹配。
    #[test]
    fn corrupted_body_is_detected() {
        let mut bytes = encode_text("hello").expect("编码");
        *bytes.last_mut().expect("非空") = b'x';
        assert!(matches!(
            decode_text(&bytes),
            Err(ClipboardError::HashMismatch)
        ));
    }

    /// 截断、超长、未知 kind、图片/文件 kind 都按约定拒绝。
    #[test]
    fn malformed_and_unsupported_payloads() {
        assert!(matches!(
            decode_text(&[]),
            Err(ClipboardError::MalformedPayload)
        ));
        let bytes = encode_text("hello").expect("编码");
        assert!(matches!(
            decode_text(&bytes[..bytes.len() - 1]),
            Err(ClipboardError::MalformedPayload)
        ));
        let mut long = bytes.clone();
        long.push(b'!');
        assert!(matches!(
            decode_text(&long),
            Err(ClipboardError::MalformedPayload)
        ));
        let mut unknown = bytes.clone();
        unknown[0] = 9;
        assert!(matches!(
            decode_text(&unknown),
            Err(ClipboardError::MalformedPayload)
        ));
        for kind in [KIND_IMAGE, KIND_FILES] {
            let mut payload = bytes.clone();
            payload[0] = kind;
            assert!(matches!(
                decode_text(&payload),
                Err(ClipboardError::Unsupported)
            ));
        }
    }

    /// 超过 1 MiB 的文本在编码阶段被拒绝，不做截断。
    #[test]
    fn oversize_text_is_rejected() {
        let text = "a".repeat(MAX_TEXT_BYTES + 1);
        assert!(matches!(
            encode_text(&text),
            Err(ClipboardError::TooLarge { .. })
        ));
        let ok = "a".repeat(MAX_TEXT_BYTES);
        assert!(encode_text(&ok).is_ok());
    }

    /// 非 UTF-8 字节序列必须拒绝，不能产生替换字符。
    #[test]
    fn invalid_utf8_is_rejected() {
        let mut bytes = encode_text("x").expect("编码");
        // 把正文改成非法 UTF-8，并把长度改回 1 以外的值以绕过长度检查。
        bytes.truncate(HEADER_LEN);
        let body = [0xFFu8, 0xFF];
        let len = body.len() as u32;
        bytes[HEADER_LEN - 4..HEADER_LEN].copy_from_slice(&len.to_le_bytes());
        bytes.extend_from_slice(&body);
        // 重算哈希，确保失败原因是 UTF-8 而不是哈希。
        let hash = digest(&body);
        bytes[1..1 + HASH_LEN].copy_from_slice(&hash);
        assert!(matches!(
            decode_text(&bytes),
            Err(ClipboardError::MalformedText { .. })
        ));
    }
}
