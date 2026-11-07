//! rdc-core / S5 多设备与屏幕墙（内核部分）
//!
//! 层级：L3 编排层
//! 用途：设备列表、多会话与角色、屏幕墙子码流、本机设置持久化。
//! 调用方：rdc-cli 的 `demo-wall`、rdc-bridge（Flutter 网关）。

pub mod devices;
pub mod multi;
pub mod settings;
pub mod wall;
