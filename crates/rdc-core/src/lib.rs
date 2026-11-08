//! rdc-core · 会话编排层
//!
//! 层级：L3 编排层（把 L2 能力组装成主控端/被控端两条流水线）
//! 用途：会话状态机、三档连接策略、质量阶梯、统计聚合、授权判定、审计日志
//! 调用方：rdc-bridge（FFI）、rdc-cli（无 UI 验证）
//!
//! 纪律：本 crate 永远不知道"界面长什么样"；禁止任何 widget/page/controller 概念。

// =====** 模块声明 =====**

pub mod audit;
pub mod authz;
pub mod doctor;
pub mod error;
pub mod paths;
pub mod policy;
pub mod s4;
pub mod s5;
pub mod session;
pub mod stats;

// =====** 对外再导出 =====**

pub use audit::AuditEvent;
pub use authz::{AuthzDecision, Permission, Role};
pub use doctor::{run_doctor, DoctorCheck};
pub use error::RdcError;
pub use policy::{ConnectionPolicy, QualityLadder, QualityStep};
pub use session::{Session, SessionEvent, SessionState};
pub use stats::{StatsAggregator, StatsSnapshot};

// =====** 版本信息 =====**

/// 返回核心 crate 版本号。
///
/// # Returns
/// 形如 `"0.1.0"` 的字符串，来源为编译期环境变量，S1 首页据此显示版本。
///
/// # Errors
/// 不会失败。
pub fn core_version() -> String {
    env!("CARGO_PKG_VERSION").to_owned()
}

/// 返回本进程构建的目标平台（`os-arch`）。
///
/// # Returns
/// 形如 `"windows-x86_64"` 的字符串。
///
/// # Errors
/// 不会失败。
pub fn platform_name() -> String {
    // 为什么用 std::env::consts：编译期常量，不依赖任何平台 API，移动端同样可用。
    format!("{}-{}", std::env::consts::OS, std::env::consts::ARCH)
}
