//! rdc-protocol / 端口常量
//!
//! 层级：L2 能力层
//! 用途：端口号单一真源，禁止在服务端或客户端写魔法数字（docs/09 §5.3）
//! 调用方：rdc-signal、rdc-relay、rdc-connect、packaging 脚本

// =====** 端口定义 =====**

/// 信令服务主端口（UDP）。
pub const PORT_SIGNAL: u16 = 21116;

/// 中继媒体转发端口（UDP）。
pub const PORT_RELAY_MEDIA: u16 = 21117;

/// 中继控制端口（TCP）。
pub const PORT_RELAY_CONTROL: u16 = 21118;

/// 健康检查端口（HTTP，供运维探活）。
pub const PORT_HEALTH: u16 = 21119;
