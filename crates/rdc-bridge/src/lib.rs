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
#[cfg(windows)]
mod control_service;
#[cfg(target_os = "linux")]
mod control_service_linux;
mod clipboard_service;
mod file_service;
mod input_service;
mod session_service;
mod wall_service;
// 该文件由 flutter_rust_bridge_codegen 生成，勿手工编辑。
mod frb_generated;

// =====** 诊断初始化 =====**

/// 进程内唯一的诊断 guard；Flutter 进程的 Rust 侧没有 `main`，
/// 因此把首次 FFI 调用当作进程入口（docs/16 §2.6）。
static DIAG_GUARD: std::sync::OnceLock<rdc_diag::LogGuard> = std::sync::OnceLock::new();

/// 幂等初始化诊断日志；重复调用不会重复注册 subscriber。
pub(crate) fn ensure_diagnostics() {
    let guard = DIAG_GUARD.get_or_init(|| rdc_diag::init("bridge"));
    // 初始化后立刻冲刷一次启动行，保证随后崩溃也能留下入口证据。
    guard.flush();
}
