//! rdc-filetransfer / 续传状态位图编解码
//!
//! 层级：L3 交互层
//! 用途：把“哪些分块已收到”编码成可传输/可落盘的位图，重连后据此跳过已确认块。
//! 调用方：rdc-core（断线重连时交换/恢复状态）

use crate::error::FileTransferError;

/// 状态头长度：分块数 u32。
const HEADER_LEN: usize = 4;

// =====** 编解码 =====**

/// 编码已收分块位图。
///
/// # Arguments
/// * `received` - 按下标标记的已收状态
///
/// # Returns
/// `4 字节块数 + ceil(n/8) 字节位图`（小端）。
///
/// # Errors
/// 不会失败。
pub fn encode_state(received: &[bool]) -> Vec<u8> {
    let count = received.len();
    let mut out = Vec::with_capacity(HEADER_LEN + count.div_ceil(8));
    out.extend_from_slice(&(count as u32).to_le_bytes());
    let mut byte = 0u8;
    for (i, &hit) in received.iter().enumerate() {
        if hit {
            byte |= 1 << (i % 8);
        }
        if i % 8 == 7 {
            out.push(byte);
            byte = 0;
        }
    }
    if count % 8 != 0 {
        out.push(byte);
    }
    out
}

/// 解码已收分块位图。
///
/// # Arguments
/// * `bytes` - [`encode_state`] 的输出
/// * `count` - 期望的分块数（来自 [`ChunkPlan`](crate::ChunkPlan)）
///
/// # Returns
/// 长度为 `count` 的已收状态。
///
/// # Errors
/// 头部缺失、块数与 `count` 不符或位图长度不符时返回
/// [`FileTransferError::MalformedState`]。
pub fn decode_state(bytes: &[u8], count: u32) -> Result<Vec<bool>, FileTransferError> {
    if bytes.len() < HEADER_LEN {
        return Err(FileTransferError::MalformedState);
    }
    let declared = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    if declared != count {
        return Err(FileTransferError::MalformedState);
    }
    let bitmap = &bytes[HEADER_LEN..];
    let expected_len = (count as usize).div_ceil(8);
    if bitmap.len() != expected_len {
        return Err(FileTransferError::MalformedState);
    }
    Ok((0..count as usize)
        .map(|i| bitmap[i / 8] & (1 << (i % 8)) != 0)
        .collect())
}

// =====** 自检 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 任意长度位图往返一致。
    #[test]
    fn roundtrip() {
        for count in 0usize..=17 {
            let received: Vec<bool> = (0..count).map(|i| i % 3 == 0).collect();
            let bytes = encode_state(&received);
            assert_eq!(bytes.len(), 4 + count.div_ceil(8));
            let decoded = decode_state(&bytes, count as u32).expect("解码");
            assert_eq!(decoded, received, "count={count}");
        }
    }

    /// 块数不符、位图长度不符、头部截断都拒绝。
    #[test]
    fn malformed_state_is_rejected() {
        let bytes = encode_state(&[true, false, true]);
        assert!(matches!(
            decode_state(&bytes, 4),
            Err(FileTransferError::MalformedState)
        ));
        assert!(matches!(
            decode_state(&bytes[..bytes.len() - 1], 3),
            Err(FileTransferError::MalformedState)
        ));
        assert!(matches!(
            decode_state(&[1, 2], 0),
            Err(FileTransferError::MalformedState)
        ));
    }
}
