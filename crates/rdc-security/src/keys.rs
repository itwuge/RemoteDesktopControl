//! rdc-security / 密钥材料与协商
//!
//! 层级：L2 能力层
//! 用途：提供长期身份密钥对与每会话临时密钥的生成、公钥编码、指纹计算与协商接口；
//!       S1 只保留脱敏 Debug 与函数签名，密码学实现推迟到 S2
//! 调用方：rdc-connect（建链前的身份与密钥交换）、rdc-core（信任列表比对与指纹展示）

use crate::error::SecurityError;

// =====** IdentityKeyPair =====**

/// 长期身份密钥对。
///
/// 私钥只以 32 字节 seed 保存在内存，不实现序列化；Debug 手写为脱敏输出，
/// 因为 derive(Debug) 会把数组内容原样打印进日志。
pub struct IdentityKeyPair {
    /// 32 字节身份私钥种子；禁止任何形式的 Debug / 序列化输出，也不跨 FFI 传递。
    seed: [u8; 32],
}

impl std::fmt::Debug for IdentityKeyPair {
    /// 脱敏 Debug 输出，固定为 `IdentityKeyPair(<redacted>)`。
    ///
    /// # Arguments
    /// * `f` - 目标格式化器
    ///
    /// # Returns
    /// 格式化结果。
    ///
    /// # Errors
    /// 仅在格式化器写入失败时返回 `std::fmt::Error`。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // 为什么是固定字符串：宁可牺牲可观测性，也不能让私钥字节进入日志或崩溃上报。
        f.write_str("IdentityKeyPair(<redacted>)")
    }
}

impl IdentityKeyPair {
    /// 生成新的身份密钥对。
    ///
    /// # Returns
    /// 新建的身份密钥对；S1 尚未实现，调用即 panic（见 `# Panics`）。
    ///
    /// # Errors
    /// 不会失败。
    ///
    /// # Panics
    /// S1 空壳：实现落地前调用会 panic；S2 起不再 panic。
    pub fn generate() -> Self {
        todo!("S1 空壳：身份密钥在 S2 实现")
    }

    /// 返回 Base64 编码的公钥。
    ///
    /// # Returns
    /// 标准 Base64（非 URL 安全变体）编码的 Ed25519 公钥，供信令注册与对端验签使用。
    ///
    /// # Errors
    /// 不会失败。
    ///
    /// # Panics
    /// S1 空壳：实现落地前调用会 panic。
    pub fn public_key_b64(&self) -> String {
        todo!("S1 空壳：公钥编码在 S2 实现")
    }

    /// 返回公钥指纹。
    ///
    /// # Returns
    /// 公钥哈希的十六进制短串，供用户肉眼核对设备身份。
    ///
    /// # Errors
    /// 不会失败。
    ///
    /// # Panics
    /// S1 空壳：实现落地前调用会 panic。
    pub fn fingerprint(&self) -> String {
        todo!("S1 空壳：指纹计算在 S2 实现")
    }
}

// =====** EphemeralKey =====**

/// 每会话临时密钥（X25519 标量），会话结束即随结构体丢弃。
pub struct EphemeralKey {
    /// 32 字节临时私钥种子；只在内存中存在，不做持久化。
    seed: [u8; 32],
}

impl EphemeralKey {
    /// 生成新的临时密钥。
    ///
    /// # Returns
    /// 新建的临时密钥；S1 尚未实现。
    ///
    /// # Errors
    /// 不会失败。
    ///
    /// # Panics
    /// S1 空壳：实现落地前调用会 panic。
    pub fn generate() -> Self {
        todo!("S1 空壳：临时密钥在 S2 实现")
    }

    /// 与对端临时公钥协商共享密钥。
    ///
    /// # Arguments
    /// * `peer_public_b64` - 对端 Base64 编码的临时公钥
    ///
    /// # Returns
    /// 32 字节共享密钥，由上层经 KDF 派生成 AEAD 密钥与握手确认值。
    ///
    /// # Errors
    /// 公钥不是合法 Base64 或长度不符时返回 [`SecurityError::InvalidKeyLength`]；
    /// 协商得出的密钥不可用（低阶点等）时返回 [`SecurityError::HandshakeStateInvalid`]。
    ///
    /// # Panics
    /// S1 空壳：实现落地前调用会 panic。
    pub fn exchange_secret(&self, _peer_public_b64: &str) -> Result<[u8; 32], SecurityError> {
        todo!("S1 空壳：临时密钥协商在 S2 实现")
    }
}
