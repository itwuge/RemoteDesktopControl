//! rdc-security / 一次性配对码与 PAKE
//!
//! 层级：L2 能力层
//! 用途：10 位 Crockford Base32 配对码的生成/归一化，以及基于 SPAKE2 的 PAKE 交互
//!       （docs/07 §2.1）。配对码只作为 PAKE 口令，服务端全程不可见。
//! 调用方：rdc-connect（握手）、rdc-cli（Demo 展示配对码）、rdc-core（配对页）

use rand_core::{OsRng, RngCore};
use spake2::{Ed25519Group, Identity, Password, Spake2};

use crate::error::{SecurityError, Spake2LibError};

// =====** 配对码常量 =====**

/// 手动配对码长度（Crockford Base32 字符）。
pub const PAIRING_CODE_LEN: usize = 10;

/// Crockford Base32 字母表：去掉易混淆的 I/L/O/U。
pub const CROCKFORD_ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// 配对码有效熵（bit）：10 字符 × 5 bit。
pub const PAIRING_CODE_BITS: u32 = 50;

/// 配对码有效期（毫秒）。
pub const PAIRING_CODE_TTL_MS: i64 = 120_000;

/// 单码最大失败次数；达到后该码立即作废。
pub const PAIRING_CODE_MAX_FAILURES: u8 = 3;

// =====** PairingCode =====**

/// 一次性配对码（已归一化：大写、无连字符）。
#[derive(Clone, PartialEq, Eq)]
pub struct PairingCode {
    /// 10 个 Crockford 字符。
    inner: String,
}

impl std::fmt::Debug for PairingCode {
    /// 脱敏 Debug：配对码不得出现在日志（docs/07 §2.1 显示位置约束）。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PairingCode(<redacted>)")
    }
}

impl PairingCode {
    /// 随机生成新配对码。
    ///
    /// # Returns
    /// 50-bit 熵、可直接展示的配对码。
    ///
    /// # Errors
    /// 不会失败。
    pub fn generate() -> Self {
        let raw = OsRng.next_u64() & ((1u64 << PAIRING_CODE_BITS) - 1);
        let mut out = String::with_capacity(PAIRING_CODE_LEN);
        let mut value = raw;
        // 低位在前依次取 5 bit，得到 10 个字符（高位不足时补 0）。
        for _ in 0..PAIRING_CODE_LEN {
            let idx = (value & 0x1F) as usize;
            out.push(CROCKFORD_ALPHABET[idx] as char);
            value >>= 5;
        }
        // 反转字符顺序，让低位在右，视觉上更像常规编码。
        let inner: String = out.chars().rev().collect();
        Self { inner }
    }

    /// 归一化用户输入。
    ///
    /// 容错规则：忽略大小写、连字符与空白；Crockford 特有的 `O→0`、`I/L→1` 映射；
    /// 其余非法字符一律拒绝（不做模糊猜测，避免把用户输入错误当成另一个码）。
    ///
    /// # Arguments
    /// * `input` - 用户输入（可含 `XXXX-XXXX-XX` 分组）
    ///
    /// # Returns
    /// 归一化后的配对码。
    ///
    /// # Errors
    /// 长度不为 10 或含非法字符时返回 [`SecurityError::PairingCodeInvalid`]。
    pub fn parse(input: &str) -> Result<Self, SecurityError> {
        let mut normalized = String::with_capacity(PAIRING_CODE_LEN);
        for ch in input.chars() {
            if ch == '-' || ch.is_whitespace() {
                continue;
            }
            let upper = ch.to_ascii_uppercase();
            let mapped = match upper {
                'O' => '0',
                'I' | 'L' => '1',
                c => c,
            };
            if !CROCKFORD_ALPHABET.contains(&(mapped as u8)) {
                return Err(SecurityError::PairingCodeInvalid);
            }
            normalized.push(mapped);
        }
        if normalized.len() != PAIRING_CODE_LEN {
            return Err(SecurityError::PairingCodeInvalid);
        }
        Ok(Self { inner: normalized })
    }

    /// 返回归一化字符串。
    ///
    /// # Returns
    /// 10 个大写字符。
    ///
    /// # Errors
    /// 不会失败。
    pub fn as_str(&self) -> &str {
        &self.inner
    }

    /// 分组展示形式 `XXXX-XXXX-XX`（仅用于 UI，不得写入日志）。
    ///
    /// # Returns
    /// 带连字符的展示串。
    ///
    /// # Errors
    /// 不会失败。
    pub fn display_grouped(&self) -> String {
        format!(
            "{}-{}-{}",
            &self.inner[0..4],
            &self.inner[4..8],
            &self.inner[8..10]
        )
    }

    /// 作为 PAKE 口令的字节序列（归一化字符串本身）。
    ///
    /// # Errors
    /// 不会失败。
    fn password_bytes(&self) -> Vec<u8> {
        self.inner.as_bytes().to_vec()
    }
}

// =====** PakeRole =====**

/// PAKE 角色：决定使用 SPAKE2 的 A/B 侧身份。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PakeRole {
    /// 主控端（发起方，SPAKE2 的 A 侧）。
    Controller,
    /// 被控端（响应方，SPAKE2 的 B 侧）。
    Host,
}

// =====** PakeSession =====**

/// 单次 PAKE 交互状态。
pub struct PakeSession {
    /// SPAKE2 状态机；`finish` 时消费。
    state: Option<Spake2<Ed25519Group>>,
    /// 角色。
    role: PakeRole,
}

impl PakeSession {
    /// 启动 PAKE 并返回需要发给对端的第一条消息。
    ///
    /// # Arguments
    /// * `role` - 本端角色
    /// * `code` - 配对码（双方必须相同）
    ///
    /// # Returns
    /// 会话状态与 PAKE 消息字节（随 HELLO / HELLO_ACK 传输）。
    ///
    /// # Errors
    /// 不会失败（密码与身份均为编译期固定格式）。
    pub fn start(role: PakeRole, code: &PairingCode) -> (Self, Vec<u8>) {
        // 身份串固定且与角色绑定：防止同一口令在另一协议上下文中复用。
        let id_controller = Identity::new(b"rdc-controller");
        let id_host = Identity::new(b"rdc-host");
        let password = Password::new(code.password_bytes());
        let (state, msg) = match role {
            PakeRole::Controller => {
                Spake2::<Ed25519Group>::start_a(&password, &id_controller, &id_host)
            }
            PakeRole::Host => {
                Spake2::<Ed25519Group>::start_b(&password, &id_controller, &id_host)
            }
        };
        (
            Self {
                state: Some(state),
                role,
            },
            msg,
        )
    }

    /// 消费对端 PAKE 消息并输出 k_pake。
    ///
    /// # Arguments
    /// * `peer_msg` - 对端随 HELLO/HELLO_ACK 发来的 PAKE 消息
    ///
    /// # Returns
    /// k_pake 字节（交给 [`crate::keys::derive_channel_keys`] 作为 HKDF salt）。
    ///
    /// # Errors
    /// 重复调用、消息长度/侧别不匹配时返回 [`SecurityError::PakeFailure`]。
    pub fn finish(&mut self, peer_msg: &[u8]) -> Result<Vec<u8>, SecurityError> {
        let state = self
            .state
            .take()
            .ok_or(SecurityError::PakeFailure {
                message: "PAKE 已完成，不能重复 finish",
                source: None,
            })?;
        state
            .finish(peer_msg)
            .map_err(|error| SecurityError::PakeFailure {
                message: "PAKE 消息不匹配或口令不一致",
                source: Some(Box::new(Spake2LibError(error))),
            })
    }

    /// 返回本端角色。
    ///
    /// # Returns
    /// 角色。
    ///
    /// # Errors
    /// 不会失败。
    pub fn role(&self) -> PakeRole {
        self.role
    }
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 归一化必须支持分组、大小写与 Crockford 映射。
    #[test]
    fn pairing_code_normalization() {
        let code = PairingCode::parse("abcd-efgh-jk").expect("合法");
        assert_eq!(code.as_str(), "ABCDEFGHJK");
        // O→0, I/L→1
        let mapped = PairingCode::parse("O0IL-1234-56").expect("映射");
        assert_eq!(mapped.as_str(), "0011123456");
        assert!(PairingCode::parse("ABC").is_err());
        assert!(PairingCode::parse("UUUU-UUUU-UU").is_err());
    }

    /// 生成的码必须可被自身解析，且分组展示正确。
    #[test]
    fn generated_code_roundtrips() {
        let code = PairingCode::generate();
        assert_eq!(code.as_str().len(), PAIRING_CODE_LEN);
        let grouped = code.display_grouped();
        assert_eq!(grouped.len(), 12);
        assert_eq!(PairingCode::parse(&grouped).expect("解析"), code);
    }

    /// 相同配对码的两端必须得到相同 k_pake；不同码必须不同。
    #[test]
    fn pake_agrees_only_with_same_code() {
        let code = PairingCode::parse("ABCD-EFGH-JK").expect("合法");
        let (mut c, msg_c) = PakeSession::start(PakeRole::Controller, &code);
        let (mut h, msg_h) = PakeSession::start(PakeRole::Host, &code);
        let kc = c.finish(&msg_h).expect("主控端 PAKE");
        let kh = h.finish(&msg_c).expect("被控端 PAKE");
        assert_eq!(kc, kh);

        let wrong = PairingCode::parse("ABCD-EFGH-J1").expect("合法");
        let (mut c2, msg_c2) = PakeSession::start(PakeRole::Controller, &code);
        let (mut h2, msg_h2) = PakeSession::start(PakeRole::Host, &wrong);
        let kc2 = c2.finish(&msg_h2).expect("PAKE 仍会完成");
        let kh2 = h2.finish(&msg_c2).expect("PAKE 仍会完成");
        assert_ne!(kc2, kh2, "不同配对码必须派生出不同密钥");
    }

    /// PAKE 不能重复 finish，防止状态被复用。
    #[test]
    fn pake_cannot_finish_twice() {
        let code = PairingCode::parse("ABCD-EFGH-JK").expect("合法");
        let (mut c, msg_c) = PakeSession::start(PakeRole::Controller, &code);
        let (_h, msg_h) = PakeSession::start(PakeRole::Host, &code);
        c.finish(&msg_h).expect("首次");
        assert!(c.finish(&msg_c).is_err());
    }
}
