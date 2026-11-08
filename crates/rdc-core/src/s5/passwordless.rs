//! rdc-core / S5 免密登录（混合信任模型）
//!
//! 层级：L3 编排层
//! 用途：决定一次连接是「信任设备免密直连」还是「必须配对码」，并在账号服务不可达时
//!       按本地信任表回退，保证连接不受影响。
//! 调用方：rdc-bridge（权限页策略展示）、未来 rdc-connect 的建链握手。
//!
//! 边界（docs/07 §4.4）：
//! - 账号服务只做分发/列表同步/撤销广播，**不参与 E2E 加密**，也不持有会话密钥；
//! - `grant_token` 只走业务通道，**不进信令包头**；
//! - 信任根始终是「本机地址簿里的公钥」，账号服务下发的列表只是输入。

use rdc_security::{verify_signature_b64, IdentityKeyPair};

use crate::s5::devices::{DeviceList, DeviceRecord};

// =====** 常量 =====**

/// 挑战串前缀（域名分隔，避免签名被挪用到其他协议用途）。
pub const CHALLENGE_PREFIX: &str = "RDC-passwordless-v1|";
/// 账号服务在授权体系中的角色说明（供页面与审计展示）。
pub const ACCOUNT_SERVICE_ROLE: &str =
    "账号服务：仅分发授权令牌、同步设备列表与撤销广播；不参与端到端加密，不持有会话密钥。";

// =====** 类型 =====**

/// 连接认证方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingMode {
    /// 信任设备：跳过配对码，走密钥挑战-应答。
    Passwordless,
    /// 未信任设备：必须配对码。
    PairingCode,
}

impl PairingMode {
    /// 稳定英文名。
    ///
    /// # Returns
    /// `passwordless` / `pairing_code`。
    ///
    /// # Errors
    /// 不会失败。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Passwordless => "passwordless",
            Self::PairingCode => "pairing_code",
        }
    }
}

/// 判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingDecision {
    /// 认证方式。
    pub mode: PairingMode,
    /// 是否仍需配对码。
    pub requires_pairing_code: bool,
    /// 是否因账号服务不可达而走了本地信任回退。
    pub used_local_fallback: bool,
    /// 判定依据（写审计）。
    pub reason: &'static str,
}

/// 免密判定错误（只有「设备身份已变」是硬拒绝）。
///
/// 为什么不再派生 `Clone/PartialEq/Eq`：`ChallengeFailed` 需要把
/// [`rdc_security::SecurityError`] 挂进 `source`，该类型内含
/// `Box<dyn Error>`，不满足这些派生的约束。
#[derive(Debug, thiserror::Error)]
pub enum PairingError {
    /// 设备记录没有公钥，只能退回配对码。
    #[error("设备记录缺少公钥，必须配对码")]
    MissingPublicKey,
    /// 对端出示的公钥与地址簿不一致（疑似换钥）。
    #[error("设备公钥与地址簿不一致（疑似换钥攻击）")]
    PublicKeyMismatch,
    /// 挑战-应答验签失败。
    #[error("免密挑战-应答验签失败")]
    ChallengeFailed {
        /// 底层验签错误，保留 [`rdc_security::SecurityError`] 具体类型。
        #[source]
        source: rdc_security::SecurityError,
    },
}

impl PairingError {
    /// 固定错误码。
    ///
    /// # Returns
    /// 稳定标识。
    ///
    /// # Errors
    /// 不会失败。
    pub fn code(&self) -> &'static str {
        match self {
            Self::MissingPublicKey => "missing_public_key",
            Self::PublicKeyMismatch => "public_key_mismatch",
            Self::ChallengeFailed { .. } => "challenge_failed",
        }
    }
}

/// 判定上下文。
#[derive(Debug, Clone, Copy)]
pub struct PairingContext<'a> {
    /// 本机地址簿。
    pub devices: &'a DeviceList,
    /// 账号服务是否可达（false 时按本地信任回退）。
    pub account_service_reachable: bool,
    /// 用户是否开启「完全免密」。
    pub passwordless_enabled: bool,
}

/// 决定认证方式。
///
/// 规则：
/// - 设备未信任 → 必须配对码（无论账号服务是否可达）；
/// - 设备已信任但地址簿没有公钥 → 必须配对码（不能凭信任位裸奔）；
/// - 设备已信任且公钥匹配 + 免密开关开 → 免密；账号服务不可达不影响结果，
///   只是标记 `used_local_fallback`。
///
/// # Arguments
/// * `device_id` - 对端设备标识
/// * `presented_pub` - 对端本次出示的身份公钥（Base64）
/// * `context` - 判定上下文
///
/// # Returns
/// 认证方式与依据。
///
/// # Errors
/// 已信任但公钥与地址簿不一致时返回 [`PairingError::PublicKeyMismatch`]。
pub fn decide(
    device_id: &str,
    presented_pub: &str,
    context: &PairingContext<'_>,
) -> Result<PairingDecision, PairingError> {
    decide_for_record(
        context.devices.get(device_id),
        presented_pub,
        context.account_service_reachable,
        context.passwordless_enabled,
    )
}

/// 按单条设备记录决定认证方式（页面/桥接层可直接调用）。
///
/// 为什么单独提供：权限页快照只需要一条记录的判定，不必构造整个地址簿上下文；
/// 与 [`decide`] 共用同一套规则，避免两处逻辑漂移。
///
/// # Arguments
/// * `record` - 设备记录；`None` 表示未知设备
/// * `presented_pub` - 对端本次出示的身份公钥
/// * `account_service_reachable` - 账号服务是否可达
/// * `passwordless_enabled` - 用户是否开启完全免密
///
/// # Returns
/// 认证方式与依据。
///
/// # Errors
/// 已信任但公钥与地址簿不一致时返回 [`PairingError::PublicKeyMismatch`]。
pub fn decide_for_record(
    record: Option<&DeviceRecord>,
    presented_pub: &str,
    account_service_reachable: bool,
    passwordless_enabled: bool,
) -> Result<PairingDecision, PairingError> {
    let trusted = record.map(|record| record.trusted).unwrap_or(false);
    let stored_key = record.map(|record| record.public_key_b64.as_str()).unwrap_or("");
    if !trusted {
        return Ok(PairingDecision {
            mode: PairingMode::PairingCode,
            requires_pairing_code: true,
            used_local_fallback: !account_service_reachable,
            reason: "untrusted_device",
        });
    }
    if stored_key.is_empty() {
        return Ok(PairingDecision {
            mode: PairingMode::PairingCode,
            requires_pairing_code: true,
            used_local_fallback: !account_service_reachable,
            reason: "missing_public_key",
        });
    }
    if stored_key != presented_pub {
        return Err(PairingError::PublicKeyMismatch);
    }
    if !passwordless_enabled {
        return Ok(PairingDecision {
            mode: PairingMode::PairingCode,
            requires_pairing_code: true,
            used_local_fallback: !account_service_reachable,
            reason: "passwordless_disabled",
        });
    }
    Ok(PairingDecision {
        mode: PairingMode::Passwordless,
        requires_pairing_code: false,
        used_local_fallback: !account_service_reachable,
        reason: "trusted_local_key",
    })
}

/// 生成免密挑战串。
///
/// # Arguments
/// * `session_tag` - 会话标识（防止跨会话重放）
/// * `now_ms` - 当前 Unix 毫秒
///
/// # Returns
/// 形如 `RDC-passwordless-v1|<tag>|<ts>` 的挑战串。
///
/// # Errors
/// 不会失败。
pub fn challenge(session_tag: &str, now_ms: u64) -> String {
    format!("{CHALLENGE_PREFIX}{session_tag}|{now_ms}")
}

/// 对端对挑战签名。
///
/// # Arguments
/// * `identity` - 对端身份密钥
/// * `challenge_str` - 挑战串
///
/// # Returns
/// Base64 签名。
///
/// # Errors
/// 不会失败。
pub fn respond(identity: &IdentityKeyPair, challenge_str: &str) -> String {
    identity.sign_b64(challenge_str.as_bytes())
}

/// 本机验证挑战-应答。
///
/// # Arguments
/// * `public_key_b64` - 地址簿中的对端公钥
/// * `challenge_str` - 挑战串
/// * `response_b64` - 对端签名
///
/// # Returns
/// 验签通过返回 `Ok(())`。
///
/// # Errors
/// 验签失败返回 [`PairingError::ChallengeFailed`]。
pub fn verify_response(
    public_key_b64: &str,
    challenge_str: &str,
    response_b64: &str,
) -> Result<(), PairingError> {
    verify_signature_b64(public_key_b64, challenge_str.as_bytes(), response_b64)
        .map_err(|source| PairingError::ChallengeFailed { source })
}

// =====** 单元测试（F1–F4） =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造地址簿：A 已信任且有公钥；B 未信任。
    fn book() -> (DeviceList, IdentityKeyPair, IdentityKeyPair) {
        let a = IdentityKeyPair::from_seed(&[1u8; 32]);
        let b = IdentityKeyPair::from_seed(&[2u8; 32]);
        let mut list = DeviceList::new();
        list.upsert_online("dev-a", "工作站 A", 1_000);
        list.set_trusted("dev-a", true);
        list.set_public_key("dev-a", &a.public_key_b64());
        list.upsert_online("dev-b", "笔记本 B", 2_000);
        (list, a, b)
    }

    /// F1：信任设备 + 公钥匹配 + 免密开 → 跳过配对码，挑战-应答通过。
    #[test]
    fn f1_trusted_device_skips_pairing_code() {
        let (list, a, _b) = book();
        let context = PairingContext {
            devices: &list,
            account_service_reachable: true,
            passwordless_enabled: true,
        };
        let decision = decide("dev-a", &a.public_key_b64(), &context).expect("判定");
        assert_eq!(decision.mode, PairingMode::Passwordless);
        assert!(!decision.requires_pairing_code);
        assert!(!decision.used_local_fallback);
        assert_eq!(decision.reason, "trusted_local_key");

        let challenge_str = challenge("session-1", 1_700_000_000_000);
        let response = respond(&a, &challenge_str);
        verify_response(&a.public_key_b64(), &challenge_str, &response).expect("验签通过");

        // 错误公钥的签名不能通过。
        let (_list, _a, b) = book();
        let wrong = respond(&b, &challenge_str);
        assert!(verify_response(&a.public_key_b64(), &challenge_str, &wrong).is_err());
    }

    /// F2：未信任设备必须配对码；信任但无公钥也必须配对码；换钥硬拒绝。
    #[test]
    fn f2_untrusted_requires_pairing_code() {
        let (list, a, b) = book();
        let context = PairingContext {
            devices: &list,
            account_service_reachable: true,
            passwordless_enabled: true,
        };
        let untrusted = decide("dev-b", &b.public_key_b64(), &context).expect("判定");
        assert_eq!(untrusted.mode, PairingMode::PairingCode);
        assert!(untrusted.requires_pairing_code);
        assert_eq!(untrusted.reason, "untrusted_device");

        // 完全未知设备同样必须配对码。
        let unknown = decide("dev-x", &b.public_key_b64(), &context).expect("判定");
        assert!(unknown.requires_pairing_code);

        // 信任但地址簿缺公钥（旧记录）。
        let mut legacy = list.clone();
        legacy.set_public_key("dev-a", "");
        let ctx2 = PairingContext {
            devices: &legacy,
            account_service_reachable: true,
            passwordless_enabled: true,
        };
        let missing = decide("dev-a", &a.public_key_b64(), &ctx2).expect("判定");
        assert!(missing.requires_pairing_code);
        assert_eq!(missing.reason, "missing_public_key");

        // 出示的公钥与地址簿不一致：硬拒绝（疑似换钥）。
        let mismatch = decide("dev-a", &b.public_key_b64(), &context).unwrap_err();
        assert_eq!(mismatch.code(), "public_key_mismatch");

        // 免密开关关闭：仍要配对码。
        let disabled = PairingContext {
            devices: &list,
            account_service_reachable: true,
            passwordless_enabled: false,
        };
        let decision = decide("dev-a", &a.public_key_b64(), &disabled).expect("判定");
        assert!(decision.requires_pairing_code);
        assert_eq!(decision.reason, "passwordless_disabled");
    }

    /// F3：账号服务不可达 → 本地信任回退，免密结果不变。
    #[test]
    fn f3_account_unreachable_falls_back_to_local_trust() {
        let (list, a, b) = book();
        let offline = PairingContext {
            devices: &list,
            account_service_reachable: false,
            passwordless_enabled: true,
        };
        let trusted = decide("dev-a", &a.public_key_b64(), &offline).expect("判定");
        assert_eq!(trusted.mode, PairingMode::Passwordless);
        assert!(trusted.used_local_fallback);
        assert!(!trusted.requires_pairing_code);

        let untrusted = decide("dev-b", &b.public_key_b64(), &offline).expect("判定");
        assert!(untrusted.requires_pairing_code);
        assert!(untrusted.used_local_fallback);
    }

    /// F4：账号服务不参与 E2E；令牌字段不出现在信令包头。
    #[test]
    fn f4_token_not_in_signal_header() {
        use rdc_protocol::{SignalEnvelope, SignalMessage};

        // 账号服务角色声明必须明确「不参与端到端加密」。
        assert!(ACCOUNT_SERVICE_ROLE.contains("不参与端到端加密"));
        assert!(ACCOUNT_SERVICE_ROLE.contains("不持有会话密钥"));

        // 指令信封（呼叫做包）只含信令字段；即使令牌就在本进程内，也不得混入包头。
        let envelope = SignalEnvelope::new(
            "dev-a",
            7,
            0,
            SignalMessage::Bye {
                session: "s-1".to_owned(),
                reason: "probe".to_owned(),
            },
        );
        let bytes = envelope.encode().expect("信封编码");
        let text = String::from_utf8_lossy(&bytes);
        for forbidden in ["grant_id", "grant_token", "owner_pub", "grantee_pub", "nonce"] {
            assert!(
                !text.contains(forbidden),
                "信令包头不得包含令牌字段 {forbidden}"
            );
        }
    }

    /// 挑战-应答验签失败：文案逐字不变，source 可追到 `SecurityError`。
    #[test]
    fn challenge_failed_keeps_source() {
        let err = verify_response("不是 Base64", "challenge", "不是 Base64").expect_err("必须失败");
        assert_eq!(err.to_string(), "免密挑战-应答验签失败");
        assert_eq!(err.code(), "challenge_failed");
        let source = std::error::Error::source(&err).expect("必须挂 source");
        assert!(source.downcast_ref::<rdc_security::SecurityError>().is_some());
    }
}
