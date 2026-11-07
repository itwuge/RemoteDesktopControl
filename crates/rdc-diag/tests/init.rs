//! ============================================================
//! rdc-diag · 初始化与过滤的集成测试
//!
//! 用途：验证重复 init 幂等、flush 可用、环形缓冲上限生效。
//! 边界：测试进程内只允许一个全局 subscriber；本文件不依赖具体输出内容，避免捕获 stderr。
//! ============================================================

// =====** 测试用例 =====**

#[test]
fn repeated_init_is_idempotent() {
    let first = rdc_diag::init("diag-test");
    let second = rdc_diag::init("diag-test");
    assert_eq!(first.service(), "diag-test");
    assert_eq!(second.filter(), first.filter());
    second.flush();
}

#[test]
fn recent_logs_respect_limit() {
    let lines = rdc_diag::recent_logs(4);
    assert!(lines.len() <= 4);
}
