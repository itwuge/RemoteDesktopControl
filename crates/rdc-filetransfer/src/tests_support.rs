//! rdc-filetransfer / 测试临时文件工具
//!
//! 层级：L3 交互层（仅 `cfg(test)` 编译，不进入产物）
//! 用途：给单元测试生成互不冲突的临时文件路径，避免并行测试互相覆盖。
//! 调用方：本 crate 的 `#[cfg(test)]` 模块

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

/// 进程内自增序号，保证同进程并行测试拿到不同路径。
static SERIAL: AtomicU64 = AtomicU64::new(0);

/// 生成一个临时文件路径（文件本身不创建）。
///
/// # Arguments
/// * `name` - 便于人工识别的标签
///
/// # Returns
/// `%TEMP%/rdc-filetransfer-<pid>-<name>-<serial>.bin`
pub(crate) fn temp_path(name: &str) -> PathBuf {
    let serial = SERIAL.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "rdc-filetransfer-{}-{}-{}.bin",
        std::process::id(),
        name,
        serial
    ))
}
