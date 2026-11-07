//! rdc-bridge · FFI 桥接出口
//!
//! 层级：L4 桥接层
//! 用途：把 rdc-core 的命令与只读数据暴露给 flutter_rust_bridge 生成的 Dart API；
//!       S5-2 起新增设备列表、设置读写、会话角色与屏幕墙预览。
//! 调用方：Flutter 的 model/gateway/rust_gateway.dart（经由生成的 src/rust/ 代码）
//!
//! 安全与所有权：本 crate **不**暴露裸指针、GPU 句柄或持久 GPU 资源；
//! 跨 FFI 的返回值都是可安全复制的 `String` / `Vec<String>` / `Vec<u8>`；
//! 屏幕墙预览每帧拷贝 RGBA 字节，Dart 侧不持有 Rust 内存。

// =====** 模块声明 =====**

pub mod api;
mod wall_service;
// 该文件由 flutter_rust_bridge_codegen 生成，勿手工编辑。
mod frb_generated;
