// ============================================================
// 文件用途：Android 动态库自检入口（C ABI，供 Dart FFI 直接调用）
//
// 层级：L4 桥接层（附加诊断出口，不参与 flutter_rust_bridge 生成）
// 被谁调用：app/lib/model/gateway/android_selfcheck.dart（启动时一次）
// 边界：只返回常量字符串；不初始化任何业务状态、不分配跨 FFI 的内存
//
// 为什么单独提供 C 入口：
//   FRB 的 `bridge_status()` 需要整套 FRB 初始化成功才可调用；
//   本函数只依赖 dlopen + 符号解析，能在「库装载成功但 FRB 初始化失败」
//   时仍然给出可判定的证据（自检字符串）。
// ============================================================

use std::ffi::{c_char, CString};
use std::sync::OnceLock;

/// 自检字符串的静态存储：首次调用时构造，之后复用同一块内存。
static SELF_CHECK: OnceLock<CString> = OnceLock::new();

/// 返回自检字符串（以 NUL 结尾的 C 字符串）。
///
/// # Returns
/// 指向静态内存的指针，内容形如
/// `rdc-bridge selfcheck OK os=android arch=aarch64`。
/// 指针在进程生命周期内有效，调用方不得释放。
///
/// # Safety
/// 本函数为 `extern "C"` 导出符号，只返回静态常量指针；
/// Dart 侧必须以只读方式读取，不得写入或释放。
#[no_mangle]
pub extern "C" fn rdc_selfcheck() -> *const c_char {
    self_check_string().as_ptr()
}

/// 构造（或复用）自检字符串。
///
/// # Returns
/// 静态 `CString` 引用；编译期常量保证不会失败。
fn self_check_string() -> &'static CString {
    SELF_CHECK.get_or_init(|| {
        // 编译目标信息直接写进字符串：交叉编译产物一眼可辨（android/aarch64）。
        let text = format!(
            "rdc-bridge selfcheck OK os={} arch={} abi={}",
            std::env::consts::OS,
            std::env::consts::ARCH,
            std::env::consts::FAMILY
        );
        // 文本只含 ASCII，unwrap 不会触发。
        CString::new(text).expect("selfcheck 文本不含 NUL")
    })
}
