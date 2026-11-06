//! rdc-security · 身份密钥、AEAD 与握手状态机
//!
//! 层级：L2 能力层（依赖 rdc-protocol，不含网络栈与平台 API）
//! 用途：定义身份密钥对、临时密钥、AEAD 对称加密、握手状态机与权限位集合；
//!       S1 只提供可编译的类型骨架，密码学实现在 S2 落地
//! 调用方：rdc-connect（建链握手与密钥交换）、rdc-core（会话权限校验）、
//!         rdc-bridge（权限开关的 FFI 映射）

// =====** 模块声明 =====**

pub mod aead;
pub mod error;
pub mod handshake;
pub mod keys;
pub mod permission;

// =====** 对外再导出 =====**

pub use aead::AeadKey;
pub use error::SecurityError;
pub use handshake::{Handshake, HandshakeRole, HandshakeState};
pub use keys::{EphemeralKey, IdentityKeyPair};
pub use permission::PermissionSet;
