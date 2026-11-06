//! rdc-bridge · FFI 桥接出口
//!
//! 层级：L4 桥接层
//! 用途：把 rdc-core 的命令与只读数据暴露给 flutter_rust_bridge 生成的 Dart API；
//!       S1 只暴露版本、平台、桥接状态与自检结果四个最小命令
//! 调用方：Flutter 的 model/gateway/rust_gateway.dart（经由生成的 src/rust/ 代码）
//!
//! 安全与所有权：本 crate **不**暴露裸指针、GPU 句柄或像素缓冲；
//! 所有返回值都是可跨 FFI 安全复制的 `String` / `Vec<String>`。

// =====** 模块声明 =====**

pub mod api;
// 该文件由 flutter_rust_bridge_codegen 生成，勿手工编辑。
mod frb_generated;
