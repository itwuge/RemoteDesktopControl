//! rdc-security / AEAD 对称加密与重放窗口
//!
//! 层级：L2 能力层
//! 用途：实现 AES-256-GCM / ChaCha20-Poly1305 的 seal/open（头部作为 AAD），
//!       nonce 构造规则（4 字节零 + 8 字节单调计数）与 64 位接收窗口（docs/04 §1.1、§07 §3）。
//! 调用方：rdc-connect（控制通道与直连会话）、rdc-transport（媒体分片）、rdc-relay（转发不改密文）
//!
//! 安全前提：同一密钥下 nonce 绝不重复；接收窗口只在 AEAD 校验通过后推进。

use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use chacha20poly1305::ChaCha20Poly1305;

use crate::error::SecurityError;

// =====** 常量 =====**

/// AEAD 标签长度（字节），与包头常量一致。
pub const TAG_LEN: usize = 16;

/// nonce 长度（字节）。
pub const NONCE_LEN: usize = 12;

/// 重放窗口位数（64 位 = 最近 64 个序号）。
pub const REPLAY_WINDOW_BITS: u64 = 64;

// =====** CipherSuite =====**

/// 对称加密算法。
//[桌面优先 AES-NI] 移动端无 AES 加速时可切 ChaCha20；协商位由能力位承载（S3）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CipherSuite {
    /// AES-256-GCM（有 AES-NI 时最快）。
    Aes256Gcm,
    /// ChaCha20-Poly1305（无 AES 加速时的软件优先方案）。
    ChaCha20Poly1305,
}

// =====** AeadKey =====**

/// 32 字节对称密钥 + 算法句柄。
///
/// 刻意不实现 Debug / 序列化：密钥材料不得出现在日志、统计或线上报文里。
pub struct AeadKey {
    /// 原始密钥字节（Copy 进算法实例后仍保留用于重配）。
    key: [u8; 32],
    /// 算法。
    suite: CipherSuite,
}

impl std::fmt::Debug for AeadKey {
    /// 脱敏 Debug。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AeadKey(<redacted>)")
    }
}

impl AeadKey {
    /// 用已有密钥字节构造密钥句柄（默认 AES-256-GCM）。
    ///
    /// # Arguments
    /// * `key` - 32 字节密钥材料，通常来自 [`crate::keys::derive_channel_keys`]
    ///
    /// # Returns
    /// 持有该密钥的 `AeadKey`。
    ///
    /// # Errors
    /// 不会失败；长度由类型系统在编译期保证。
    pub fn new(key: [u8; 32]) -> Self {
        Self {
            key,
            suite: CipherSuite::Aes256Gcm,
        }
    }

    /// 指定算法构造密钥句柄。
    ///
    /// # Arguments
    /// * `key` - 32 字节密钥
    /// * `suite` - 算法
    ///
    /// # Returns
    /// 持有该密钥的 `AeadKey`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn with_suite(key: [u8; 32], suite: CipherSuite) -> Self {
        Self { key, suite }
    }

    /// 返回算法。
    ///
    /// # Returns
    /// 当前算法。
    ///
    /// # Errors
    /// 不会失败。
    pub fn suite(&self) -> CipherSuite {
        self.suite
    }

    /// 加密并附加认证标签（AAD 为空）。
    ///
    /// # Arguments
    /// * `plaintext` - 待加密的明文
    /// * `nonce` - 12 字节 nonce，同一密钥下禁止复用
    ///
    /// # Returns
    /// 密文与 16 字节认证标签拼接后的字节序列。
    ///
    /// # Errors
    /// 加密失败（通常只有长度超限）返回 [`SecurityError::AeadFailure`]。
    pub fn seal(&self, plaintext: &[u8], nonce: &[u8; NONCE_LEN]) -> Result<Vec<u8>, SecurityError> {
        self.seal_with_aad(plaintext, nonce, &[])
    }

    /// 校验标签并解密（AAD 为空）。
    ///
    /// # Arguments
    /// * `ciphertext` - 密文与认证标签
    /// * `nonce` - 与加密时完全一致的 12 字节 nonce
    ///
    /// # Returns
    /// 解密后的明文。
    ///
    /// # Errors
    /// 标签校验失败、密文长度不足或 nonce 不一致时返回 [`SecurityError::AeadFailure`]
    /// （不区分失败原因，避免侧信道泄漏）。
    pub fn open(&self, ciphertext: &[u8], nonce: &[u8; NONCE_LEN]) -> Result<Vec<u8>, SecurityError> {
        self.open_with_aad(ciphertext, nonce, &[])
    }

    /// 加密并附加认证标签，附加数据（AAD）参与认证但不加密。
    ///
    /// 媒体包的 AAD 就是 32 字节头部：任何头部字段被篡改都会导致解密失败。
    ///
    /// # Arguments
    /// * `plaintext` - 明文
    /// * `nonce` - 12 字节 nonce
    /// * `aad` - 附加认证数据
    ///
    /// # Returns
    /// 密文 + 标签。
    ///
    /// # Errors
    /// 加密失败返回 [`SecurityError::AeadFailure`]。
    pub fn seal_with_aad(
        &self,
        plaintext: &[u8],
        nonce: &[u8; NONCE_LEN],
        aad: &[u8],
    ) -> Result<Vec<u8>, SecurityError> {
        let payload = Payload { msg: plaintext, aad };
        match self.suite {
            CipherSuite::Aes256Gcm => {
                let cipher = Aes256Gcm::new_from_slice(&self.key)
                    .map_err(|_| SecurityError::AeadFailure)?;
                cipher
                    .encrypt(Nonce::from_slice(nonce), payload)
                    .map_err(|_| SecurityError::AeadFailure)
            }
            CipherSuite::ChaCha20Poly1305 => {
                let cipher = ChaCha20Poly1305::new_from_slice(&self.key)
                    .map_err(|_| SecurityError::AeadFailure)?;
                cipher
                    .encrypt(chacha20poly1305::Nonce::from_slice(nonce), payload)
                    .map_err(|_| SecurityError::AeadFailure)
            }
        }
    }

    /// 校验标签并解密（AAD 参与认证）。
    ///
    /// # Arguments
    /// * `ciphertext` - 密文 + 标签
    /// * `nonce` - 12 字节 nonce
    /// * `aad` - 与加密时一致的附加认证数据
    ///
    /// # Returns
    /// 明文。
    ///
    /// # Errors
    /// 标签/AAD 校验失败返回 [`SecurityError::AeadFailure`]。
    pub fn open_with_aad(
        &self,
        ciphertext: &[u8],
        nonce: &[u8; NONCE_LEN],
        aad: &[u8],
    ) -> Result<Vec<u8>, SecurityError> {
        // 长度不足直接拒绝，避免算法库报“空密文”这种误导性错误。
        if ciphertext.len() < TAG_LEN {
            return Err(SecurityError::AeadFailure);
        }
        let payload = Payload { msg: ciphertext, aad };
        match self.suite {
            CipherSuite::Aes256Gcm => {
                let cipher = Aes256Gcm::new_from_slice(&self.key)
                    .map_err(|_| SecurityError::AeadFailure)?;
                cipher
                    .decrypt(Nonce::from_slice(nonce), payload)
                    .map_err(|_| SecurityError::AeadFailure)
            }
            CipherSuite::ChaCha20Poly1305 => {
                let cipher = ChaCha20Poly1305::new_from_slice(&self.key)
                    .map_err(|_| SecurityError::AeadFailure)?;
                cipher
                    .decrypt(chacha20poly1305::Nonce::from_slice(nonce), payload)
                    .map_err(|_| SecurityError::AeadFailure)
            }
        }
    }
}

/// 按协议规则构造 nonce：前 4 字节全零，后 8 字节为单调计数器（大端）。
///
/// # Arguments
/// * `counter` - 单调递增计数（同一密钥下从 1 开始）
///
/// # Returns
/// 12 字节 nonce。
///
/// # Errors
/// 不会失败。
pub fn nonce_from_counter(counter: u64) -> [u8; NONCE_LEN] {
    let mut nonce = [0u8; NONCE_LEN];
    nonce[4..].copy_from_slice(&counter.to_be_bytes());
    nonce
}

// =====** ReplayWindow =====**

/// 64 位滑动重放窗口。
///
/// 只在校验通过的包上调用 `check_and_update`；窗口记录“最近 64 个序号是否已用”。
#[derive(Debug, Clone)]
pub struct ReplayWindow {
    /// 已接收的最大序号。
    largest: u64,
    /// 位图：bit0 对应 largest，bit63 对应 largest-63。
    bitmap: u64,
    /// 是否已收到过任何包。
    initialized: bool,
}

impl Default for ReplayWindow {
    /// 初始为空窗口。
    fn default() -> Self {
        Self::new()
    }
}

impl ReplayWindow {
    /// 创建空窗口。
    ///
    /// # Returns
    /// 尚未接收任何序号。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new() -> Self {
        Self {
            largest: 0,
            bitmap: 0,
            initialized: false,
        }
    }

    /// 检查并推进窗口。
    ///
    /// # Arguments
    /// * `seq` - 收到的包序号（u64）
    ///
    /// # Returns
    /// 首次出现且未超出窗口时返回 `Ok(())`。
    ///
    /// # Errors
    /// 重复序号（重放）或比窗口最旧还旧时返回 [`SecurityError::ReplayDetected`]。
    pub fn check_and_update(&mut self, seq: u64) -> Result<(), SecurityError> {
        if !self.initialized {
            // 第一包无条件接受：session 建立后第一次通信的序号可能不是 0。
            self.initialized = true;
            self.largest = seq;
            self.bitmap = 1;
            return Ok(());
        }
        if seq > self.largest {
            let shift = seq - self.largest;
            self.bitmap = if shift >= REPLAY_WINDOW_BITS {
                // 跨度超过窗口，全部历史失效，只保留新包自己。
                1
            } else {
                (self.bitmap << shift) | 1
            };
            self.largest = seq;
            Ok(())
        } else {
            let diff = self.largest - seq;
            if diff >= REPLAY_WINDOW_BITS {
                // 比窗口最旧还旧：无法判断是否重放，按重放拒绝。
                return Err(SecurityError::ReplayDetected {
                    seq,
                    highest: self.largest,
                });
            }
            let mask = 1u64 << diff;
            if self.bitmap & mask != 0 {
                return Err(SecurityError::ReplayDetected {
                    seq,
                    highest: self.largest,
                });
            }
            self.bitmap |= mask;
            Ok(())
        }
    }

    /// 当前窗口最高序号（未初始化时为 0）。
    ///
    /// # Returns
    /// 最大已接受序号。
    ///
    /// # Errors
    /// 不会失败。
    pub fn highest(&self) -> u64 {
        self.largest
    }
}

// =====** 单元测试：篡改与重放 =====**

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> AeadKey {
        AeadKey::new([0x11u8; 32])
    }

    /// 两种算法都必须能往返。
    #[test]
    fn seal_open_roundtrip_both_suites() {
        for suite in [CipherSuite::Aes256Gcm, CipherSuite::ChaCha20Poly1305] {
            let k = AeadKey::with_suite([0x22u8; 32], suite);
            let nonce = nonce_from_counter(1);
            let ct = k.seal(b"hello rdc", &nonce).expect("加密");
            assert_eq!(ct.len(), 9 + TAG_LEN);
            assert_eq!(k.open(&ct, &nonce).expect("解密"), b"hello rdc");
        }
    }

    /// 篡改密文任何一个字节都必须解密失败。
    #[test]
    fn tampered_ciphertext_is_rejected() {
        let nonce = nonce_from_counter(1);
        let ct = key().seal(b"payload", &nonce).expect("加密");
        let mut bad = ct.clone();
        bad[0] ^= 0x01;
        assert!(matches!(key().open(&bad, &nonce), Err(SecurityError::AeadFailure)));
    }

    /// 篡改 AAD（例如包头字段）必须解密失败：这是头部不可篡改的保证。
    #[test]
    fn tampered_aad_is_rejected() {
        let nonce = nonce_from_counter(2);
        let aad = b"header-bytes";
        let ct = key()
            .seal_with_aad(b"body", &nonce, aad)
            .expect("加密");
        key().open_with_aad(&ct, &nonce, aad).expect("正常解密");
        let mut bad_aad = aad.to_vec();
        bad_aad[0] ^= 0x01;
        assert!(matches!(
            key().open_with_aad(&ct, &nonce, &bad_aad),
            Err(SecurityError::AeadFailure)
        ));
    }

    /// nonce 复用同一密钥会泄漏明文关系，测试只验证“不同计数产生不同 nonce”。
    #[test]
    fn nonce_layout_is_4_zero_plus_counter() {
        let n = nonce_from_counter(0x0102_0304_0506_0708);
        assert_eq!(&n[..4], &[0, 0, 0, 0]);
        assert_eq!(&n[4..], &0x0102_0304_0506_0708u64.to_be_bytes());
    }

    /// 同一序号第二次到达必须被拒绝。
    #[test]
    fn replay_is_rejected() {
        let mut w = ReplayWindow::new();
        w.check_and_update(10).expect("首包");
        assert!(matches!(
            w.check_and_update(10),
            Err(SecurityError::ReplayDetected { .. })
        ));
    }

    /// 窗口内的乱序包应被接受，窗口外的旧包应被拒绝。
    #[test]
    fn out_of_order_within_window_is_accepted() {
        let mut w = ReplayWindow::new();
        w.check_and_update(100).expect("首包");
        w.check_and_update(98).expect("窗口内乱序");
        assert!(w.check_and_update(98).is_err(), "重复必须拒绝");
        assert!(w.check_and_update(100 - 63).is_ok());
        assert!(w.check_and_update(10).is_err(), "窗口外旧包必须拒绝");
        w.check_and_update(200).expect("大跨度前进");
        assert!(w.check_and_update(100).is_err(), "超过 64 的历史必须拒绝");
        assert_eq!(w.highest(), 200);
    }
}
