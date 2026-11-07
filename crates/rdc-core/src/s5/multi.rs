//! rdc-core / S5 多会话与角色
//!
//! 层级：L3 编排层
//! 用途：把 docs/04 §2.4 的多会话（bit10）与屏幕墙（bit11）能力位用于会话注册，
//!       并给出「控制者可注入输入、观看者只读」的唯一判定点。
//! 调用方：rdc-core 会话编排、rdc-cli 演示。
//!
//! 纪律：观看者不得继承输入通道权限；任何输入注入前必须经过 [`authorize_input`]。

use std::collections::BTreeMap;

use rdc_protocol::{capability_bits, Capabilities};

use crate::authz::{AuthzDecision, Role};

// =====** 能力位 =====**

/// 屏幕墙所需的能力位：子码流 + 多会话 + 屏幕墙。
pub const WALL_CAPABILITY_MASK: u64 =
    capability_bits::SUB_STREAM | capability_bits::MULTI_SESSION | capability_bits::SCREEN_WALL;

/// 在本端能力上声明多会话与屏幕墙。
///
/// 为什么单独提供函数而不是改 [`Capabilities::DEFAULT`]：默认能力位代表「S2 起
/// 全链路已实现的最小集合」，被注册/呼叫等多处复用；屏幕墙与多会话只在 S5 场景启用。
///
/// # Arguments
/// * `base` - 本端基础能力
///
/// # Returns
/// 置位 bit7/bit10/bit11 后的能力集。
///
/// # Errors
/// 不会失败。
pub const fn wall_capabilities(base: Capabilities) -> Capabilities {
    base.with(capability_bits::SUB_STREAM, true)
        .with(capability_bits::MULTI_SESSION, true)
        .with(capability_bits::SCREEN_WALL, true)
}

/// 判断协商结果是否允许屏幕墙。
///
/// # Arguments
/// * `negotiated` - 双方按位与后的能力集
///
/// # Returns
/// 三个位全部置位时返回 `true`。
///
/// # Errors
/// 不会失败。
pub const fn supports_wall(negotiated: Capabilities) -> bool {
    negotiated.contains(WALL_CAPABILITY_MASK)
}

// =====** SessionRole =====**

/// 应用层会话角色。
///
/// 与 [`Role`] 的区别：本枚举只描述「这一路连接是控制者还是观看者」，
/// 不包含被控端所有者；映射关系见 [`SessionRole::as_role`]。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum SessionRole {
    /// 观看者：只允许拉子码流，不得注入输入。
    Viewer,
    /// 控制者：允许输入、剪贴板、文件与主码流。
    Controller,
}

impl SessionRole {
    /// 是否允许注入输入。
    ///
    /// # Returns
    /// 仅控制者返回 `true`。
    ///
    /// # Errors
    /// 不会失败。
    pub const fn can_inject_input(self) -> bool {
        matches!(self, Self::Controller)
    }

    /// 是否允许拉取屏幕墙子码流。
    ///
    /// # Returns
    /// 两种角色都允许（观看者的存在意义就是拉子码流）。
    ///
    /// # Errors
    /// 不会失败。
    pub const fn can_view_wall(self) -> bool {
        true
    }

    /// 映射到授权模块的角色。
    ///
    /// # Returns
    /// 观看者 → [`Role::Viewer`]，控制者 → [`Role::Controller`]。
    ///
    /// # Errors
    /// 不会失败。
    pub const fn as_role(self) -> Role {
        match self {
            Self::Viewer => Role::Viewer,
            Self::Controller => Role::Controller,
        }
    }

    /// 稳定的线上/日志名称。
    ///
    /// # Returns
    /// `"viewer"` 或 `"controller"`。
    ///
    /// # Errors
    /// 不会失败。
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Viewer => "viewer",
            Self::Controller => "controller",
        }
    }
}

/// 输入注入的统一判定点。
///
/// 为什么包一层而不是直接调用 `authz::check`：观看者的拒绝理由必须固定为
/// `viewer_readonly`（审计日志与测试都依赖该字符串），并且未来「临时接管」
/// 只能在这里补充，避免出现第二条判定路径。
///
/// # Arguments
/// * `role` - 该路会话的应用层角色
///
/// # Returns
/// 允许/拒绝判定。
///
/// # Errors
/// 不会失败。
pub fn authorize_input(role: SessionRole) -> AuthzDecision {
    match role {
        SessionRole::Controller => AuthzDecision::allow("role_granted"),
        SessionRole::Viewer => AuthzDecision::deny("viewer_readonly"),
    }
}

// =====** SessionSlot =====**

/// 一路已注册会话的静态信息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionSlot {
    /// 会话标识（信令/传输层分配的 session id 的数值化表示）。
    pub session_id: u64,
    /// 应用层角色。
    pub role: SessionRole,
    /// 对端设备标识（设备列表的 `device_id`）。
    pub device_id: String,
    /// 注册时刻（Unix 毫秒）。
    pub started_ms: u64,
}

// =====** MultiSessionError =====**

/// 多会话注册错误。
#[derive(Debug, thiserror::Error)]
pub enum MultiSessionError {
    /// 达到并发上限。
    #[error("并发会话已达上限 {max}，拒绝新会话（对端 {device_id}）")]
    LimitReached {
        /// 上限。
        max: usize,
        /// 被拒绝的对端。
        device_id: String,
    },
    /// 会话标识重复。
    #[error("会话 {session_id} 已存在")]
    Duplicate {
        /// 重复的会话标识。
        session_id: u64,
    },
    /// 会话不存在。
    #[error("会话 {session_id} 不存在")]
    NotFound {
        /// 缺失的会话标识。
        session_id: u64,
    },
}

// =====** SessionRegistry =====**

/// 多会话注册表。
///
/// 只记录静态信息与角色，不持有网络对象；实际收发仍由各会话所有者负责。
#[derive(Debug, Clone)]
pub struct SessionRegistry {
    /// 并发上限（含控制者与观看者）。
    max_sessions: usize,
    /// 按会话标识有序保存。
    slots: BTreeMap<u64, SessionSlot>,
}

impl SessionRegistry {
    /// 构造注册表。
    ///
    /// # Arguments
    /// * `max_sessions` - 并发上限，`0` 会被提升为 1（无上限语义不允许）。
    ///
    /// # Returns
    /// 空注册表。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new(max_sessions: usize) -> Self {
        Self {
            max_sessions: max_sessions.max(1),
            slots: BTreeMap::new(),
        }
    }

    /// 并发上限。
    ///
    /// # Returns
    /// 构造时给定的上限（至少 1）。
    ///
    /// # Errors
    /// 不会失败。
    pub fn max_sessions(&self) -> usize {
        self.max_sessions
    }

    /// 当前会话数。
    ///
    /// # Returns
    /// 已注册的会话条数。
    ///
    /// # Errors
    /// 不会失败。
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    /// 是否为空。
    ///
    /// # Returns
    /// 无会话时返回 `true`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// 注册一路会话。
    ///
    /// # Arguments
    /// * `slot` - 会话静态信息
    ///
    /// # Returns
    /// 注册成功返回 `Ok(())`。
    ///
    /// # Errors
    /// 标识重复返回 [`MultiSessionError::Duplicate`]；达到上限返回
    /// [`MultiSessionError::LimitReached`]。
    pub fn register(&mut self, slot: SessionSlot) -> Result<(), MultiSessionError> {
        if self.slots.contains_key(&slot.session_id) {
            return Err(MultiSessionError::Duplicate {
                session_id: slot.session_id,
            });
        }
        if self.slots.len() >= self.max_sessions {
            return Err(MultiSessionError::LimitReached {
                max: self.max_sessions,
                device_id: slot.device_id,
            });
        }
        self.slots.insert(slot.session_id, slot);
        Ok(())
    }

    /// 注销一路会话。
    ///
    /// # Arguments
    /// * `session_id` - 会话标识
    ///
    /// # Returns
    /// 原本存在时返回 `true`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn unregister(&mut self, session_id: u64) -> bool {
        self.slots.remove(&session_id).is_some()
    }

    /// 查询会话。
    ///
    /// # Arguments
    /// * `session_id` - 会话标识
    ///
    /// # Returns
    /// 命中返回槽位引用。
    ///
    /// # Errors
    /// 不会失败。
    pub fn get(&self, session_id: u64) -> Option<&SessionSlot> {
        self.slots.get(&session_id)
    }

    /// 某角色的会话数。
    ///
    /// # Arguments
    /// * `role` - 应用层角色
    ///
    /// # Returns
    /// 该角色的会话条数。
    ///
    /// # Errors
    /// 不会失败。
    pub fn count_role(&self, role: SessionRole) -> usize {
        self.slots.values().filter(|s| s.role == role).count()
    }
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// M1：能力位声明与协商；旧对端缺少的位被丢弃。
    #[test]
    fn wall_capabilities_negotiate() {
        let ours = wall_capabilities(Capabilities::DEFAULT);
        assert!(ours.contains(capability_bits::MULTI_SESSION));
        assert!(ours.contains(capability_bits::SCREEN_WALL));
        assert!(ours.contains(capability_bits::SUB_STREAM));
        assert!(supports_wall(ours));

        // 旧对端只有默认能力：屏幕墙三个位都协商不出来。
        let negotiated = ours.negotiate(Capabilities::DEFAULT);
        assert!(!supports_wall(negotiated));
        assert!(!negotiated.contains(capability_bits::MULTI_SESSION));

        assert!(supports_wall(ours.negotiate(ours)));
    }

    /// M2：观看者不得注入输入，控制者允许。
    #[test]
    fn viewer_cannot_inject() {
        let viewer = authorize_input(SessionRole::Viewer);
        assert!(!viewer.allowed);
        assert_eq!(viewer.reason, "viewer_readonly");
        let ctrl = authorize_input(SessionRole::Controller);
        assert!(ctrl.allowed);
        assert!(SessionRole::Viewer.can_view_wall());
        assert!(!SessionRole::Viewer.can_inject_input());
        assert!(SessionRole::Controller.can_inject_input());
    }

    /// M3：达到上限拒绝、释放后可再注册、重复标识拒绝。
    #[test]
    fn registry_enforces_limit() {
        let mut registry = SessionRegistry::new(3);
        for id in 0..3u64 {
            registry
                .register(SessionSlot {
                    session_id: id,
                    role: SessionRole::Viewer,
                    device_id: format!("dev-{id}"),
                    started_ms: 1,
                })
                .expect("前 3 路应成功");
        }
        assert_eq!(registry.len(), 3);
        assert_eq!(registry.count_role(SessionRole::Viewer), 3);
        let rejected = registry.register(SessionSlot {
            session_id: 3,
            role: SessionRole::Viewer,
            device_id: "dev-3".to_owned(),
            started_ms: 2,
        });
        assert!(matches!(rejected, Err(MultiSessionError::LimitReached { .. })));
        assert!(registry.unregister(0));
        assert!(registry.register(SessionSlot {
            session_id: 3,
            role: SessionRole::Viewer,
            device_id: "dev-3".to_owned(),
            started_ms: 3,
        }).is_ok());
        let duplicate = registry.register(SessionSlot {
            session_id: 3,
            role: SessionRole::Viewer,
            device_id: "dev-3".to_owned(),
            started_ms: 4,
        });
        assert!(matches!(duplicate, Err(MultiSessionError::Duplicate { .. })));
        assert_eq!(registry.get(3).map(|s| s.started_ms), Some(3));
    }

    /// 上限 0 视为 1，避免出现「永不接受任何会话」的配置。
    #[test]
    fn zero_limit_is_one() {
        let registry = SessionRegistry::new(0);
        assert_eq!(registry.max_sessions(), 1);
        assert!(registry.is_empty());
    }
}
