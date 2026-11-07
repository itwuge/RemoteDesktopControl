//! rdc-core / S5 多设备与屏幕墙（内核部分）
//!
//! 层级：L3 编排层
//! 用途：设备列表、多会话与角色、屏幕墙子码流。
//! 调用方：rdc-cli 的 `demo-wall`、后续 rdc-bridge。

pub mod devices;
pub mod multi;
pub mod wall;
