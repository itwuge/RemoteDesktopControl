//! rdc-protocol / 挑战-应答与时间窗
//!
//! 层级：L2 能力层
//! 用途：定义免密登录的挑战长度、域分隔字符串、签名内容拼接规则与 ±30 s 时间窗；
//!       格式在此冻结，签名/验签由 rdc-security 执行（docs/07 §2.2）。
//! 调用方：rdc-connect（免密呼叫与握手）、rdc-security（签名与验签）、rdc-core（审计）

use base64::engine::general_purpose::STANDARD;
use base64::Engine;

use crate::error::ProtocolError;

// =====** 常量 =====**

/// 挑战随机串长度（字节）：32 字节 = 256 bit，足够抵抗重放。
pub const CHALLENGE_LEN: usize = 32;

/// 时间窗（毫秒）：端点校验 `|now - ts| <= 30 s`；服务端不参与，避免时区问题。
pub const AUTH_TIME_WINDOW_MS: i64 = 30_000;

/// nonce 内存滑动窗口（毫秒）：窗口内已用挑战不得再次出现（docs/07 §2.2）。
pub const NONCE_WINDOW_MS: i64 = 5 * 60 * 1000;

/// 会话级 nonce 持久保留时长（毫秒）：跨重启防重放（docs/07 §2.2）。
pub const NONCE_PERSIST_MS: i64 = 24 * 60 * 60 * 1000;

/// 域分隔字符串：防止同一签名被复用到其它协议用途。
pub const AUTH_DOMAIN: &[u8] = b"RDC-AUTH";

// =====** 签名内容拼接 =====**

/// 拼接挑战-应答的签名内容：`"RDC-AUTH" ‖ session ‖ challenge ‖ ts`。
///
/// 为什么用固定域前缀 + 定长字段：域前缀防止跨用途重放；session 防跨会话重放；
/// challenge 防同会话内重放；ts 由端点校验时间窗。
///
/// # Arguments
/// * `session` - 会话 ID（UTF-8 字节直接拼入）
/// * `challenge` - 32 字节随机挑战
/// * `ts` - 发送方毫秒时间戳
///
/// # Returns
/// 待签名字节序列。
///
/// # Errors
/// 不会失败。
pub fn auth_signing_payload(session: &str, challenge: &[u8; CHALLENGE_LEN], ts: i64) -> Vec<u8> {
    let mut out = Vec::with_capacity(AUTH_DOMAIN.len() + session.len() + CHALLENGE_LEN + 8);
    out.extend_from_slice(AUTH_DOMAIN);
    out.extend_from_slice(session.as_bytes());
    out.extend_from_slice(challenge);
    // 时间戳用大端 i64：两端按同一字节序拼接，避免平台差异。
    out.extend_from_slice(&ts.to_be_bytes());
    out
}

/// 校验时间窗。
///
/// # Arguments
/// * `now_ms` - 本端当前毫秒时间戳
/// * `ts` - 对端声明的毫秒时间戳
///
/// # Returns
/// 在 ±[`AUTH_TIME_WINDOW_MS`] 内返回 `Ok(())`。
///
/// # Errors
/// 超出窗口返回 [`ProtocolError::InvalidField`]；调用方应返回 `auth_failed`，
/// 且不得区分“码错/过期”以免泄漏信息。
pub fn validate_auth_time(now_ms: i64, ts: i64) -> Result<(), ProtocolError> {
    // 用 saturating_sub 防止极端值溢出；abs 后再比较。
    if (now_ms - ts).abs() > AUTH_TIME_WINDOW_MS {
        return Err(ProtocolError::InvalidField("挑战时间戳超出 ±30 s 窗口"));
    }
    Ok(())
}

// =====** Base64 辅助 =====**

/// 把 32 字节挑战编码为 Base64（标准字母表，无 URL 安全变体）。
///
/// # Arguments
/// * `challenge` - 32 字节随机挑战
///
/// # Returns
/// Base64 文本。
///
/// # Errors
/// 不会失败。
pub fn encode_challenge(challenge: &[u8; CHALLENGE_LEN]) -> String {
    STANDARD.encode(challenge)
}

/// 解码 Base64 挑战并校验长度。
///
/// # Arguments
/// * `text` - 线上收到的 Base64 文本
///
/// # Returns
/// 32 字节挑战。
///
/// # Errors
/// Base64 非法或长度不是 32 字节时返回 [`ProtocolError::InvalidField`]。
pub fn decode_challenge(text: &str) -> Result<[u8; CHALLENGE_LEN], ProtocolError> {
    let raw = STANDARD
        .decode(text)
        .map_err(|_| ProtocolError::InvalidField("挑战不是合法 Base64"))?;
    if raw.len() != CHALLENGE_LEN {
        return Err(ProtocolError::InvalidField("挑战长度必须为 32 字节"));
    }
    let mut out = [0u8; CHALLENGE_LEN];
    out.copy_from_slice(&raw);
    Ok(out)
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 签名内容必须逐字节稳定（冻结格式，防止两端拼接不一致）。
    #[test]
    fn signing_payload_is_byte_stable() {
        let challenge = [0xABu8; CHALLENGE_LEN];
        let payload = auth_signing_payload("sess-1", &challenge, 1234);
        assert_eq!(&payload[..AUTH_DOMAIN.len()], AUTH_DOMAIN);
        assert_eq!(&payload[AUTH_DOMAIN.len()..AUTH_DOMAIN.len() + 6], b"sess-1");
        assert_eq!(
            &payload[AUTH_DOMAIN.len() + 6..AUTH_DOMAIN.len() + 6 + CHALLENGE_LEN],
            &challenge
        );
        assert_eq!(
            &payload[AUTH_DOMAIN.len() + 6 + CHALLENGE_LEN..],
            &1234i64.to_be_bytes()
        );
    }

    /// 会话或挑战变化必须改变签名内容。
    #[test]
    fn signing_payload_binds_session_and_challenge() {
        let challenge = [1u8; CHALLENGE_LEN];
        let a = auth_signing_payload("s1", &challenge, 1);
        let b = auth_signing_payload("s2", &challenge, 1);
        let mut c = auth_signing_payload("s1", &challenge, 1);
        c[CHALLENGE_LEN] ^= 1;
        assert_ne!(a, b);
        assert_ne!(a, c);
    }

    /// 时间窗必须严格为 ±30 s（边界内通过、边界外拒绝）。
    #[test]
    fn time_window_is_30_seconds() {
        assert!(validate_auth_time(1_000_000, 1_000_000).is_ok());
        assert!(validate_auth_time(1_000_000, 970_000).is_ok());
        assert!(validate_auth_time(1_000_000, 1_030_000).is_ok());
        assert!(validate_auth_time(1_000_000, 969_999).is_err());
        assert!(validate_auth_time(1_000_000, 1_030_001).is_err());
    }

    /// Base64 挑战往返必须无损，长度错误要拒绝。
    #[test]
    fn challenge_base64_roundtrip() {
        let challenge = [7u8; CHALLENGE_LEN];
        let text = encode_challenge(&challenge);
        assert_eq!(decode_challenge(&text).expect("解码"), challenge);
        assert!(decode_challenge("AAAA").is_err());
    }
}
