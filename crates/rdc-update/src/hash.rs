//! rdc-update / 哈希工具
//!
//! 层级：L2 能力层
//! 用途：安装包字节流与落盘文件的 SHA256 计算，供下载校验与升级前复检共用。
//! 调用方：stage、apply、rdc-updater CLI。

use std::fs::File;
use std::io::{BufReader, Read};

use sha2::{Digest, Sha256};

use crate::error::Result;

/// 计算字节内容的 SHA256（小写十六进制）。
///
/// # Arguments
/// * `bytes` - 原始字节
///
/// # Returns
/// 64 字符十六进制串。
///
/// # Errors
/// 不会失败。
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

/// 计算文件 SHA256 与大小（流式读取，避免大安装包占内存）。
///
/// # Arguments
/// * `path` - 文件路径
///
/// # Returns
/// `(字节数, 小写十六进制哈希)`。
///
/// # Errors
/// 文件不可读时返回 [`crate::error::codes::IO`]。
pub fn sha256_file(path: &std::path::Path) -> Result<(u64, String)> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut total: u64 = 0;
    loop {
        // 分块读取：安装包可能有上百 MiB，不能整体读入内存。
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total += read as u64;
        hasher.update(&buffer[..read]);
    }
    let digest = hasher.finalize();
    Ok((total, digest.iter().map(|b| format!("{b:02x}")).collect()))
}

/// 校验十六进制 SHA256 文本是否合法（64 位 0-9a-f）。
///
/// # Arguments
/// * `text` - 待校验文本
///
/// # Returns
/// 合法返回 `true`。
///
/// # Errors
/// 不会失败。
pub fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit())
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 空内容哈希必须与标准值一致。
    #[test]
    fn hashes_empty_bytes() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    /// 文件哈希与大小必须一致。
    #[test]
    fn hashes_file() {
        let path = std::env::temp_dir().join(format!("rdc-hash-{}", std::process::id()));
        std::fs::write(&path, b"abc").expect("写文件");
        let (size, hash) = sha256_file(&path).expect("计算");
        assert_eq!(size, 3);
        assert_eq!(
            hash,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = std::fs::remove_file(path);
    }
}
