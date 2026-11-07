//! rdc-bridge · FFI 桥接出口
//!
//! 层级：L4 桥接层
//! 用途：把 rdc-core 的命令与只读数据暴露给 flutter_rust_bridge 生成的 Dart API；
//!       S5-2 起新增设备列表、设置读写、会话角色与屏幕墙预览。
//! 调用方：Flutter 的 model/gateway/rust_gateway.dart（经由生成的 src/rust/ 代码）
//!
//! 安全与所有权：除控制页外部纹理所需的 **共享纹理句柄与事件句柄**（仅 S5-3，
//! 由 runner 的 control_texture.cpp 消费，Dart 只做 i64 透传）外，
//! 不暴露裸指针或持久 GPU 资源；其余跨 FFI 返回值都是可安全复制的可序列化类型。

// =====** 模块声明 =====**

pub mod api;
// Android 启动自检：纯 C ABI 出口，不经过 flutter_rust_bridge。
pub mod android_selfcheck;
#[cfg(windows)]
mod control_service;
#[cfg(target_os = "linux")]
mod control_service_linux;
mod clipboard_service;
mod file_service;
mod input_service;
mod session_service;
mod video_service;
mod wall_service;
// 该文件由 flutter_rust_bridge_codegen 生成，勿手工编辑。
mod frb_generated;
