//! rdc-protocol / 协议版本
//!
//! 层级：L2 能力层
//! 用途：版本号单一真源，服务端与客户端共用（docs/09 §5.3）
//! 调用方：rdc-signal、rdc-connect、rdc-transport、rdc-bridge

// =====** 版本常量 =====**

/// 协议主版本：不兼容变更时必须递增，且服务端要能同时接受旧主版本一段时间。
pub const PROTOCOL_VERSION_MAJOR: u16 = 1;

/// 协议次版本：向后兼容的新增字段/能力位只递增此号。
pub const PROTOCOL_VERSION_MINOR: u16 = 0;

/// 协议版本组合值，线格式为 `u16 major || u16 minor`，便于报文里用单个 u32 表达。
pub const PROTOCOL_VERSION: u32 = ((PROTOCOL_VERSION_MAJOR as u32) << 16) | PROTOCOL_VERSION_MINOR as u32;

// =====** 版本工具 =====**

/// 生成 `major.minor` 形式的可读版本串。
///
/// # Returns
/// 形如 `"1.0"` 的静态分配字符串，用于日志与 `rdc-cli doctor` 输出。
///
/// # Errors
/// 不会失败（编译期常量拼接）。
pub fn version_string() -> String {
    // 为什么要手工拼接：避免引入 format! 的运行时分配以外的任何依赖；
    // 这里仅低频调用（启动与自检），可接受一次分配。
    format!("{}.{}", PROTOCOL_VERSION_MAJOR, PROTOCOL_VERSION_MINOR)
}
