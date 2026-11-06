//! rdc-security / AEAD 对称加密
//!
//! 层级：L2 能力层
//! 用途：持有 32 字节对称密钥并暴露 seal / open 接口；S1 只落库密钥，算法在 S2 接入
//! 调用方：rdc-connect（握手后建立加密通道）、rdc-transport（媒体分片加解密）

use crate::error::SecurityError;

// =====** AeadKey =====**

/// 32 字节对称密钥。
///
/// 刻意不实现 Debug / 序列化：密钥材料不得出现在日志、统计或线上报文里。
pub struct AeadKey([u8; 32]);

impl AeadKey {
    /// 用已有密钥字节构造密钥句柄。
    ///
    /// # Arguments
    /// * `key` - 32 字节密钥材料，通常来自握手阶段的 KDF 输出
    ///
    /// # Returns
    /// 持有该密钥的 `AeadKey`。
    ///
    /// # Errors
    /// 不会失败；长度由类型系统在编译期保证。
    pub fn new(key: [u8; 32]) -> Self {
        Self(key)
    }

    /// 加密并附加认证标签。
    ///
    /// # Arguments
    /// * `plaintext` - 待加密的明文
    /// * `nonce` - 12 字节 nonce，同一密钥下禁止复用
    ///
    /// # Returns
    /// 密文与认证标签拼接后的字节序列。
    ///
    /// # Errors
    /// 密文生成失败时返回 [`SecurityError::AeadFailure`]；密钥长度已在构造期校验。
    ///
    /// # Panics
    /// S1 空壳：实现落地前调用会 panic。
    pub fn seal(&self, _plaintext: &[u8], _nonce: &[u8; 12]) -> Result<Vec<u8>, SecurityError> {
        todo!("S1 空壳：AEAD seal 在 S2 实现")
    }

    /// 校验标签并解密。
    ///
    /// # Arguments
    /// * `ciphertext` - 密文与认证标签
    /// * `nonce` - 与加密时完全一致的 12 字节 nonce
    ///
    /// # Returns
    /// 解密后的明文。
    ///
    /// # Errors
    /// 标签校验失败、密文长度不足或 nonce 与发送端不一致时返回
    /// [`SecurityError::AeadFailure`]（不区分失败原因，避免侧信道泄漏）。
    ///
    /// # Panics
    /// S1 空壳：实现落地前调用会 panic。
    pub fn open(&self, _ciphertext: &[u8], _nonce: &[u8; 12]) -> Result<Vec<u8>, SecurityError> {
        todo!("S1 空壳：AEAD open 在 S2 实现")
    }
}
