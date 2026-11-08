//! rdc-core / S5 授权内核（多账号共享控制）
//!
//! 层级：L3 编排层
//! 用途：授权令牌 `grant_token` 的签发与离线校验、权限模板映射、独占/接管/踢出/吊销
//!       状态机，以及「观看者禁止注入」的唯一硬闸。
//! 调用方：rdc-bridge（权限页）、rdc-cli（S5-5 验证）、未来的 rdc-connect 握手侧。
//!
//! 关键约束（docs/07 §4.4）：
//! - 令牌由 **owner 设备** Ed25519 私钥签名，被控端用地址簿公钥离线验签；
//! - 权限位是 `u16` 九位位图，`POWER` / `DISPLAY_SWITCH` 不进默认模板；
//! - 授权**不可转授**，权限只能削减不能追加；
//! - 观看者注入在判定层直接拒绝（错误码 `viewer_readonly`）。
//!
//! 与旧 `rdc_security::permission::PermissionSet`（u32、五位）的关系：本模块按
//! docs/07 位序自带 [`PermBits`]，旧类型不动；仅在边界显式换算，冲突已记决策待办。

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use rdc_security::{verify_signature_b64, IdentityKeyPair};
use serde::{Deserialize, Serialize};

use crate::audit::AuditLog;
use crate::authz::AuthzDecision;
use crate::s5::multi::SessionRole;

// =====** 常量 =====**

/// 一天毫秒数。
pub const DAY_MS: u64 = 86_400_000;
/// 默认有效期：30 天。
pub const DEFAULT_VALIDITY_MS: u64 = 30 * DAY_MS;
/// 允许签发的最长有效期：90 天。
pub const MAX_VALIDITY_MS: u64 = 90 * DAY_MS;
/// 已用 nonce 的保留窗口：24 小时（docs/07 §4.4「已用 nonce 表（24h）」）。
pub const NONCE_WINDOW_MS: u64 = DAY_MS;
/// 控制者让位等待：15 秒（超时视为让位）。
pub const CONTROLLER_RESPONSE_TIMEOUT_MS: u64 = 15_000;
/// 本机二次确认等待：30 秒（超时拒绝）。
pub const LOCAL_CONFIRM_TIMEOUT_MS: u64 = 30_000;
/// 踢出从下发到停止的目标上限：3 秒。
pub const KICK_EFFECT_MAX_MS: u64 = 3_000;
/// 撤销最终一致上限：15 分钟 + 90 秒。
pub const REVOCATION_PROPAGATION_MAX_MS: u64 = 15 * 60_000 + 90_000;
/// 权限页/客户端撤销文案（**不得**写「立即生效」）。
pub const REVOCATION_UI_TEXT: &str =
    "撤销将在最长 15 分 90 秒内在各设备生效；本机可立即断开当前会话（踢出）。";
/// 授权文件版本。
pub const GRANT_FILE_VERSION: u32 = 1;
/// 原子写临时文件后缀。
const TEMP_SUFFIX: &str = ".tmp";

// =====** PermBits =====**

/// docs/07 §4 的九位权限位图。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[repr(transparent)]
pub struct PermBits(
    /// 原始位图，位序严格按 docs/07 表格自上而下。
    pub u16,
);

impl PermBits {
    /// bit0：观看画面。
    pub const VIEW: u16 = 1 << 0;
    /// bit1：注入键鼠。
    pub const CONTROL: u16 = 1 << 1;
    /// bit2：文本剪贴板。
    pub const CLIPBOARD_TEXT: u16 = 1 << 2;
    /// bit3：图片剪贴板。
    pub const CLIPBOARD_IMAGE: u16 = 1 << 3;
    /// bit4：文件传输。
    pub const FILE_TRANSFER: u16 = 1 << 4;
    /// bit5：音频回传。
    pub const AUDIO: u16 = 1 << 5;
    /// bit6：电源操作（单次确认）。
    pub const POWER: u16 = 1 << 6;
    /// bit7：切换显示器。
    pub const DISPLAY_SWITCH: u16 = 1 << 7;
    /// bit8：断线自动重连。
    pub const RECONNECT: u16 = 1 << 8;
    /// 全部已定义位。
    pub const ALL: u16 = 0x1FF;
    /// 敏感位：必须本机二次确认，且不得进入默认模板。
    pub const SENSITIVE: u16 = Self::POWER | Self::DISPLAY_SWITCH;
    /// 「完全控制」模板可包含的安全位（不含敏感位）。
    pub const SAFE_FULL: u16 = Self::ALL & !Self::SENSITIVE;

    /// 构造。
    ///
    /// # Arguments
    /// * `bits` - 原始位图（未定义位会被忽略）
    ///
    /// # Returns
    /// 已剔除未定义位的集合。
    ///
    /// # Errors
    /// 不会失败。
    pub const fn new(bits: u16) -> Self {
        Self(bits & Self::ALL)
    }

    /// 是否包含全部指定位。
    ///
    /// # Arguments
    /// * `mask` - 位掩码
    ///
    /// # Returns
    /// 全部置位时返回 `true`；`mask` 为 0 恒为 `true`。
    ///
    /// # Errors
    /// 不会失败。
    pub const fn contains(self, mask: u16) -> bool {
        self.0 & mask == mask
    }

    /// 两个集合的交集。
    ///
    /// # Arguments
    /// * `other` - 另一个集合
    ///
    /// # Returns
    /// 双方都授予的位。
    ///
    /// # Errors
    /// 不会失败。
    pub const fn intersect(self, other: PermBits) -> Self {
        Self(self.0 & other.0)
    }

    /// 是否为另一集合的子集（只能削减的判定基础）。
    ///
    /// # Arguments
    /// * `parent` - 上限集合
    ///
    /// # Returns
    /// `self` 的每一位都在 `parent` 中时返回 `true`。
    ///
    /// # Errors
    /// 不会失败。
    pub const fn is_subset_of(self, parent: PermBits) -> bool {
        self.0 & !parent.0 == 0
    }

    /// 原始位图。
    ///
    /// # Returns
    /// u16 位图。
    ///
    /// # Errors
    /// 不会失败。
    pub const fn to_bits(self) -> u16 {
        self.0
    }
}

// =====** 模板 =====**

/// 授权模板。
///
/// 三档模板都**不含** `POWER` / `DISPLAY_SWITCH`（docs/07 §4 脚注）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GrantTemplate {
    /// 只观看：仅 `VIEW`。
    ReadOnly,
    /// 可操作：观看 + 键鼠 + 文本剪贴板。
    Operate,
    /// 完全控制：可操作 + 文件 + 音频 + 重连（仍不含敏感位）。
    FullControl,
}

impl GrantTemplate {
    /// 解析模板名（兼容页面旧值 `standard` / `full`）。
    ///
    /// # Arguments
    /// * `value` - 模板名
    ///
    /// # Returns
    /// 命中返回模板；未知返回 `None`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "readonly" => Some(Self::ReadOnly),
            "standard" | "operate" => Some(Self::Operate),
            "full" | "full_control" => Some(Self::FullControl),
            _ => None,
        }
    }

    /// 模板对应权限位。
    ///
    /// # Returns
    /// 权限位集合。
    ///
    /// # Errors
    /// 不会失败。
    pub const fn bits(self) -> PermBits {
        match self {
            Self::ReadOnly => PermBits(PermBits::VIEW),
            Self::Operate => PermBits(PermBits::VIEW | PermBits::CONTROL | PermBits::CLIPBOARD_TEXT),
            Self::FullControl => PermBits(PermBits::SAFE_FULL),
        }
    }

    /// 稳定英文名（写审计与跨 FFI）。
    ///
    /// # Returns
    /// `readonly` / `operate` / `full`。
    ///
    /// # Errors
    /// 不会失败。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "readonly",
            Self::Operate => "operate",
            Self::FullControl => "full",
        }
    }

    /// 中文名（页面展示）。
    ///
    /// # Returns
    /// `只观看` / `可操作` / `完全控制`。
    ///
    /// # Errors
    /// 不会失败。
    pub const fn display_zh(self) -> &'static str {
        match self {
            Self::ReadOnly => "只观看",
            Self::Operate => "可操作",
            Self::FullControl => "完全控制",
        }
    }
}

/// 按模板名取权限位。
///
/// # Arguments
/// * `value` - 模板名
///
/// # Returns
/// 命中返回权限位；未知返回 `None`。
///
/// # Errors
/// 不会失败。
pub fn template_bits(value: &str) -> Option<PermBits> {
    GrantTemplate::parse(value).map(GrantTemplate::bits)
}

// =====** 错误码 =====**

/// 授权校验错误（每个变体对应一个固定错误码，便于审计与页面展示）。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GrantError {
    /// 令牌绑定的 host 与本机不一致。
    #[error("授权绑定的设备与本机不一致")]
    HostMismatch,
    /// 签发者不在地址簿。
    #[error("授权签发者未被本机信任")]
    OwnerUntrusted,
    /// 令牌授权的对象不是本次连接的对端。
    #[error("令牌授权的对象与连接对端不一致")]
    GranteeMismatch,
    /// 签名为空、格式错误或验签失败。
    #[error("授权签名无效")]
    SignatureInvalid,
    /// 尚未到生效时间。
    #[error("授权尚未生效")]
    NotYetValid,
    /// 已过期。
    #[error("授权已过期")]
    Expired,
    /// 有效期超过 90 天或为 0。
    #[error("授权有效期非法（默认 30 天，最长 90 天）")]
    ValidityTooLong,
    /// 权限位超过可授予上限（只能削减）。
    #[error("授权权限超出发起方可授予范围")]
    PermExceeded,
    /// 含敏感位但本机未允许。
    #[error("电源/切换显示器需要本机二次确认")]
    SensitiveNeedsConfirm,
    /// 撤销世代回退。
    #[error("撤销世代回退（疑似回滚攻击）")]
    EpochRegression,
    /// nonce 已使用。
    #[error("授权 nonce 已使用（疑似重放）")]
    NonceReused,
    /// 参数非法（如未定义位）。
    #[error("授权参数非法：{0}")]
    Invalid(String),
}

impl GrantError {
    /// 固定错误码。
    ///
    /// # Returns
    /// 用于审计与 UI 的稳定标识。
    ///
    /// # Errors
    /// 不会失败。
    pub fn code(&self) -> &'static str {
        match self {
            Self::HostMismatch => "host_mismatch",
            Self::OwnerUntrusted => "owner_untrusted",
            Self::GranteeMismatch => "grantee_mismatch",
            Self::SignatureInvalid => "signature_invalid",
            Self::NotYetValid => "not_yet_valid",
            Self::Expired => "expired",
            Self::ValidityTooLong => "validity_too_long",
            Self::PermExceeded => "perm_exceeded",
            Self::SensitiveNeedsConfirm => "sensitive_needs_confirm",
            Self::EpochRegression => "epoch_regression",
            Self::NonceReused => "nonce_reused",
            Self::Invalid(_) => "invalid_argument",
        }
    }
}

// =====** GrantToken =====**

/// 授权令牌（可离线验签的完整字段集）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantToken {
    /// 授权 ID。
    pub grant_id: String,
    /// 授权方（owner）身份公钥（Base64）。
    pub owner_pub: String,
    /// 被控端（host）身份公钥（Base64）；防止把 A 机器的授权挪到 B 机器。
    pub host_pub: String,
    /// 被授权方（grantee）身份公钥（Base64）。
    pub grantee_pub: String,
    /// 权限位（u16）。
    pub perms: u16,
    /// 生效时间（Unix 毫秒）。
    pub not_before: u64,
    /// 到期时间（Unix 毫秒）。
    pub expires_at: u64,
    /// 一次性随机串（防重放）。
    pub nonce: String,
    /// 撤销世代基线（单调递增）。
    pub revoked_epoch_base: u64,
}

impl GrantToken {
    /// 规范化签名字节。
    ///
    /// 为什么用固定分隔串而不是 JSON：字段顺序、空白与转义都可能随实现变化，
    /// 手写规范串能保证两端/两版本签名字节完全一致。
    ///
    /// # Returns
    /// UTF-8 字节。
    ///
    /// # Errors
    /// 不会失败。
    pub fn canonical_bytes(&self) -> Vec<u8> {
        format!(
            "RDC-grant-v1|{}|{}|{}|{}|{}|{}|{}|{}|{}",
            self.grant_id,
            self.owner_pub,
            self.host_pub,
            self.grantee_pub,
            self.perms,
            self.not_before,
            self.expires_at,
            self.nonce,
            self.revoked_epoch_base
        )
        .into_bytes()
    }

    /// 权限位视图。
    ///
    /// # Returns
    /// [`PermBits`]。
    ///
    /// # Errors
    /// 不会失败。
    pub const fn perms(&self) -> PermBits {
        PermBits::new(self.perms)
    }
}

/// 签发结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedGrant {
    /// 令牌。
    pub token: GrantToken,
    /// owner 私钥签名（Base64）。
    pub signature: String,
}

/// 签发参数。
#[derive(Debug, Clone)]
pub struct GrantSpec<'a> {
    /// 被控端（本机）身份公钥。
    pub host_pub: &'a str,
    /// 被授权方身份公钥。
    pub grantee_pub: &'a str,
    /// 授权权限位。
    pub perms: PermBits,
    /// 有效期（毫秒）；`None` 用默认 30 天。
    pub validity_ms: Option<u64>,
    /// 撤销世代基线。
    pub revoked_epoch_base: u64,
}

/// 生成唯一标识（本地签发使用，不依赖随机源）。
///
/// 为什么不用 rand：令牌 ID / nonce 只需在本机单调唯一，时钟 + 原子计数已足够，
/// 也避免在 L3 编排层引入额外的密码学随机依赖。
fn unique_id(prefix: &str, now_ms: u64) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("{prefix}-{now_ms:x}-{seq:x}")
}

/// 签发授权令牌。
///
/// # Arguments
/// * `identity` - owner 设备身份密钥
/// * `spec` - 签发参数
/// * `now_ms` - 当前 Unix 毫秒
///
/// # Returns
/// 令牌与签名。
///
/// # Errors
/// 有效期非法（0 或 > 90 天）、权限位含未定义位时返回 [`GrantError`]。
pub fn issue_grant(
    identity: &IdentityKeyPair,
    spec: &GrantSpec<'_>,
    now_ms: u64,
) -> Result<SignedGrant, GrantError> {
    let validity = spec.validity_ms.unwrap_or(DEFAULT_VALIDITY_MS);
    if validity == 0 || validity > MAX_VALIDITY_MS {
        return Err(GrantError::ValidityTooLong);
    }
    if spec.perms.to_bits() & !PermBits::ALL != 0 {
        return Err(GrantError::Invalid("权限位含未定义位".to_owned()));
    }
    let token = GrantToken {
        grant_id: unique_id("g", now_ms),
        owner_pub: identity.public_key_b64(),
        host_pub: spec.host_pub.to_owned(),
        grantee_pub: spec.grantee_pub.to_owned(),
        perms: spec.perms.to_bits(),
        not_before: now_ms,
        expires_at: now_ms.saturating_add(validity),
        nonce: unique_id("n", now_ms),
        revoked_epoch_base: spec.revoked_epoch_base,
    };
    let signature = identity.sign_b64(&token.canonical_bytes());
    Ok(SignedGrant { token, signature })
}

// =====** 已用 nonce 表 =====**

/// 已用 nonce 表（默认保留 24 小时）。
#[derive(Debug, Clone, Default)]
pub struct ReplayGuard {
    /// nonce → 首次出现时间。
    used: BTreeMap<String, u64>,
}

impl ReplayGuard {
    /// 空表。
    ///
    /// # Returns
    /// 不含记录的表。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new() -> Self {
        Self::default()
    }

    /// 检查并登记 nonce。
    ///
    /// # Arguments
    /// * `nonce` - 令牌 nonce
    /// * `now_ms` - 当前 Unix 毫秒
    ///
    /// # Returns
    /// 首次出现返回 `true`（已登记）；重复返回 `false`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn check_and_mark(&mut self, nonce: &str, now_ms: u64) -> bool {
        self.prune(now_ms);
        if self.used.contains_key(nonce) {
            return false;
        }
        self.used.insert(nonce.to_owned(), now_ms);
        true
    }

    /// 清理超过 24 小时的记录。
    fn prune(&mut self, now_ms: u64) {
        let cutoff = now_ms.saturating_sub(NONCE_WINDOW_MS);
        self.used.retain(|_, seen| *seen >= cutoff);
    }
}

// =====** 校验 =====**

/// 校验上下文（由被控端组装）。
#[derive(Debug, Clone)]
pub struct VerifyContext<'a> {
    /// 本机身份公钥。
    pub local_host_pub: &'a str,
    /// 本次连接的对端身份公钥；`None` 表示离线批量校验，跳过对端绑定检查。
    pub peer_pub: Option<&'a str>,
    /// 地址簿里已配对的 owner 公钥（信任根，不从网络拉取）。
    pub trusted_owner_pubs: &'a [String],
    /// 本机允许授予的最大权限位。
    pub owner_ceiling: PermBits,
    /// 本机当前撤销世代。
    pub local_revoked_epoch: u64,
    /// 是否允许敏感位（`POWER` / `DISPLAY_SWITCH` 需本机二次确认后置 true）。
    pub allow_sensitive_bits: bool,
}

/// 校验通过后的有效授权。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedGrant {
    /// 原令牌（调用方只读）。
    pub token: GrantToken,
    /// 会话实际生效的权限位（已按上限削减）。
    pub effective_perms: PermBits,
}

/// 五步离线校验（任一失败即拒绝）。
///
/// # Arguments
/// * `token` - 令牌
/// * `signature` - owner 签名
/// * `context` - 本机校验上下文
/// * `now_ms` - 当前 Unix 毫秒
/// * `replay` - 已用 nonce 表
///
/// # Returns
/// 校验通过返回有效权限。
///
/// # Errors
/// 任一步失败返回带固定错误码的 [`GrantError`]。
pub fn verify_grant(
    token: &GrantToken,
    signature: &str,
    context: &VerifyContext<'_>,
    now_ms: u64,
    replay: &mut ReplayGuard,
) -> Result<VerifiedGrant, GrantError> {
    // 步骤 1：host 绑定。
    if token.host_pub != context.local_host_pub {
        return Err(GrantError::HostMismatch);
    }
    // 步骤 2：owner 必须在地址簿中，且签名可验。
    if !context
        .trusted_owner_pubs
        .iter()
        .any(|key| key == &token.owner_pub)
    {
        return Err(GrantError::OwnerUntrusted);
    }
    verify_signature_b64(&token.owner_pub, &token.canonical_bytes(), signature)
        .map_err(|_| GrantError::SignatureInvalid)?;
    // 步骤 2b：令牌必须确实签给本次连接的对端，防止 A 的令牌被 B 拿去用。
    if let Some(peer) = context.peer_pub {
        if token.grantee_pub != peer {
            return Err(GrantError::GranteeMismatch);
        }
    }
    // 步骤 3：时间窗与有效期上限。
    if now_ms < token.not_before {
        return Err(GrantError::NotYetValid);
    }
    if now_ms >= token.expires_at {
        return Err(GrantError::Expired);
    }
    let validity = token.expires_at.saturating_sub(token.not_before);
    if validity == 0 || validity > MAX_VALIDITY_MS {
        return Err(GrantError::ValidityTooLong);
    }
    // 步骤 4：权限只能削减；敏感位需本机允许。
    let requested = token.perms();
    if !requested.is_subset_of(context.owner_ceiling) {
        return Err(GrantError::PermExceeded);
    }
    if token
        .perms()
        .to_bits()
        & PermBits::SENSITIVE
        != 0
        && !context.allow_sensitive_bits
    {
        return Err(GrantError::SensitiveNeedsConfirm);
    }
    // 步骤 5：撤销世代单调 + nonce 未用。
    if token.revoked_epoch_base < context.local_revoked_epoch {
        return Err(GrantError::EpochRegression);
    }
    if !replay.check_and_mark(&token.nonce, now_ms) {
        return Err(GrantError::NonceReused);
    }
    Ok(VerifiedGrant {
        token: token.clone(),
        effective_perms: requested.intersect(context.owner_ceiling),
    })
}

/// 会话权限 = 请求 ∩ 令牌权限 ∩ 本机允许。
///
/// # Arguments
/// * `requested` - 主控端在 call 中请求的权限位
/// * `token` - 令牌权限
/// * `local_allowed` - 本机允许权限
///
/// # Returns
/// 三者交集（只会变小）。
///
/// # Errors
/// 不会失败。
pub const fn session_perms(requested: PermBits, token: PermBits, local_allowed: PermBits) -> PermBits {
    requested.intersect(token).intersect(local_allowed)
}

/// 输入注入硬闸：观看者一律拒绝，与令牌权限无关。
///
/// # Arguments
/// * `role` - 会话角色
/// * `effective_perms` - 该会话实际生效的权限位
///
/// # Returns
/// 允许/拒绝；观看者固定错误码 `viewer_readonly`。
///
/// # Errors
/// 不会失败。
pub fn authorize_input_with_grant(role: SessionRole, effective_perms: PermBits) -> AuthzDecision {
    match role {
        SessionRole::Viewer => AuthzDecision::deny("viewer_readonly"),
        SessionRole::Controller => {
            if effective_perms.contains(PermBits::CONTROL) {
                AuthzDecision::allow("grant_control")
            } else {
                AuthzDecision::deny("control_not_granted")
            }
        }
    }
}

// =====** 独占与接管 =====**

/// 接管策略（与 settings 的 `takeover_policy` 同值域）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TakeoverPolicy {
    /// 自动同意：控制者让位后直接转交。
    Auto,
    /// 需本人确认：控制者让位后本机确认，30 秒超时拒绝。
    Confirm,
    /// 禁止接管。
    Forbid,
}

impl TakeoverPolicy {
    /// 解析策略名。
    ///
    /// # Arguments
    /// * `value` - `auto` / `confirm` / `forbid`
    ///
    /// # Returns
    /// 未知返回默认 `confirm`（安全默认）。
    ///
    /// # Errors
    /// 不会失败。
    pub fn parse(value: &str) -> Self {
        match value {
            "auto" => Self::Auto,
            "forbid" => Self::Forbid,
            _ => Self::Confirm,
        }
    }

    /// 稳定英文名。
    ///
    /// # Returns
    /// 策略名。
    ///
    /// # Errors
    /// 不会失败。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Confirm => "confirm",
            Self::Forbid => "forbid",
        }
    }
}

/// 控制权决策。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlDecision {
    /// 已获得控制权。
    Granted {
        /// 控制者会话 ID。
        session_id: u64,
    },
    /// 等待当前控制者响应（15 秒）。
    PendingControllerResponse {
        /// 申请者会话 ID。
        requester: u64,
        /// 截止时间（Unix 毫秒）。
        deadline_ms: u64,
    },
    /// 等待本机二次确认（30 秒）。
    PendingOwnerConfirm {
        /// 申请者会话 ID。
        requester: u64,
        /// 截止时间（Unix 毫秒）。
        deadline_ms: u64,
    },
    /// 被拒绝。
    Denied {
        /// 固定错误码。
        code: &'static str,
    },
}

/// 待决阶段。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PendingPhase {
    /// 等控制者响应。
    Controller,
    /// 等本机确认。
    Owner,
}

/// 待决接管。
#[derive(Debug, Clone)]
struct Pending {
    /// 申请者。
    requester: u64,
    /// 阶段。
    phase: PendingPhase,
    /// 截止时间。
    deadline_ms: u64,
    /// 原控制者（让位后写审计用）。
    previous_controller: Option<u64>,
}

/// 独占控制权仲裁器（注入时钟，便于可判定测试）。
#[derive(Debug, Clone)]
pub struct ControlArbiter {
    /// 当前控制者。
    controller: Option<u64>,
    /// 待决接管。
    pending: Option<Pending>,
    /// 接管策略。
    policy: TakeoverPolicy,
}

impl ControlArbiter {
    /// 构造。
    ///
    /// # Arguments
    /// * `policy` - 接管策略
    ///
    /// # Returns
    /// 无控制者的仲裁器。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new(policy: TakeoverPolicy) -> Self {
        Self {
            controller: None,
            pending: None,
            policy,
        }
    }

    /// 当前控制者。
    ///
    /// # Returns
    /// 无控制者返回 `None`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn controller(&self) -> Option<u64> {
        self.controller
    }

    /// 是否有待决接管。
    ///
    /// # Returns
    /// 存在待决返回 `true`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    /// 申请控制权。
    ///
    /// # Arguments
    /// * `session_id` - 申请会话 ID
    /// * `now_ms` - 当前 Unix 毫秒
    /// * `audit` - 审计日志
    ///
    /// # Returns
    /// 直接授予、进入待决或拒绝。
    ///
    /// # Errors
    /// 不会失败。
    pub fn request_control(
        &mut self,
        session_id: u64,
        now_ms: u64,
        audit: &mut AuditLog,
    ) -> ControlDecision {
        if self.controller == Some(session_id) {
            return ControlDecision::Granted { session_id };
        }
        if self.pending.is_some() {
            audit.record(
                now_ms,
                "takeover",
                &session_id.to_string(),
                "request",
                None,
                None,
                "denied",
            );
            return ControlDecision::Denied {
                code: "takeover_pending",
            };
        }
        let Some(previous) = self.controller else {
            self.controller = Some(session_id);
            audit.record(
                now_ms,
                "connect",
                &session_id.to_string(),
                "grant_control",
                None,
                Some("controller"),
                "allowed",
            );
            return ControlDecision::Granted { session_id };
        };
        if self.policy == TakeoverPolicy::Forbid {
            audit.record(
                now_ms,
                "takeover",
                &session_id.to_string(),
                "request",
                Some(&format!("controller={previous}")),
                None,
                "denied",
            );
            return ControlDecision::Denied {
                code: "takeover_forbidden",
            };
        }
        let deadline = now_ms.saturating_add(CONTROLLER_RESPONSE_TIMEOUT_MS);
        self.pending = Some(Pending {
            requester: session_id,
            phase: PendingPhase::Controller,
            deadline_ms: deadline,
            previous_controller: Some(previous),
        });
        audit.record(
            now_ms,
            "takeover",
            &session_id.to_string(),
            "request",
            Some(&format!("controller={previous}")),
            Some("pending_controller_response"),
            "pending",
        );
        ControlDecision::PendingControllerResponse {
            requester: session_id,
            deadline_ms: deadline,
        }
    }

    /// 当前控制者响应接管（允许让位 / 拒绝）。
    ///
    /// # Arguments
    /// * `accept` - 是否让位
    /// * `now_ms` - 当前 Unix 毫秒
    /// * `audit` - 审计日志
    ///
    /// # Returns
    /// 下一阶段决策。
    ///
    /// # Errors
    /// 无待决接管时返回 `Denied { code: "no_pending_takeover" }`。
    pub fn controller_response(
        &mut self,
        accept: bool,
        now_ms: u64,
        audit: &mut AuditLog,
    ) -> ControlDecision {
        let Some(pending) = self.pending.clone() else {
            return ControlDecision::Denied {
                code: "no_pending_takeover",
            };
        };
        if pending.phase != PendingPhase::Controller {
            return ControlDecision::Denied {
                code: "controller_response_too_late",
            };
        }
        if !accept {
            self.pending = None;
            audit.record(
                now_ms,
                "takeover",
                &pending.requester.to_string(),
                "controller_deny",
                Some("pending_controller_response"),
                Some("denied"),
                "denied",
            );
            return ControlDecision::Denied {
                code: "controller_denied",
            };
        }
        self.advance_after_yield(pending, now_ms, audit)
    }

    /// 推进时钟：处理 15 秒让位与 30 秒本机确认超时。
    ///
    /// # Arguments
    /// * `now_ms` - 当前 Unix 毫秒
    /// * `audit` - 审计日志
    ///
    /// # Returns
    /// 有状态迁移时返回新决策；无待决返回 `None`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn tick(&mut self, now_ms: u64, audit: &mut AuditLog) -> Option<ControlDecision> {
        let pending = self.pending.clone()?;
        if now_ms < pending.deadline_ms {
            return None;
        }
        match pending.phase {
            PendingPhase::Controller => {
                // 15 秒未响应视为让位。
                self.advance_after_yield(pending, now_ms, audit).into()
            }
            PendingPhase::Owner => {
                // 本机 30 秒内未确认：拒绝。
                self.pending = None;
                audit.record(
                    now_ms,
                    "takeover",
                    &pending.requester.to_string(),
                    "local_confirm_timeout",
                    Some("pending_owner_confirm"),
                    Some("denied"),
                    "denied",
                );
                Some(ControlDecision::Denied {
                    code: "local_confirm_timeout",
                })
            }
        }
    }

    /// 本机二次确认接管。
    ///
    /// # Arguments
    /// * `approve` - 是否同意
    /// * `now_ms` - 当前 Unix 毫秒
    /// * `audit` - 审计日志
    ///
    /// # Returns
    /// 授予或拒绝。
    ///
    /// # Errors
    /// 阶段不符时返回 `Denied { code: "no_pending_owner_confirm" }`。
    pub fn owner_confirm(
        &mut self,
        approve: bool,
        now_ms: u64,
        audit: &mut AuditLog,
    ) -> ControlDecision {
        let Some(pending) = self.pending.clone() else {
            return ControlDecision::Denied {
                code: "no_pending_owner_confirm",
            };
        };
        if pending.phase != PendingPhase::Owner {
            return ControlDecision::Denied {
                code: "no_pending_owner_confirm",
            };
        }
        self.pending = None;
        if !approve {
            audit.record(
                now_ms,
                "takeover",
                &pending.requester.to_string(),
                "owner_deny",
                Some("pending_owner_confirm"),
                Some("denied"),
                "denied",
            );
            return ControlDecision::Denied {
                code: "owner_denied",
            };
        }
        let old = pending
            .previous_controller
            .map(|id| format!("controller={id}"))
            .unwrap_or_else(|| "controller=none".to_owned());
        self.controller = Some(pending.requester);
        audit.record(
            now_ms,
            "takeover",
            &pending.requester.to_string(),
            "owner_approve",
            Some(&old),
            Some(&format!("controller={}", pending.requester)),
            "allowed",
        );
        ControlDecision::Granted {
            session_id: pending.requester,
        }
    }

    /// 会话结束时释放控制权。
    ///
    /// # Arguments
    /// * `session_id` - 结束的会话
    ///
    /// # Returns
    /// 该会话原本是控制者时返回 `true`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn release(&mut self, session_id: u64) -> bool {
        if self.controller == Some(session_id) {
            self.controller = None;
            true
        } else {
            false
        }
    }

    /// 控制者让位后的下一阶段。
    fn advance_after_yield(
        &mut self,
        pending: Pending,
        now_ms: u64,
        audit: &mut AuditLog,
    ) -> ControlDecision {
        let old = pending
            .previous_controller
            .map(|id| format!("controller={id}"))
            .unwrap_or_else(|| "controller=none".to_owned());
        self.controller = None;
        audit.record(
            now_ms,
            "takeover",
            &pending.requester.to_string(),
            "controller_yield",
            Some(&old),
            Some("controller=none"),
            "yielded",
        );
        if self.policy == TakeoverPolicy::Auto {
            self.pending = None;
            self.controller = Some(pending.requester);
            audit.record(
                now_ms,
                "takeover",
                &pending.requester.to_string(),
                "auto_approve",
                Some("controller=none"),
                Some(&format!("controller={}", pending.requester)),
                "allowed",
            );
            return ControlDecision::Granted {
                session_id: pending.requester,
            };
        }
        let deadline = now_ms.saturating_add(LOCAL_CONFIRM_TIMEOUT_MS);
        self.pending = Some(Pending {
            requester: pending.requester,
            phase: PendingPhase::Owner,
            deadline_ms: deadline,
            previous_controller: pending.previous_controller,
        });
        ControlDecision::PendingOwnerConfirm {
            requester: pending.requester,
            deadline_ms: deadline,
        }
    }
}

// =====** 踢出 =====**

/// 踢出票据（owner 私钥签名，被控端校验）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KickTicket {
    /// 会话 ID。
    pub session_id: u64,
    /// 签发者（owner）公钥。
    pub issuer_pub: String,
    /// 目标设备公钥。
    pub target_pub: String,
    /// 下发时间（Unix 毫秒）。
    pub issued_at_ms: u64,
    /// 一次性随机串。
    pub nonce: String,
}

impl KickTicket {
    /// 规范化签名字节。
    ///
    /// # Returns
    /// UTF-8 字节。
    ///
    /// # Errors
    /// 不会失败。
    pub fn canonical_bytes(&self) -> Vec<u8> {
        format!(
            "RDC-kick-v1|{}|{}|{}|{}|{}",
            self.session_id, self.issuer_pub, self.target_pub, self.issued_at_ms, self.nonce
        )
        .into_bytes()
    }
}

/// 已签名的踢出票据。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SignedKick {
    /// 票据。
    pub ticket: KickTicket,
    /// 签名（Base64）。
    pub signature: String,
}

/// 踢出校验错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum KickError {
    /// 签发者未受信任。
    #[error("踢出签发者未被本机信任")]
    OwnerUntrusted,
    /// 签名无效。
    #[error("踢出签名无效")]
    SignatureInvalid,
    /// 超过 3 秒生效窗口。
    #[error("踢出票据已超出 3 秒生效窗口")]
    Expired,
    /// 票据时间来自未来。
    #[error("踢出票据时间非法")]
    FromFuture,
}

impl KickError {
    /// 固定错误码。
    ///
    /// # Returns
    /// 稳定标识。
    ///
    /// # Errors
    /// 不会失败。
    pub fn code(&self) -> &'static str {
        match self {
            Self::OwnerUntrusted => "owner_untrusted",
            Self::SignatureInvalid => "signature_invalid",
            Self::Expired => "kick_expired",
            Self::FromFuture => "kick_from_future",
        }
    }
}

/// 签发踢出票据。
///
/// # Arguments
/// * `identity` - owner 身份密钥
/// * `session_id` - 目标会话
/// * `target_pub` - 目标设备公钥
/// * `now_ms` - 当前 Unix 毫秒
///
/// # Returns
/// 票据与签名。
///
/// # Errors
/// 不会失败。
pub fn issue_kick(
    identity: &IdentityKeyPair,
    session_id: u64,
    target_pub: &str,
    now_ms: u64,
) -> SignedKick {
    let ticket = KickTicket {
        session_id,
        issuer_pub: identity.public_key_b64(),
        target_pub: target_pub.to_owned(),
        issued_at_ms: now_ms,
        nonce: unique_id("k", now_ms),
    };
    let signature = identity.sign_b64(&ticket.canonical_bytes());
    SignedKick { ticket, signature }
}

/// 校验踢出票据（必须由地址簿中的 owner 签发，且在 3 秒窗口内）。
///
/// # Arguments
/// * `signed` - 已签名票据
/// * `trusted_owner_pubs` - 地址簿 owner 公钥
/// * `now_ms` - 当前 Unix 毫秒
///
/// # Returns
/// 通过返回 `Ok(())`。
///
/// # Errors
/// 未信任、签名无效、超窗或时间来自未来时返回 [`KickError`]。
pub fn verify_kick(
    signed: &SignedKick,
    trusted_owner_pubs: &[String],
    now_ms: u64,
) -> Result<(), KickError> {
    if !trusted_owner_pubs
        .iter()
        .any(|key| key == &signed.ticket.issuer_pub)
    {
        return Err(KickError::OwnerUntrusted);
    }
    verify_signature_b64(
        &signed.ticket.issuer_pub,
        &signed.ticket.canonical_bytes(),
        &signed.signature,
    )
    .map_err(|_| KickError::SignatureInvalid)?;
    // 允许 1 秒时钟抖动，但超出 3 秒生效窗口的票据一律拒绝。
    if now_ms.saturating_add(1_000) < signed.ticket.issued_at_ms {
        return Err(KickError::FromFuture);
    }
    if now_ms > signed.ticket.issued_at_ms.saturating_add(KICK_EFFECT_MAX_MS) {
        return Err(KickError::Expired);
    }
    Ok(())
}

// =====** 授权存储 =====**

/// 一条授权记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantRecord {
    /// 令牌。
    pub token: GrantToken,
    /// owner 签名。
    pub signature: String,
    /// 被授权方显示名（本地别名）。
    pub grantee_name: String,
    /// 创建时间。
    pub created_ms: u64,
    /// 最后修改时间。
    pub updated_ms: u64,
    /// 是否已吊销（最终一致，见 [`REVOCATION_PROPAGATION_MAX_MS`]）。
    pub revoked: bool,
    /// 本机减免后的有效权限位。
    ///
    /// 为什么单独存：令牌由 owner 签名，本机削减权限**不能改写令牌字段**
    /// （否则签名失效）；削减只影响本地生效值。`0` 视为「与令牌一致」（兼容旧文件）。
    #[serde(default)]
    pub effective_perms: u16,
}

impl GrantRecord {
    /// 本机生效权限（令牌权限与本地削减的交集）。
    ///
    /// # Returns
    /// `effective_perms` 为 0 时回落到令牌权限。
    ///
    /// # Errors
    /// 不会失败。
    pub fn local_perms(&self) -> PermBits {
        if self.effective_perms == 0 {
            self.token.perms()
        } else {
            PermBits::new(self.effective_perms)
        }
    }
}

/// 授权存储错误。
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum GrantStoreError {
    /// 文件读写失败。
    #[error("授权存储 IO 失败：{0}")]
    Io(String),
    /// JSON 解析失败。
    #[error("授权存储格式错误：{0}")]
    Format(String),
    /// 文件版本超前。
    #[error("授权存储版本不支持：文件 {found}，本端上限 {supported}")]
    UnsupportedVersion {
        /// 文件版本。
        found: u32,
        /// 本端支持版本。
        supported: u32,
    },
    /// 授权 ID 重复。
    #[error("授权已存在：{0}")]
    Duplicate(String),
    /// 授权不存在。
    #[error("授权不存在：{0}")]
    NotFound(String),
    /// 权限追加被拒绝（只能削减）。
    #[error("授权权限只能削减不能追加：{0} → {1}")]
    PermEscalation(u16, u16),
}

/// 授权存储的磁盘表示。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct GrantFile {
    /// 文件版本。
    version: u32,
    /// 撤销世代。
    revoked_epoch: u64,
    /// 授权记录。
    grants: Vec<GrantRecord>,
}

/// 授权存储（内存视图 + 原子持久化）。
#[derive(Debug, Clone, Default)]
pub struct GrantStore {
    /// 授权，按 grant_id 排序。
    grants: BTreeMap<String, GrantRecord>,
    /// 撤销世代（只增不减）。
    revoked_epoch: u64,
}

impl GrantStore {
    /// 空存储。
    ///
    /// # Returns
    /// 无授权、世代 0。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new() -> Self {
        Self::default()
    }

    /// 加载（文件缺失视为空）。
    ///
    /// # Arguments
    /// * `path` - 授权文件路径
    ///
    /// # Returns
    /// 解析后的存储。
    ///
    /// # Errors
    /// IO、JSON 或版本错误返回 [`GrantStoreError`]。
    pub fn load(path: impl AsRef<Path>) -> Result<Self, GrantStoreError> {
        let bytes = match std::fs::read(path.as_ref()) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Self::new()),
            Err(err) => return Err(GrantStoreError::Io(err.to_string())),
        };
        let file: GrantFile =
            serde_json::from_slice(&bytes).map_err(|err| GrantStoreError::Format(err.to_string()))?;
        if file.version > GRANT_FILE_VERSION {
            return Err(GrantStoreError::UnsupportedVersion {
                found: file.version,
                supported: GRANT_FILE_VERSION,
            });
        }
        let mut store = Self {
            grants: BTreeMap::new(),
            revoked_epoch: file.revoked_epoch,
        };
        for record in file.grants {
            let mut record = record;
            if record.effective_perms == 0 {
                record.effective_perms = record.token.perms;
            }
            store
                .grants
                .insert(record.token.grant_id.clone(), record);
        }
        Ok(store)
    }

    /// 原子保存。
    ///
    /// # Arguments
    /// * `path` - 目标路径；父目录不存在时自动创建
    ///
    /// # Returns
    /// 写入成功返回 `Ok(())`。
    ///
    /// # Errors
    /// 目录/写入/rename 失败返回 [`GrantStoreError`]。
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), GrantStoreError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|err| GrantStoreError::Io(err.to_string()))?;
            }
        }
        let file = GrantFile {
            version: GRANT_FILE_VERSION,
            revoked_epoch: self.revoked_epoch,
            grants: self.grants.values().cloned().collect(),
        };
        let bytes = serde_json::to_vec_pretty(&file)
            .map_err(|err| GrantStoreError::Format(err.to_string()))?;
        let mut name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "grants.json".to_owned());
        name.push_str(TEMP_SUFFIX);
        let temp = path.with_file_name(name);
        {
            let mut handle =
                std::fs::File::create(&temp).map_err(|err| GrantStoreError::Io(err.to_string()))?;
            handle
                .write_all(&bytes)
                .map_err(|err| GrantStoreError::Io(err.to_string()))?;
            handle
                .sync_all()
                .map_err(|err| GrantStoreError::Io(err.to_string()))?;
        }
        std::fs::rename(&temp, path).map_err(|err| GrantStoreError::Io(err.to_string()))?;
        Ok(())
    }

    /// 当前撤销世代。
    ///
    /// # Returns
    /// 单调递增的世代号。
    ///
    /// # Errors
    /// 不会失败。
    pub fn revoked_epoch(&self) -> u64 {
        self.revoked_epoch
    }

    /// 授权数量。
    ///
    /// # Returns
    /// 含已吊销的条数。
    ///
    /// # Errors
    /// 不会失败。
    pub fn len(&self) -> usize {
        self.grants.len()
    }

    /// 是否为空。
    ///
    /// # Returns
    /// 无授权时返回 `true`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn is_empty(&self) -> bool {
        self.grants.is_empty()
    }

    /// 按 ID 查找。
    ///
    /// # Arguments
    /// * `grant_id` - 授权 ID
    ///
    /// # Returns
    /// 命中返回记录。
    ///
    /// # Errors
    /// 不会失败。
    pub fn get(&self, grant_id: &str) -> Option<&GrantRecord> {
        self.grants.get(grant_id)
    }

    /// 全部授权，按 ID 升序。
    ///
    /// # Returns
    /// 只读切片视图。
    ///
    /// # Errors
    /// 不会失败。
    pub fn grants(&self) -> Vec<&GrantRecord> {
        self.grants.values().collect()
    }

    /// 新增授权（落审计）。
    ///
    /// # Arguments
    /// * `record` - 授权记录
    /// * `now_ms` - 当前 Unix 毫秒
    /// * `audit` - 审计日志
    ///
    /// # Returns
    /// 成功返回 `Ok(())`。
    ///
    /// # Errors
    /// ID 重复返回 [`GrantStoreError::Duplicate`]。
    pub fn add(
        &mut self,
        mut record: GrantRecord,
        now_ms: u64,
        audit: &mut AuditLog,
    ) -> Result<(), GrantStoreError> {
        let grant_id = record.token.grant_id.clone();
        if self.grants.contains_key(&grant_id) {
            return Err(GrantStoreError::Duplicate(grant_id));
        }
        if record.effective_perms == 0 {
            record.effective_perms = record.token.perms;
        }
        audit.record(
            now_ms,
            "grant_created",
            &grant_id,
            "issue",
            None,
            Some(&format!(
                "grantee={} perms=0x{:X} expires_at={}",
                record.token.grantee_pub, record.token.perms, record.token.expires_at
            )),
            "ok",
        );
        self.grants.insert(grant_id, record);
        Ok(())
    }

    /// 修改授权权限（只能削减，落审计）。
    ///
    /// # Arguments
    /// * `grant_id` - 授权 ID
    /// * `new_perms` - 新权限位
    /// * `now_ms` - 当前 Unix 毫秒
    /// * `audit` - 审计日志
    ///
    /// # Returns
    /// 成功返回 `Ok(())`。
    ///
    /// # Errors
    /// 不存在、已吊销或尝试追加权限时返回 [`GrantStoreError`]。
    pub fn update_perms(
        &mut self,
        grant_id: &str,
        new_perms: PermBits,
        now_ms: u64,
        audit: &mut AuditLog,
    ) -> Result<(), GrantStoreError> {
        let Some(record) = self.grants.get_mut(grant_id) else {
            return Err(GrantStoreError::NotFound(grant_id.to_owned()));
        };
        if record.revoked {
            return Err(GrantStoreError::NotFound(grant_id.to_owned()));
        }
        let old = record.local_perms();
        if !new_perms.is_subset_of(old) {
            audit.record(
                now_ms,
                "grant_updated",
                grant_id,
                "reduce_perms",
                Some(&format!("perms=0x{:X}", old.to_bits())),
                Some(&format!("perms=0x{:X}", new_perms.to_bits())),
                "denied",
            );
            return Err(GrantStoreError::PermEscalation(
                old.to_bits(),
                new_perms.to_bits(),
            ));
        }
        record.effective_perms = new_perms.to_bits();
        record.updated_ms = now_ms;
        audit.record(
            now_ms,
            "grant_updated",
            grant_id,
            "reduce_perms",
            Some(&format!("perms=0x{:X}", old.to_bits())),
            Some(&format!("perms=0x{:X}", new_perms.to_bits())),
            "ok",
        );
        Ok(())
    }

    /// 吊销授权（落审计并推进撤销世代）。
    ///
    /// # Arguments
    /// * `grant_id` - 授权 ID
    /// * `now_ms` - 当前 Unix 毫秒
    /// * `audit` - 审计日志
    ///
    /// # Returns
    /// 成功返回 `Ok(())`。
    ///
    /// # Errors
    /// 不存在返回 [`GrantStoreError::NotFound`]。
    pub fn revoke(
        &mut self,
        grant_id: &str,
        now_ms: u64,
        audit: &mut AuditLog,
    ) -> Result<(), GrantStoreError> {
        let Some(record) = self.grants.get_mut(grant_id) else {
            return Err(GrantStoreError::NotFound(grant_id.to_owned()));
        };
        if record.revoked {
            return Ok(());
        }
        record.revoked = true;
        record.updated_ms = now_ms;
        self.revoked_epoch = self.revoked_epoch.saturating_add(1);
        audit.record(
            now_ms,
            "grant_revoked",
            grant_id,
            "revoke",
            Some("active"),
            Some("revoked"),
            "ok",
        );
        Ok(())
    }

    /// 删除授权（仅本机清理，不产生撤销广播）。
    ///
    /// # Arguments
    /// * `grant_id` - 授权 ID
    ///
    /// # Returns
    /// 原先存在返回 `true`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn remove(&mut self, grant_id: &str) -> bool {
        self.grants.remove(grant_id).is_some()
    }

    /// 未吊销的授权数量。
    ///
    /// # Returns
    /// 有效授权条数。
    ///
    /// # Errors
    /// 不会失败。
    pub fn active_count(&self) -> usize {
        self.grants.values().filter(|record| !record.revoked).count()
    }
}

/// 授权记录里所有被授权方公钥（供地址簿交叉核对）。
///
/// # Returns
/// 去重后的公钥集合。
///
/// # Errors
/// 不会失败。
pub fn grantee_keys(grants: &GrantStore) -> BTreeSet<String> {
    grants
        .grants
        .values()
        .filter(|record| !record.revoked)
        .map(|record| record.token.grantee_pub.clone())
        .collect()
}

/// 由令牌与授权上限推导会话权限并判定输入。
///
/// # Arguments
/// * `role` - 会话角色
/// * `requested` - 会话请求位
/// * `token_perms` - 令牌权限
/// * `local_allowed` - 本机允许
///
/// # Returns
/// `(有效权限, 输入判定)`。
///
/// # Errors
/// 不会失败。
pub fn evaluate_session(
    role: SessionRole,
    requested: PermBits,
    token_perms: PermBits,
    local_allowed: PermBits,
) -> (PermBits, AuthzDecision) {
    let effective = session_perms(requested, token_perms, local_allowed);
    (effective, authorize_input_with_grant(role, effective))
}

/// 当前 Unix 毫秒（与 devices 模块一致）。
///
/// # Returns
/// 系统时间早于 1970 时返回 0。
///
/// # Errors
/// 不会失败。
pub fn now_ms() -> u64 {
    crate::s5::devices::now_ms()
}

// =====** 单元测试（G1–G7） =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用固定时钟原点：2023-11-14。
    const T0: u64 = 1_700_000_000_000;

    /// 构造 owner / host / grantee 三个身份。
    fn ids() -> (IdentityKeyPair, IdentityKeyPair, IdentityKeyPair) {
        (
            IdentityKeyPair::from_seed(&[1u8; 32]),
            IdentityKeyPair::from_seed(&[2u8; 32]),
            IdentityKeyPair::from_seed(&[3u8; 32]),
        )
    }

    /// 构造指定权限与有效期的令牌。
    fn signed(perms: PermBits, validity_ms: Option<u64>) -> SignedGrant {
        let (owner, host, grantee) = ids();
        issue_grant(
            &owner,
            &GrantSpec {
                host_pub: &host.public_key_b64(),
                grantee_pub: &grantee.public_key_b64(),
                perms,
                validity_ms,
                revoked_epoch_base: 0,
            },
            T0,
        )
        .expect("签发")
    }

    /// 默认上下文：信任 owner，允许全部安全位。
    fn context<'a>(trusted: &'a [String], host_pub: &'a str) -> VerifyContext<'a> {
        VerifyContext {
            local_host_pub: host_pub,
            peer_pub: None,
            trusted_owner_pubs: trusted,
            owner_ceiling: PermBits(PermBits::ALL),
            local_revoked_epoch: 0,
            allow_sensitive_bits: false,
        }
    }

    /// G1：默认 30 天；超过 90 天拒绝；签名可验。
    #[test]
    fn g1_issue_defaults_and_validity_ceiling() {
        let (owner, host, grantee) = ids();
        let issued = issue_grant(
            &owner,
            &GrantSpec {
                host_pub: &host.public_key_b64(),
                grantee_pub: &grantee.public_key_b64(),
                perms: GrantTemplate::ReadOnly.bits(),
                validity_ms: None,
                revoked_epoch_base: 0,
            },
            T0,
        )
        .expect("默认有效期应可签发");
        assert_eq!(
            issued.token.expires_at - issued.token.not_before,
            DEFAULT_VALIDITY_MS
        );
        assert_eq!(issued.token.owner_pub, owner.public_key_b64());
        owner
            .verify_b64(&issued.token.canonical_bytes(), &issued.signature)
            .expect("签名自检");

        let too_long = issue_grant(
            &owner,
            &GrantSpec {
                host_pub: &host.public_key_b64(),
                grantee_pub: &grantee.public_key_b64(),
                perms: GrantTemplate::ReadOnly.bits(),
                validity_ms: Some(MAX_VALIDITY_MS + DAY_MS),
                revoked_epoch_base: 0,
            },
            T0,
        );
        assert_eq!(too_long.unwrap_err().code(), "validity_too_long");

        // 恰好 90 天可以签发。
        let exact = issue_grant(
            &owner,
            &GrantSpec {
                host_pub: &host.public_key_b64(),
                grantee_pub: &grantee.public_key_b64(),
                perms: GrantTemplate::ReadOnly.bits(),
                validity_ms: Some(MAX_VALIDITY_MS),
                revoked_epoch_base: 0,
            },
            T0,
        )
        .expect("90 天可签发");
        assert_eq!(exact.token.expires_at, T0 + MAX_VALIDITY_MS);
    }

    /// G2：五步校验的九个反例都返回固定错误码。
    #[test]
    fn g2_verify_rejects_each_step() {
        let (owner, host, _grantee) = ids();
        let trusted = vec![owner.public_key_b64()];
        let valid = signed(GrantTemplate::Operate.bits(), None);
        let host_pub = host.public_key_b64();

        // 正例：通过且有效权限等于令牌权限。
        let mut replay = ReplayGuard::new();
        let ok = verify_grant(
            &valid.token,
            &valid.signature,
            &context(&trusted, &host_pub),
            T0 + 1_000,
            &mut replay,
        )
        .expect("正例应通过");
        assert_eq!(ok.effective_perms, GrantTemplate::Operate.bits());

        // 1) host 不匹配。
        let wrong_host = IdentityKeyPair::from_seed(&[9u8; 32]).public_key_b64();
        let err = verify_grant(
            &valid.token,
            &valid.signature,
            &context(&trusted, &wrong_host),
            T0 + 1_000,
            &mut ReplayGuard::new(),
        )
        .unwrap_err();
        assert_eq!(err.code(), "host_mismatch");

        // 2a) owner 未信任。
        let err = verify_grant(
            &valid.token,
            &valid.signature,
            &context(&[], &host_pub),
            T0 + 1_000,
            &mut ReplayGuard::new(),
        )
        .unwrap_err();
        assert_eq!(err.code(), "owner_untrusted");

        // 2a2) 令牌不是签给本次连接的对端。
        let other_peer = IdentityKeyPair::from_seed(&[8u8; 32]).public_key_b64();
        let mut peer_ctx = context(&trusted, &host_pub);
        peer_ctx.peer_pub = Some(other_peer.as_str());
        let err = verify_grant(
            &valid.token,
            &valid.signature,
            &peer_ctx,
            T0 + 1_000,
            &mut ReplayGuard::new(),
        )
        .unwrap_err();
        assert_eq!(err.code(), "grantee_mismatch");

        // 2b) 篡改签名。
        let err = verify_grant(
            &valid.token,
            "AAAA",
            &context(&trusted, &host_pub),
            T0 + 1_000,
            &mut ReplayGuard::new(),
        )
        .unwrap_err();
        assert_eq!(err.code(), "signature_invalid");

        // 2c) 签发后篡改令牌字段（签名与内容不再匹配）。
        let mut tampered = valid.token.clone();
        tampered.perms |= PermBits::FILE_TRANSFER;
        let err = verify_grant(
            &tampered,
            &valid.signature,
            &context(&trusted, &host_pub),
            T0 + 1_000,
            &mut ReplayGuard::new(),
        )
        .unwrap_err();
        assert_eq!(err.code(), "signature_invalid");

        // 3a) 尚未生效。
        let err = verify_grant(
            &valid.token,
            &valid.signature,
            &context(&trusted, &host_pub),
            T0 - 1,
            &mut ReplayGuard::new(),
        )
        .unwrap_err();
        assert_eq!(err.code(), "not_yet_valid");

        // 3b) 已过期。
        let err = verify_grant(
            &valid.token,
            &valid.signature,
            &context(&trusted, &host_pub),
            valid.token.expires_at,
            &mut ReplayGuard::new(),
        )
        .unwrap_err();
        assert_eq!(err.code(), "expired");

        // 3c) 有效期超 90 天（手工构造 + 重新签名，模拟签发端缺陷）。
        let mut long = valid.token.clone();
        long.expires_at = long.not_before + MAX_VALIDITY_MS + DAY_MS;
        let long_sig = owner.sign_b64(&long.canonical_bytes());
        let err = verify_grant(
            &long,
            &long_sig,
            &context(&trusted, &host_pub),
            T0 + 1_000,
            &mut ReplayGuard::new(),
        )
        .unwrap_err();
        assert_eq!(err.code(), "validity_too_long");

        // 4a) 权限超过本机上限。
        let mut limited = context(&trusted, &host_pub);
        limited.owner_ceiling = PermBits(PermBits::VIEW);
        let err = verify_grant(
            &valid.token,
            &valid.signature,
            &limited,
            T0 + 1_000,
            &mut ReplayGuard::new(),
        )
        .unwrap_err();
        assert_eq!(err.code(), "perm_exceeded");

        // 4b) 敏感位未获本机二次确认。
        let sensitive = signed(PermBits(PermBits::VIEW | PermBits::POWER), None);
        let err = verify_grant(
            &sensitive.token,
            &sensitive.signature,
            &context(&trusted, &host_pub),
            T0 + 1_000,
            &mut ReplayGuard::new(),
        )
        .unwrap_err();
        assert_eq!(err.code(), "sensitive_needs_confirm");

        // 5a) 撤销世代回退。
        let mut regressed = context(&trusted, &host_pub);
        regressed.local_revoked_epoch = 1;
        let err = verify_grant(
            &valid.token,
            &valid.signature,
            &regressed,
            T0 + 1_000,
            &mut ReplayGuard::new(),
        )
        .unwrap_err();
        assert_eq!(err.code(), "epoch_regression");

        // 5b) nonce 重放。
        let err = verify_grant(
            &valid.token,
            &valid.signature,
            &context(&trusted, &host_pub),
            T0 + 1_000,
            &mut replay,
        )
        .unwrap_err();
        assert_eq!(err.code(), "nonce_reused");
    }

    /// G3：三档模板映射；任何模板都不含 `POWER` / `DISPLAY_SWITCH`。
    #[test]
    fn g3_templates_never_include_sensitive_bits() {
        assert_eq!(GrantTemplate::ReadOnly.bits(), PermBits(PermBits::VIEW));
        assert_eq!(
            GrantTemplate::Operate.bits(),
            PermBits(PermBits::VIEW | PermBits::CONTROL | PermBits::CLIPBOARD_TEXT)
        );
        let full = GrantTemplate::FullControl.bits();
        assert!(full.contains(
            PermBits::VIEW
                | PermBits::CONTROL
                | PermBits::CLIPBOARD_TEXT
                | PermBits::FILE_TRANSFER
                | PermBits::AUDIO
                | PermBits::RECONNECT
        ));
        for template in [
            GrantTemplate::ReadOnly,
            GrantTemplate::Operate,
            GrantTemplate::FullControl,
        ] {
            assert!(
                !template.bits().contains(PermBits::SENSITIVE),
                "模板 {} 不得包含敏感位",
                template.as_str()
            );
        }
        // 页面旧名兼容。
        assert_eq!(template_bits("standard"), Some(GrantTemplate::Operate.bits()));
        assert_eq!(template_bits("full"), Some(GrantTemplate::FullControl.bits()));
        assert_eq!(template_bits("readonly"), Some(GrantTemplate::ReadOnly.bits()));
        assert_eq!(template_bits("custom"), None);
        // 位序与 docs/07 表格一致。
        assert_eq!(PermBits::VIEW, 1 << 0);
        assert_eq!(PermBits::CONTROL, 1 << 1);
        assert_eq!(PermBits::CLIPBOARD_TEXT, 1 << 2);
        assert_eq!(PermBits::CLIPBOARD_IMAGE, 1 << 3);
        assert_eq!(PermBits::FILE_TRANSFER, 1 << 4);
        assert_eq!(PermBits::AUDIO, 1 << 5);
        assert_eq!(PermBits::POWER, 1 << 6);
        assert_eq!(PermBits::DISPLAY_SWITCH, 1 << 7);
        assert_eq!(PermBits::RECONNECT, 1 << 8);
    }

    /// G4：独占待批准、控制者 15 秒超时让位、本机 30 秒确认超时拒绝。
    #[test]
    fn g4_exclusive_and_takeover_state_machine() {
        let mut audit = AuditLog::new(0);
        let mut arbiter = ControlArbiter::new(TakeoverPolicy::Confirm);
        assert_eq!(
            arbiter.request_control(1, T0, &mut audit),
            ControlDecision::Granted { session_id: 1 }
        );
        // 已有控制者：新请求进入待批准。
        let pending = arbiter.request_control(2, T0 + 100, &mut audit);
        assert!(matches!(
            pending,
            ControlDecision::PendingControllerResponse { requester: 2, .. }
        ));
        assert_eq!(arbiter.controller(), Some(1));

        // 控制者 15 秒未响应：tick 到截止时间视为让位，进入本机确认。
        let after_yield = arbiter.tick(T0 + 100 + CONTROLLER_RESPONSE_TIMEOUT_MS, &mut audit);
        assert!(matches!(
            after_yield,
            Some(ControlDecision::PendingOwnerConfirm { requester: 2, .. })
        ));
        assert_eq!(arbiter.controller(), None);

        // 本机确认：角色变更，审计含旧值与新值。
        assert_eq!(
            arbiter.owner_confirm(true, T0 + 100 + 15_100, &mut audit),
            ControlDecision::Granted { session_id: 2 }
        );
        assert_eq!(arbiter.controller(), Some(2));
        let takeover_entries: Vec<_> = audit
            .entries()
            .iter()
            .filter(|entry| entry.kind == "takeover")
            .collect();
        assert!(takeover_entries.iter().any(|entry| entry.action == "owner_approve"
            && entry.old_value.as_deref() == Some("controller=1")
            && entry.new_value.as_deref() == Some("controller=2")));

        // 30 秒未确认：拒绝。
        let mut audit2 = AuditLog::new(0);
        let mut arbiter2 = ControlArbiter::new(TakeoverPolicy::Confirm);
        arbiter2.request_control(10, T0, &mut audit2);
        arbiter2.request_control(11, T0 + 1, &mut audit2);
        arbiter2.tick(T0 + 1 + CONTROLLER_RESPONSE_TIMEOUT_MS, &mut audit2);
        let timeout = arbiter2.tick(
            T0 + 1 + CONTROLLER_RESPONSE_TIMEOUT_MS + LOCAL_CONFIRM_TIMEOUT_MS,
            &mut audit2,
        );
        assert_eq!(
            timeout,
            Some(ControlDecision::Denied {
                code: "local_confirm_timeout"
            })
        );
        assert_eq!(arbiter2.controller(), None);

        // 禁止接管。
        let mut audit3 = AuditLog::new(0);
        let mut arbiter3 = ControlArbiter::new(TakeoverPolicy::Forbid);
        arbiter3.request_control(20, T0, &mut audit3);
        assert_eq!(
            arbiter3.request_control(21, T0 + 1, &mut audit3),
            ControlDecision::Denied {
                code: "takeover_forbidden"
            }
        );

        // 自动同意：控制者让位后直接转交。
        let mut audit4 = AuditLog::new(0);
        let mut arbiter4 = ControlArbiter::new(TakeoverPolicy::Auto);
        arbiter4.request_control(30, T0, &mut audit4);
        arbiter4.request_control(31, T0 + 1, &mut audit4);
        let auto = arbiter4
            .tick(T0 + 1 + CONTROLLER_RESPONSE_TIMEOUT_MS, &mut audit4)
            .expect("自动接管应完成");
        assert_eq!(auto, ControlDecision::Granted { session_id: 31 });
    }

    /// G5：踢出签名与 3 秒窗口；吊销文案不含「立即生效」。
    #[test]
    fn g5_kick_window_and_revocation_text() {
        let (owner, _host, target) = ids();
        let signed = issue_kick(&owner, 7, &target.public_key_b64(), T0);
        let trusted = vec![owner.public_key_b64()];
        verify_kick(&signed, &trusted, T0).expect("3 秒内应有效");
        verify_kick(&signed, &trusted, T0 + KICK_EFFECT_MAX_MS)
            .expect("恰好 3 秒边界仍有效");
        let too_late = verify_kick(&signed, &trusted, T0 + KICK_EFFECT_MAX_MS + 1).unwrap_err();
        assert_eq!(too_late.code(), "kick_expired");
        let untrusted = verify_kick(&signed, &[], T0).unwrap_err();
        assert_eq!(untrusted.code(), "owner_untrusted");

        const _: () = assert!(REVOCATION_PROPAGATION_MAX_MS >= 15 * 60_000);
        assert!(!REVOCATION_UI_TEXT.contains("立即生效"));
        assert!(REVOCATION_UI_TEXT.contains("15 分 90 秒"));
    }

    /// G6：观看者注入被拒（即使令牌含控制位），控制者需令牌含 `CONTROL`。
    #[test]
    fn g6_viewer_hard_gate() {
        let full = GrantTemplate::FullControl.bits();
        let viewer = authorize_input_with_grant(SessionRole::Viewer, full);
        assert!(!viewer.allowed);
        assert_eq!(viewer.reason, "viewer_readonly");

        let (effective, decision) = evaluate_session(
            SessionRole::Controller,
            PermBits(PermBits::VIEW | PermBits::CONTROL),
            GrantTemplate::Operate.bits(),
            PermBits(PermBits::ALL),
        );
        assert_eq!(
            effective,
            PermBits(PermBits::VIEW | PermBits::CONTROL),
            "会话权限取三者交集，本机模板多出的位不会追加给请求方"
        );
        assert!(decision.allowed);

        let (_effective, denied) = evaluate_session(
            SessionRole::Controller,
            PermBits(PermBits::VIEW),
            GrantTemplate::ReadOnly.bits(),
            PermBits(PermBits::ALL),
        );
        assert!(!denied.allowed);
        assert_eq!(denied.reason, "control_not_granted");

        // 观看者即使有控制位，会话权限也不得用于注入：硬闸不依赖权限位。
        assert!(!authorize_input_with_grant(SessionRole::Viewer, PermBits(PermBits::ALL)).allowed);
    }

    /// G7：授权存储的创建/削减/吊销都落审计，且字段齐全、无媒体内容。
    #[test]
    fn g7_grant_store_audits_and_only_reduces() {
        let (owner, host, grantee) = ids();
        let issued = issue_grant(
            &owner,
            &GrantSpec {
                host_pub: &host.public_key_b64(),
                grantee_pub: &grantee.public_key_b64(),
                perms: GrantTemplate::FullControl.bits(),
                validity_ms: None,
                revoked_epoch_base: 0,
            },
            T0,
        )
        .expect("签发");
        let grant_id = issued.token.grant_id.clone();
        let record = GrantRecord {
            token: issued.token,
            signature: issued.signature,
            grantee_name: "同事 B".to_owned(),
            created_ms: T0,
            updated_ms: T0,
            revoked: false,
            effective_perms: GrantTemplate::FullControl.bits().to_bits(),
        };
        let mut store = GrantStore::new();
        let mut audit = AuditLog::new(0);
        store.add(record, T0, &mut audit).expect("新增");
        assert_eq!(store.len(), 1);
        assert_eq!(store.active_count(), 1);

        // 追加权限被拒绝，并落 denied 审计。
        let escalate = store.update_perms(
            &grant_id,
            PermBits(GrantTemplate::FullControl.bits().to_bits() | PermBits::POWER),
            T0 + 1,
            &mut audit,
        );
        assert!(matches!(
            escalate,
            Err(GrantStoreError::PermEscalation(_, _))
        ));
        assert!(audit
            .entries()
            .iter()
            .any(|entry| entry.kind == "grant_updated" && entry.outcome == "denied"));

        // 削减成功，旧值/新值可核对。
        store
            .update_perms(&grant_id, GrantTemplate::ReadOnly.bits(), T0 + 2, &mut audit)
            .expect("削减应成功");
        assert_eq!(
            store.get(&grant_id).expect("存在").local_perms(),
            PermBits(PermBits::VIEW)
        );

        // 吊销推进世代，落审计。
        store.revoke(&grant_id, T0 + 3, &mut audit).expect("吊销");
        assert_eq!(store.revoked_epoch(), 1);
        assert_eq!(store.active_count(), 0);
        let revoked = audit
            .entries()
            .iter()
            .find(|entry| entry.kind == "grant_revoked")
            .expect("吊销审计");
        assert_eq!(revoked.old_value.as_deref(), Some("active"));
        assert_eq!(revoked.new_value.as_deref(), Some("revoked"));

        // 吊销后令牌在校验层被世代回退拒绝（最终一致的本地侧判定）。
        let trusted = vec![owner.public_key_b64()];
        let host_pub = host.public_key_b64();
        let mut ctx = context(&trusted, &host_pub);
        ctx.local_revoked_epoch = store.revoked_epoch();
        let err = verify_grant(
            &store.get(&grant_id).expect("存在").token,
            &store.get(&grant_id).expect("存在").signature,
            &ctx,
            T0 + 4,
            &mut ReplayGuard::new(),
        )
        .unwrap_err();
        assert_eq!(err.code(), "epoch_regression");

        // 审计红线：不得出现媒体内容字段。
        let jsonl = audit.to_jsonl();
        for forbidden in ["screenshot", "frame", "clipboard_text", "file_bytes"] {
            assert!(!jsonl.contains(forbidden));
        }
    }

    /// 存储往返：文件缺失视为空，保存后不残留临时文件。
    #[test]
    fn store_roundtrip_and_atomic_write() {
        let dir = std::env::temp_dir().join(format!(
            "rdc-s5-grant-{}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(&dir).expect("创建目录");
        let path = dir.join("grants.json");
        let (owner, host, grantee) = ids();
        let issued = issue_grant(
            &owner,
            &GrantSpec {
                host_pub: &host.public_key_b64(),
                grantee_pub: &grantee.public_key_b64(),
                perms: GrantTemplate::ReadOnly.bits(),
                validity_ms: None,
                revoked_epoch_base: 0,
            },
            T0,
        )
        .expect("签发");
        let mut store = GrantStore::new();
        let mut audit = AuditLog::new(0);
        store
            .add(
                GrantRecord {
                    token: issued.token,
                    signature: issued.signature,
                    grantee_name: "B".to_owned(),
                    created_ms: T0,
                    updated_ms: T0,
                    revoked: false,
                    effective_perms: GrantTemplate::ReadOnly.bits().to_bits(),
                },
                T0,
                &mut audit,
            )
            .expect("新增");
        store.save(&path).expect("保存");
        let loaded = GrantStore::load(&path).expect("加载");
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded.active_count(), 1);
        assert!(!dir.join("grants.json.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir); // gate-allow R-C-2: 用例收尾删除独立临时目录，失败不影响断言结论
    }
}
