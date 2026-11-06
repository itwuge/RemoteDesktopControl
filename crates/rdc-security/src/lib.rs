//! rdc-security · 身份密钥、AEAD、PAKE 与握手状态机
//!
//! 层级：L2 能力层（依赖 rdc-protocol，不含网络栈与平台 API）
//! 用途：Ed25519 身份、X25519 临时密钥、HKDF 分通道派生、AEAD 与重放窗口、
//!       一次性配对码 + SPAKE2 + 4 位 SAS、4 报文握手状态机、失败退避计数。
//! 调用方：rdc-connect（建链握手与通道加密）、rdc-signal（限流与 token 校验）、
//!         rdc-core（权限/审计）、rdc-cli（Demo）
//!
//! 纪律：本 crate 不允许出现 socket 与平台 API；所有密钥材料不得进入 Debug/日志。

// =====** 模块声明 =====**

pub mod aead;
pub mod error;
pub mod handshake;
pub mod keys;
pub mod pake;
pub mod permission;
pub mod throttle;

// =====** 对外再导出 =====**

pub use aead::{nonce_from_counter, AeadKey, CipherSuite, ReplayWindow, NONCE_LEN, TAG_LEN};
pub use error::SecurityError;
pub use handshake::{
    sign_auth_challenge, verify_auth_challenge, HandshakeMessage, HandshakeRole, HandshakeState,
    SessionHandshake,
};
pub use keys::{
    derive_channel_keys, derive_sas, from_b64_exact, hmac_sha256_b64, verify_hmac_sha256_b64,
    verify_signature_b64, ChannelKeys, EphemeralKey, IdentityKeyPair,
};
pub use pake::{
    PakeRole, PakeSession, PairingCode, PAIRING_CODE_TTL_MS, PAIRING_CODE_MAX_FAILURES,
};
pub use permission::PermissionSet;
pub use throttle::{FailureLimiter, ThrottlePolicy, ThrottleReject};
