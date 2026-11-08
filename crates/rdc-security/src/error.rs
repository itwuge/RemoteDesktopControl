//! rdc-security / 错误类型
//!
//! 层级：L2 能力层
//! 用途：定义密钥长度、AEAD、握手状态、签名与票据校验失败的可判别错误，
//!       并透传 rdc-protocol 的报文解析错误
//! 调用方：rdc-connect / rdc-core 的错误映射，最终由 rdc-core 收敛进 RdcError

// =====** SecurityError =====**

/// 安全层错误。
///
/// 敏感值约束：Display 文案**不得**引用底层 `#[source]`；源错误只供日志/排查按
/// 类型取回，绝不参与用户可见文本。各变体的 `source` 为 `Option`，因为部分失败
/// 是前置校验（如长度不足），没有底层错误。
#[derive(Debug, thiserror::Error)]
pub enum SecurityError {
    /// 密钥长度不符合算法要求。
    #[error("密钥长度错误: 期望 {expected} 字节, 实际 {actual} 字节")]
    InvalidKeyLength {
        /// 算法要求的字节数。
        expected: usize,
        /// 实际收到的字节数。
        actual: usize,
        /// 底层 Base64 解码错误；长度前置校验时为 `None`。
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
    /// AEAD 加密或解密失败（标签校验不通过）。
    #[error("AEAD 加解密失败")]
    AeadFailure {
        /// 底层算法库错误（内容不透明，不含密钥/明文）；前置长度校验时为 `None`。
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
    /// 握手状态机不接受当前操作。
    #[error("握手状态非法: {state}")]
    HandshakeStateInvalid {
        /// 当前状态的可读名称，用于日志定位。
        state: String,
    },
    /// 签名校验不通过。
    #[error("签名校验失败")]
    SignatureInvalid {
        /// 底层公钥/签名解析或验签错误；无底层错误时为 `None`。
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
    /// 报文序号被重放或超出接收窗口。
    #[error("检测到重放或序号超出窗口: seq={seq}, 窗口最高={highest}")]
    ReplayDetected {
        /// 收到的序号。
        seq: u64,
        /// 接收窗口当前最高序号。
        highest: u64,
    },
    /// PAKE 交互失败（消息长度/侧别不匹配或口令不一致）。
    #[error("PAKE 交互失败: {message}")]
    PakeFailure {
        /// 失败原因（静态文案，不含口令或消息内容）。
        message: &'static str,
        /// 底层 PAKE/Base64 错误；状态机前置检查时为 `None`。
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
    /// 配对码格式非法。
    #[error("配对码格式非法")]
    PairingCodeInvalid,
    /// 握手确认 HMAC 校验失败（通常意味着配对码错误）。
    #[error("握手确认失败")]
    ConfirmationFailed {
        /// 底层 Base64 解码或 HMAC 比较错误；状态机前置检查时为 `None`。
        #[source]
        source: Option<Box<dyn std::error::Error + Send + Sync>>,
    },
    /// 中继票据无效或已过期。
    #[error("中继票据无效")]
    TicketInvalid,
    /// 对端协议报文不可解析。
    ///
    /// 保留原始错误而不是折叠成字符串：上层需要按 `ErrorCode` 选择 Dart 文案。
    #[error(transparent)]
    Protocol(#[from] rdc_protocol::ProtocolError),
}

// =====** AeadLibError =====**

/// AEAD 算法库错误的适配层。
///
/// 为什么需要：`aead` 只在开启 `std` 特性时才实现 `std::error::Error`
/// （`aead-0.5.2/src/lib.rs:90-91`），本 workspace 未开启该特性；这里补一个
/// 零信息损失的适配，让底层错误可以挂到 `#[source]` 链上。
///
/// 敏感值约束：`Display` 只转发算法库自身文案（固定为 `aead::Error`），
/// 不包含密文、密钥或 nonce。
#[derive(Debug)]
pub(crate) struct AeadLibError(pub(crate) aes_gcm::Error);

impl std::fmt::Display for AeadLibError {
    /// 转发算法库自身的 Display，而不是拼接任何会话材料。
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, formatter)
    }
}

impl std::error::Error for AeadLibError {}

// =====** Spake2LibError =====**

/// PAKE 算法库错误的适配层。
///
/// 为什么需要：`spake2` 只在开启 `std` 特性时才实现 `std::error::Error`
/// （`spake2-0.4.0/src/error.rs:31-32`，默认特性只有 `getrandom`），本 workspace
/// 未开启该特性；这里补一个零信息损失的适配。
///
/// 敏感值约束：`Display` 只转发算法库自身文案，不包含口令或密钥材料。
#[derive(Debug)]
pub(crate) struct Spake2LibError(pub(crate) spake2::Error);

impl std::fmt::Display for Spake2LibError {
    /// 转发算法库自身的 Display，而不是拼接任何会话材料。
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.0, formatter)
    }
}

impl std::error::Error for Spake2LibError {}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用伪造敏感值：一旦出现在 Display 文本里即视为泄漏。
    const FAKE_SEED: &str = "FAKE-SEED-DO-NOT-LOG-0123456789ABCDEF";
    /// 伪造密文。
    const FAKE_CIPHERTEXT: &str = "FAKE-CIPHERTEXT-DO-NOT-LOG-BASE64";
    /// 伪造公钥。
    const FAKE_PUBLIC_KEY: &str = "FAKE-PUBLIC-KEY-DO-NOT-LOG-BASE64";
    /// 伪造口令。
    const FAKE_PASSPHRASE: &str = "FAKE-PASSPHRASE-DO-NOT-LOG";
    /// 伪造握手确认密钥。
    const FAKE_CONFIRMATION_KEY: &str = "FAKE-CONFIRMATION-KEY-DO-NOT-LOG";

    /// 携带伪造敏感值的源错误，用于验证 Display 不引用 `#[source]`。
    #[derive(Debug)]
    struct FakeSecretSource(&'static str);

    impl std::fmt::Display for FakeSecretSource {
        /// 故意输出伪造敏感值：只要 Display 引用它就会泄漏。
        fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            formatter.write_str(self.0)
        }
    }

    impl std::error::Error for FakeSecretSource {}

    /// 断言：文案逐字保留、不泄漏伪造值、`source()` 可取回具体类型。
    fn assert_display_hides_source(error: SecurityError, expected: &str, secret: &str) {
        let rendered = error.to_string();
        assert_eq!(rendered, expected, "用户可见文案必须逐字保留");
        assert!(!rendered.contains(secret), "Display 不得泄漏 source 中的敏感值");
        let source = std::error::Error::source(&error).expect("source() 必须能取回底层错误");
        assert!(
            source.downcast_ref::<FakeSecretSource>().is_some(),
            "source() 取回后应能 downcast 到具体错误类型"
        );
    }

    /// 密钥长度错误：源错误里的种子不得进入文案。
    #[test]
    fn invalid_key_length_display_hides_seed() {
        let error = SecurityError::InvalidKeyLength {
            expected: 32,
            actual: 5,
            source: Some(Box::new(FakeSecretSource(FAKE_SEED))),
        };
        assert_display_hides_source(error, "密钥长度错误: 期望 32 字节, 实际 5 字节", FAKE_SEED);
    }

    /// AEAD 失败：源错误里的密文不得进入文案。
    #[test]
    fn aead_failure_display_hides_ciphertext() {
        let error = SecurityError::AeadFailure {
            source: Some(Box::new(FakeSecretSource(FAKE_CIPHERTEXT))),
        };
        assert_display_hides_source(error, "AEAD 加解密失败", FAKE_CIPHERTEXT);
    }

    /// 签名失败：源错误里的公钥不得进入文案。
    #[test]
    fn signature_invalid_display_hides_public_key() {
        let error = SecurityError::SignatureInvalid {
            source: Some(Box::new(FakeSecretSource(FAKE_PUBLIC_KEY))),
        };
        assert_display_hides_source(error, "签名校验失败", FAKE_PUBLIC_KEY);
    }

    /// PAKE 失败：源错误里的口令不得进入文案。
    #[test]
    fn pake_failure_display_hides_passphrase() {
        let error = SecurityError::PakeFailure {
            message: "PAKE 消息长度或侧别不匹配",
            source: Some(Box::new(FakeSecretSource(FAKE_PASSPHRASE))),
        };
        assert_display_hides_source(
            error,
            "PAKE 交互失败: PAKE 消息长度或侧别不匹配",
            FAKE_PASSPHRASE,
        );
    }

    /// 握手确认失败：源错误里的确认密钥不得进入文案。
    #[test]
    fn confirmation_failed_display_hides_key() {
        let error = SecurityError::ConfirmationFailed {
            source: Some(Box::new(FakeSecretSource(FAKE_CONFIRMATION_KEY))),
        };
        assert_display_hides_source(error, "握手确认失败", FAKE_CONFIRMATION_KEY);
    }

    /// 适配层 Display 必须是算法库固定文案，且自身没有更深的 source。
    #[test]
    fn aead_lib_error_display_is_library_text() {
        let error = AeadLibError(aes_gcm::Error);
        assert_eq!(error.to_string(), "aead::Error");
        assert!(std::error::Error::source(&error).is_none());
    }
}
