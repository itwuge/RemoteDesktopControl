//! rdc-security / 身份密钥、临时密钥与分通道派生
//!
//! 层级：L2 能力层
//! 用途：Ed25519 长期身份（签名/验签）、X25519 临时密钥（每会话）、
//!       HKDF-SHA256 分通道派生（docs/07 §2、§3）。
//! 调用方：rdc-connect（建链握手）、rdc-core（指纹展示与信任列表）、
//!         rdc-cli（Demo 的身份初始化）
//!
//! 安全前提：私钥只在内存中存在，不实现 Debug / Serialize；跨 FFI 只传公钥。

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand_core::OsRng;
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroize;

use crate::error::SecurityError;

// =====** HMAC-SHA256（会话 token / 确认） =====**

/// HMAC-SHA256 类型别名。
type HmacSha256 = Hmac<Sha256>;

/// 计算 HMAC-SHA256 并返回 Base64。
///
/// 信令 token 校验与握手确认共用同一实现，避免两处细节不一致。
///
/// # Arguments
/// * `key` - 任意长度密钥（调用方通常传 32 字节 token）
/// * `payload` - 待认证内容（信令使用规范化信封字节）
///
/// # Returns
/// Base64 编码的 32 字节 HMAC。
///
/// # Errors
/// 不会失败（HMAC 接受任意长度密钥）。
pub fn hmac_sha256_b64(key: &[u8], payload: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC 支持任意长度密钥");
    mac.update(payload);
    STANDARD.encode(mac.finalize().into_bytes())
}

/// 常量时间校验 HMAC-SHA256。
///
/// # Arguments
/// * `key` - 密钥
/// * `payload` - 原始内容
/// * `mac_b64` - 对端提供的 Base64 HMAC
///
/// # Returns
/// 校验通过返回 `Ok(())`。
///
/// # Errors
/// Base64 非法或 HMAC 不匹配时返回 [`SecurityError::ConfirmationFailed`]。
pub fn verify_hmac_sha256_b64(key: &[u8], payload: &[u8], mac_b64: &str) -> Result<(), SecurityError> {
    let provided = STANDARD
        .decode(mac_b64)
        .map_err(|error| SecurityError::ConfirmationFailed {
            source: Some(Box::new(error)),
        })?;
    let mut mac = HmacSha256::new_from_slice(key).expect("HMAC 支持任意长度密钥");
    mac.update(payload);
    // `verify_slice` 内部为常量时间比较。
    mac.verify_slice(&provided)
        .map_err(|error| SecurityError::ConfirmationFailed {
            source: Some(Box::new(error)),
        })
}

// =====** Base64 辅助 =====**

/// 公钥/签名统一使用标准 Base64（非 URL 安全变体），避免与二维码深链冲突。
///
/// # Arguments
/// * `bytes` - 待编码字节
///
/// # Returns
/// Base64 文本。
///
/// # Errors
/// 不会失败。
pub fn to_b64(bytes: &[u8]) -> String {
    STANDARD.encode(bytes)
}

/// 解码 Base64 并校验长度。
///
/// # Arguments
/// * `text` - Base64 文本
/// * `expected` - 期望的解码后字节数
///
/// # Returns
/// 固定长度字节数组。
///
/// # Errors
/// Base64 非法或长度不符时返回 [`SecurityError::InvalidKeyLength`]。
pub fn from_b64_exact<const N: usize>(text: &str, expected: usize) -> Result<[u8; N], SecurityError> {
    let raw = STANDARD
        .decode(text)
        .map_err(|error| SecurityError::InvalidKeyLength {
            expected,
            actual: text.len(),
            source: Some(Box::new(error)),
        })?;
    if raw.len() != N || raw.len() != expected {
        return Err(SecurityError::InvalidKeyLength {
            expected,
            actual: raw.len(),
            source: None,
        });
    }
    let mut out = [0u8; N];
    out.copy_from_slice(&raw);
    Ok(out)
}

// =====** IdentityKeyPair =====**

/// 长期身份密钥对（Ed25519）。
///
/// 私钥由 `SigningKey` 内部持有并在 Drop 时清零；本结构不派生 Debug，
/// 防止误打印。
pub struct IdentityKeyPair {
    /// Ed25519 签名密钥。
    signing: SigningKey,
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
        // 为什么是固定字符串：私钥字节绝不能进入日志或崩溃上报。
        f.write_str("IdentityKeyPair(<redacted>)")
    }
}

impl IdentityKeyPair {
    /// 生成新的身份密钥对。
    ///
    /// # Returns
    /// 使用操作系统 CSPRNG 生成的新密钥对。
    ///
    /// # Errors
    /// 不会失败（OsRng 在极端环境下会 panic，Rust 生态的既定行为）。
    pub fn generate() -> Self {
        Self {
            signing: SigningKey::generate(&mut OsRng),
        }
    }

    /// 从 32 字节种子恢复（仅用于测试向量与持久化加载）。
    ///
    /// # Arguments
    /// * `seed` - 32 字节种子
    ///
    /// # Returns
    /// 对应密钥对。
    ///
    /// # Errors
    /// 不会失败；长度由类型系统保证。
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        Self {
            signing: SigningKey::from_bytes(seed),
        }
    }

    /// 返回 Base64 编码的公钥（Ed25519，32 字节）。
    ///
    /// # Returns
    /// 标准 Base64 文本，供信令注册与对端验签使用。
    ///
    /// # Errors
    /// 不会失败。
    pub fn public_key_b64(&self) -> String {
        to_b64(self.signing.verifying_key().as_bytes())
    }

    /// 返回公钥指纹（SHA-256 前 8 字节的十六进制，冒号分隔）。
    ///
    /// # Returns
    /// 形如 `AB:CD:EF:01:23:45:67:89` 的短串，供用户肉眼核对。
    ///
    /// # Errors
    /// 不会失败。
    pub fn fingerprint(&self) -> String {
        fingerprint_of(&self.signing.verifying_key())
    }

    /// 导出 32 字节种子（**仅用于本地持久化**）。
    ///
    /// 为什么需要：桌面端身份密钥必须跨进程稳定，否则每次启动都会换指纹，
    /// 已配对设备与已签发授权令牌全部失效。调用方必须把结果写入仅本用户可读
    /// 的位置，且不得进入日志/崩溃上报。
    ///
    /// # Returns
    /// Ed25519 私钥种子。
    ///
    /// # Errors
    /// 不会失败。
    pub fn to_seed_bytes(&self) -> [u8; 32] {
        self.signing.to_bytes()
    }

    /// 对字节内容签名。
    ///
    /// # Arguments
    /// * `payload` - 待签名字节（调用方必须使用协议定义的规范化内容）
    ///
    /// # Returns
    /// Base64 编码的 Ed25519 签名（64 字节）。
    ///
    /// # Errors
    /// 不会失败。
    pub fn sign_b64(&self, payload: &[u8]) -> String {
        to_b64(&self.signing.sign(payload).to_bytes())
    }

    /// 用本端私钥对应的公钥验签（自检/测试用）。
    ///
    /// # Arguments
    /// * `payload` - 原始内容
    /// * `sig_b64` - Base64 签名
    ///
    /// # Returns
    /// 验签通过返回 `Ok(())`。
    ///
    /// # Errors
    /// 签名格式非法或验签失败返回 [`SecurityError::SignatureInvalid`]。
    pub fn verify_b64(&self, payload: &[u8], sig_b64: &str) -> Result<(), SecurityError> {
        verify_signature_b64(&self.public_key_b64(), payload, sig_b64)
    }
}

/// 由 Base64 公钥计算指纹。
///
/// # Arguments
/// * `public_key_b64` - 32 字节 Ed25519 公钥（Base64）
///
/// # Returns
/// 形如 `AB:CD:...` 的 16 位十六进制指纹。
///
/// # Errors
/// Base64/长度/曲线点非法时返回 [`SecurityError::SignatureInvalid`]。
pub fn fingerprint_from_b64(public_key_b64: &str) -> Result<String, SecurityError> {
    let bytes: [u8; 32] = from_b64_exact(public_key_b64, 32)?;
    let key = VerifyingKey::from_bytes(&bytes).map_err(|error| {
        // 文案保持「签名校验失败」不变；底层曲线点错误挂在 source 上，不进 Display。
        SecurityError::SignatureInvalid {
            source: Some(Box::new(error)),
        }
    })?;
    Ok(fingerprint_of(&key))
}

/// 计算公钥指纹。
///
/// # Arguments
/// * `key` - Ed25519 公钥
///
/// # Returns
/// 冒号分隔的 16 位十六进制串。
///
/// # Errors
/// 不会失败。
pub fn fingerprint_of(key: &VerifyingKey) -> String {
    let digest = Sha256::digest(key.as_bytes());
    // 只取前 8 字节：够区分设备，又便于用户口述比对。
    digest[..8]
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(":")
}

/// 用 Base64 公钥验签。
///
/// # Arguments
/// * `public_key_b64` - Ed25519 公钥（Base64）
/// * `payload` - 原始内容
/// * `sig_b64` - 签名（Base64，64 字节）
///
/// # Returns
/// 验签通过返回 `Ok(())`。
///
/// # Errors
/// 公钥/签名长度非法或验签失败返回 [`SecurityError::SignatureInvalid`] /
/// [`SecurityError::InvalidKeyLength`]。
pub fn verify_signature_b64(
    public_key_b64: &str,
    payload: &[u8],
    sig_b64: &str,
) -> Result<(), SecurityError> {
    let key_bytes: [u8; 32] = from_b64_exact(public_key_b64, 32)?;
    let key = VerifyingKey::from_bytes(&key_bytes).map_err(|error| {
        SecurityError::SignatureInvalid {
            source: Some(Box::new(error)),
        }
    })?;
    let sig_bytes: [u8; 64] = from_b64_exact(sig_b64, 64).map_err(|error| {
        SecurityError::SignatureInvalid {
            source: Some(Box::new(error)),
        }
    })?;
    let signature = Signature::from_bytes(&sig_bytes);
    // 验签失败不区分原因，避免给攻击者提供额外的判别信息。
    key.verify(payload, &signature).map_err(|error| {
        SecurityError::SignatureInvalid {
            source: Some(Box::new(error)),
        }
    })
}

// =====** EphemeralKey =====**

/// 每会话临时 X25519 密钥，会话结束即析构。
pub struct EphemeralKey {
    /// X25519 私钥（StaticSecret 支持保存与多次使用，会话结束 Drop 清零）。
    secret: StaticSecret,
}

impl std::fmt::Debug for EphemeralKey {
    /// 脱敏 Debug 输出。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EphemeralKey(<redacted>)")
    }
}

impl EphemeralKey {
    /// 生成新的临时密钥。
    ///
    /// # Returns
    /// 随机 X25519 密钥。
    ///
    /// # Errors
    /// 不会失败。
    pub fn generate() -> Self {
        Self {
            secret: StaticSecret::random_from_rng(OsRng),
        }
    }

    /// 返回 Base64 编码的临时公钥（32 字节）。
    ///
    /// # Returns
    /// 标准 Base64 文本。
    ///
    /// # Errors
    /// 不会失败。
    pub fn public_key_b64(&self) -> String {
        to_b64(PublicKey::from(&self.secret).as_bytes())
    }

    /// 返回临时公钥原始字节（供签名拼接使用）。
    ///
    /// # Returns
    /// 32 字节公钥。
    ///
    /// # Errors
    /// 不会失败。
    pub fn public_key_bytes(&self) -> [u8; 32] {
        PublicKey::from(&self.secret).to_bytes()
    }

    /// 与对端临时公钥协商共享密钥。
    ///
    /// # Arguments
    /// * `peer_public_b64` - 对端 Base64 编码的临时公钥
    ///
    /// # Returns
    /// 32 字节共享密钥（X25519 输出），交给 [`derive_channel_keys`]。
    ///
    /// # Errors
    /// 公钥不是合法 Base64 或长度不符返回 [`SecurityError::InvalidKeyLength`]；
    /// 协商结果为全零（低阶点）返回 [`SecurityError::HandshakeStateInvalid`]。
    pub fn exchange_secret(&self, peer_public_b64: &str) -> Result<[u8; 32], SecurityError> {
        let peer_bytes: [u8; 32] = from_b64_exact(peer_public_b64, 32)?;
        let shared = self.secret.diffie_hellman(&PublicKey::from(peer_bytes));
        let bytes = shared.to_bytes();
        // 全零共享密钥意味着对端给了低阶点，必须拒绝（否则派生出的密钥可预测）。
        if bytes.iter().all(|b| *b == 0) {
            return Err(SecurityError::HandshakeStateInvalid {
                state: "X25519 共享密钥为全零（低阶点）".to_owned(),
            });
        }
        Ok(bytes)
    }
}

// =====** ChannelKeys =====**

/// 分通道派生结果。
///
/// 每个通道独立密钥：防止把一个通道的合法密文重放到另一个通道
/// （docs/07 §3 的“防跨通道重放”）。
#[derive(Clone)]
pub struct ChannelKeys {
    /// 媒体 主控端→被控端。
    pub media_c2h: [u8; 32],
    /// 媒体 被控端→主控端。
    pub media_h2c: [u8; 32],
    /// 控制通道（双向共用一个方向键，业务上按发送方区分）。
    pub control: [u8; 32],
    /// 握手确认 HMAC 密钥。
    pub confirm: [u8; 32],
    /// 输入通道。
    pub input: [u8; 32],
    /// 剪贴板通道。
    pub clipboard: [u8; 32],
    /// 文件通道。
    pub file: [u8; 32],
}

impl std::fmt::Debug for ChannelKeys {
    /// 脱敏 Debug：只显示“已派生”，不输出任何密钥字节。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ChannelKeys(<redacted>)")
    }
}

impl Drop for ChannelKeys {
    /// 会话结束时清零全部通道密钥，降低内存残留风险。
    fn drop(&mut self) {
        self.media_c2h.zeroize();
        self.media_h2c.zeroize();
        self.control.zeroize();
        self.confirm.zeroize();
        self.input.zeroize();
        self.clipboard.zeroize();
        self.file.zeroize();
    }
}

/// HKDF 分通道标签（info 字段）。
///
/// 标签是派生契约的一部分：两端必须使用完全相同的字符串，否则密钥不匹配。
pub mod labels {
    /// 媒体 c2h。
    pub const MEDIA_C2H: &[u8] = b"RDC-v1/media-c2h";
    /// 媒体 h2c。
    pub const MEDIA_H2C: &[u8] = b"RDC-v1/media-h2c";
    /// 控制通道。
    pub const CONTROL: &[u8] = b"RDC-v1/control";
    /// 握手确认。
    pub const CONFIRM: &[u8] = b"RDC-v1/confirm";
    /// 输入通道。
    pub const INPUT: &[u8] = b"RDC-v1/input";
    /// 剪贴板通道。
    pub const CLIPBOARD: &[u8] = b"RDC-v1/clipboard";
    /// 文件通道。
    pub const FILE: &[u8] = b"RDC-v1/file";
}

/// 用 X25519 共享密钥 + PAKE 输出派生全部分通道密钥。
///
/// 规则：`HKDF-SHA256(ikm = shared, salt = k_pake) + info = 通道标签`。
/// 为什么把 k_pake 作为 salt：即使 ECDH 被中间人替换，缺少配对码也无法算出相同密钥；
/// 而 shared 作为 IKM 保证每条会话的前向保密。
///
/// # Arguments
/// * `shared` - X25519 共享密钥（32 字节）
/// * `k_pake` - PAKE 输出（长度由 SPAKE2 决定，通常 32 字节）
///
/// # Returns
/// 七个通道的独立密钥。
///
/// # Errors
/// 不会失败（HKDF 对任意长度输入都合法）。
pub fn derive_channel_keys(shared: &[u8; 32], k_pake: &[u8]) -> ChannelKeys {
    let hk = Hkdf::<Sha256>::new(Some(k_pake), shared);
    ChannelKeys {
        media_c2h: expand(&hk, labels::MEDIA_C2H),
        media_h2c: expand(&hk, labels::MEDIA_H2C),
        control: expand(&hk, labels::CONTROL),
        confirm: expand(&hk, labels::CONFIRM),
        input: expand(&hk, labels::INPUT),
        clipboard: expand(&hk, labels::CLIPBOARD),
        file: expand(&hk, labels::FILE),
    }
}

/// 按标签展开 32 字节密钥。
///
/// # Arguments
/// * `hk` - 已初始化的 HKDF
/// * `label` - 通道标签
///
/// # Returns
/// 32 字节密钥；`expand` 对 32 字节输出不会失败。
fn expand(hk: &Hkdf<Sha256>, label: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    // OKM 长度 32 远小于 HKDF-SHA256 的上限（255 * 32），因此 unwrap 是安全的；
    // 用 expect 而不是 unwrap 让失败信息可读（实际不会触发）。
    if let Err(err) = hk.expand(label, &mut out) {
        // 理论上不可达；宁可 panic 也不返回全零密钥。
        panic!("HKDF expand 失败: {err}");
    }
    out
}

/// 从 k_pake 派生 4 位 SAS（short auth string）。
///
/// 规则：`SHA256(k_pake ‖ "SAS")` 前 2 字节取模 10000，补零到 4 位。
/// 两端独立计算应得到相同值，用户比对一致才确认连接。
///
/// # Arguments
/// * `k_pake` - PAKE 输出
///
/// # Returns
/// 形如 `"0427"` 的 4 位数字串。
///
/// # Errors
/// 不会失败。
pub fn derive_sas(k_pake: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(k_pake);
    hasher.update(b"SAS");
    let digest = hasher.finalize();
    let value = u16::from_be_bytes([digest[0], digest[1]]) as u32 % 10_000;
    format!("{value:04}")
}

// =====** 单元测试：密钥一致性与分通道隔离 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 两个独立临时密钥必须协商出相同共享密钥。
    #[test]
    fn x25519_agrees() {
        let a = EphemeralKey::generate();
        let b = EphemeralKey::generate();
        let sa = a.exchange_secret(&b.public_key_b64()).expect("协商");
        let sb = b.exchange_secret(&a.public_key_b64()).expect("协商");
        assert_eq!(sa, sb);
        assert_ne!(sa, [0u8; 32]);
    }

    /// 身份签名必须可验，篡改任何一个字节都必须失败。
    #[test]
    fn signature_verifies_and_tamper_fails() {
        let id = IdentityKeyPair::generate();
        let payload = b"RDC-AUTH session challenge";
        let sig = id.sign_b64(payload);
        id.verify_b64(payload, &sig).expect("验签");
        let mut tampered = payload.to_vec();
        tampered[0] ^= 1;
        assert!(id.verify_b64(&tampered, &sig).is_err());
        let mut sig_bytes = STANDARD.decode(&sig).expect("Base64");
        sig_bytes[0] ^= 1;
        let bad_sig = STANDARD.encode(sig_bytes);
        assert!(id.verify_b64(payload, &bad_sig).is_err());
    }

    /// 相同输入派生必须确定，不同通道标签必须产生不同密钥。
    #[test]
    fn hkdf_channels_are_distinct_and_deterministic() {
        let shared = [9u8; 32];
        let k_pake = [7u8; 32];
        let a = derive_channel_keys(&shared, &k_pake);
        let b = derive_channel_keys(&shared, &k_pake);
        assert_eq!(a.media_c2h, b.media_c2h);
        assert_ne!(a.media_c2h, a.media_h2c);
        assert_ne!(a.control, a.input);
        assert_ne!(a.clipboard, a.file);
        // 不同 k_pake 必须改变所有通道密钥（PAKE 绑定生效）。
        let c = derive_channel_keys(&shared, &[8u8; 32]);
        assert_ne!(a.control, c.control);
    }

    /// SAS 必须是 4 位数字且两端一致。
    #[test]
    fn sas_is_four_digits_and_stable() {
        let sas = derive_sas(&[1u8; 32]);
        assert_eq!(sas.len(), 4);
        assert!(sas.chars().all(|c| c.is_ascii_digit()));
        assert_eq!(sas, derive_sas(&[1u8; 32]));
        assert_ne!(sas, derive_sas(&[2u8; 32]));
    }

    /// 指纹必须稳定且带冒号分隔。
    #[test]
    fn fingerprint_is_stable() {
        let id = IdentityKeyPair::generate();
        let fp = id.fingerprint();
        assert_eq!(fp.split(':').count(), 8);
        assert_eq!(fp, id.fingerprint());
    }
}
