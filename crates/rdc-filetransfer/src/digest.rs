//! rdc-filetransfer / SHA-256 工具
//!
//! 层级：L3 交互层
//! 用途：分块与整文件的内容摘要（断点续传的前置校验）
//! 调用方：rdc-filetransfer 的发送端、接收端与分块构造

use std::fs::File;
use std::io::Read;
use std::path::Path;

use sha2::{Digest, Sha256};

use crate::error::FileTransferError;

/// 整文件摘要读取缓冲（64 KiB，兼顾吞吐与内存）。
const HASH_BUFFER_BYTES: usize = 64 * 1024;

// =====** 摘要 =====**

/// 计算字节摘要。
///
/// # Arguments
/// * `bytes` - 待摘要内容
///
/// # Returns
/// 32 字节 SHA-256。
pub fn hash_bytes(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize().into()
}

/// 流式计算文件摘要（不整体读入内存）。
///
/// # Arguments
/// * `path` - 文件路径
///
/// # Returns
/// 32 字节 SHA-256。
///
/// # Errors
/// 打开或读取失败时返回 [`FileTransferError::Io`]。
pub fn hash_file(path: &Path) -> Result<[u8; 32], FileTransferError> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; HASH_BUFFER_BYTES];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().into())
}

// =====** 自检 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 与已知向量一致（空串 / "abc"）。
    #[test]
    fn known_vectors() {
        assert_eq!(
            hash_bytes(b""),
            hex(b"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855")
        );
        assert_eq!(
            hash_bytes(b"abc"),
            hex(b"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")
        );
    }

    /// 文件摘要与字节摘要一致。
    #[test]
    fn file_hash_matches_bytes() {
        let path = crate::tests_support::temp_path("hash");
        std::fs::write(&path, b"abc").expect("写文件");
        assert_eq!(hash_file(&path).expect("摘要"), hash_bytes(b"abc"));
        let _ = std::fs::remove_file(&path); // gate-allow R-C-2: 用例收尾删除临时文件，失败不影响断言结论
    }

    /// 十六进制字面量 → 字节。
    fn hex(text: &[u8]) -> [u8; 32] {
        let mut out = [0u8; 32];
        for (i, pair) in text.chunks(2).enumerate() {
            let s = std::str::from_utf8(pair).expect("ascii");
            out[i] = u8::from_str_radix(s, 16).expect("hex");
        }
        out
    }
}
