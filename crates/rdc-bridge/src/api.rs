//! rdc-bridge / 对外 API
//!
//! 层级：L4 桥接层
//! 用途：flutter_rust_bridge 的扫描入口（rust_input: crate::api）；
//!       每个 `pub fn` 会生成一个同名 Dart 函数
//! 调用方：生成的 Dart 绑定，再由 model/gateway/rust_gateway.dart 统一收口

// =====** 版本与平台 =====**

/// 返回 Rust 核心版本号。
///
/// # Returns
/// 形如 `"0.1.0"` 的字符串；Flutter 首页显示在版本卡片上。
///
/// # Errors
/// 不会失败。
pub fn core_version() -> String {
    rdc_core::core_version()
}

/// 返回构建平台（`os-arch`）。
///
/// # Returns
/// 形如 `"windows-x86_64"` 的字符串。
///
/// # Errors
/// 不会失败。
pub fn platform_name() -> String {
    rdc_core::platform_name()
}

// =====** 桥接状态 =====**

/// 探测 FFI 链路是否真正可用。
///
/// # Returns
/// 固定返回 `"OK"`：能执行到本函数即证明 Dart → Rust 调用链已打通。
///
/// # Errors
/// 不会失败。
pub fn bridge_status() -> String {
    // 这是 S1 的最小闭环验证点：Dart 侧拿到 "OK" 即代表动态库装载、符号解析均成功。
    "OK".to_owned()
}

// =====** 自检 =====**

/// 返回 5 行环境自检结果（docs/10 §2.2 第 3 条）。
///
/// # Returns
/// 每条为 `[OK] 名称: 详情` 的字符串数组，顺序固定。
///
/// # Errors
/// 不会失败；单项失败以文本标注，不抛错。
pub fn doctor_report() -> Vec<String> {
    rdc_core::run_doctor().iter().map(|check| check.line()).collect()
}
