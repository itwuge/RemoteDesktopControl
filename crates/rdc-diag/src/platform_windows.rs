//! ============================================================
//! rdc-diag · Windows 平台错误格式化
//!
//! 层级：L1 基础设施（rdc-diag 内部模块）。
//! 用途：统一 HRESULT / Win32 错误码的文本形式，并尽量取回系统文案，
//!       保证日志里「数值 + 可读原因」同时存在（docs/16 §3 错误不丢信息）。
//! 调用方：error.rs 的 `PlatformError::Display`、各 Windows 平台后端的诊断输出。
//! 边界：非 Windows 平台只做数值格式化，`system_message` 返回 `None`，不做假数据。
//! ============================================================

// =====** 数值格式化 =====**

/// 把 HRESULT 渲染成 8 位十六进制（`0x80004005`）。
///
/// # Arguments
/// * `hresult` - 有符号 HRESULT；内部按无符号位模式输出，负值不会被截断。
pub fn format_hresult(hresult: i32) -> String {
    format!("0x{:08X}", hresult as u32)
}

/// 把 Win32 错误码渲染成十六进制（`0x5`）。
pub fn format_win32(code: u32) -> String {
    format!("0x{:X}", code)
}

// =====** 系统文案 =====**

/// 取系统对该错误码的可读文案；取不到返回 `None`。
///
/// # 说明
/// * 只读查询，不改变 `GetLastError` 的语义；
/// * 返回值已去掉结尾换行，避免破坏单行日志格式；
/// * 非 Windows 平台恒为 `None`（不编造文案）。
pub fn system_message(code: u32) -> Option<String> {
    #[cfg(windows)]
    {
        windows_message(code)
    }
    #[cfg(not(windows))]
    {
        system_message_unavailable(code)
    }
}

/// 非 Windows 平台的占位实现：没有系统消息表，明确返回 `None`。
#[cfg(not(windows))]
fn system_message_unavailable(_code: u32) -> Option<String> {
    None
}

/// 由 HRESULT 与可选 Win32 码构造平台旁证。
///
/// # Arguments
/// * `hresult` - 优先展示的 HRESULT；
/// * `win32` - `GetLastError` 结果，调用方没有时传 `None`。
pub fn platform_error(hresult: i32, win32: Option<u32>) -> crate::error::PlatformError {
    // 系统文案按 Win32 码优先、HRESULT 次之查询：两者编码空间不通用。
    let message_code = win32.unwrap_or(hresult as u32);
    crate::error::PlatformError {
        hresult: Some(hresult),
        win32,
        system_message: system_message(message_code),
    }
}

// =====** Windows FFI =====**

#[cfg(windows)]
mod win32 {
    //! 仅声明 `FormatMessageW`，避免为一条文案引入额外 crate 特性。

    use core::ffi::c_void;

    /// `FORMAT_MESSAGE_FROM_SYSTEM`：从系统消息表取文案。
    pub const FROM_SYSTEM: u32 = 0x0000_1000;
    /// `FORMAT_MESSAGE_IGNORE_INSERTS`：不解析 `%1` 占位符。
    pub const IGNORE_INSERTS: u32 = 0x0000_0200;

    #[link(name = "kernel32")]
    extern "system" {
        /// 取系统错误文案；签名与 Win32 SDK 一致。
        pub fn FormatMessageW(
            flags: u32,
            source: *const c_void,
            message_id: u32,
            language_id: u32,
            buffer: *mut u16,
            size: u32,
            arguments: *const *const u8,
        ) -> u32;
    }
}

/// Windows 下真正调用 `FormatMessageW`。
#[cfg(windows)]
fn windows_message(code: u32) -> Option<String> {
    // 512 个 UTF-16 单元足够覆盖系统错误文案；超长会被截断但不越界。
    let mut buffer = [0u16; 512];
    // SAFETY: 缓冲区与长度一致，flags 只使用只读查询标志，arguments 传空指针符合文档要求。
    let written = unsafe {
        win32::FormatMessageW(
            win32::FROM_SYSTEM | win32::IGNORE_INSERTS,
            core::ptr::null(),
            code,
            0,
            buffer.as_mut_ptr(),
            buffer.len() as u32,
            core::ptr::null(),
        )
    };
    if written == 0 {
        return None;
    }
    let text = String::from_utf16_lossy(&buffer[..written as usize]);
    let trimmed = text.trim_end_matches(['\r', '\n', '\0']);
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_hresult_without_sign_loss() {
        assert_eq!(format_hresult(-2147467259), "0x80004005");
        assert_eq!(format_hresult(0), "0x00000000");
    }

    #[test]
    fn formats_win32_code() {
        assert_eq!(format_win32(5), "0x5");
        assert_eq!(format_win32(0x2A), "0x2A");
    }

    #[test]
    fn platform_error_keeps_both_codes() {
        let error = platform_error(-2147467259, Some(5));
        assert_eq!(error.hresult, Some(-2147467259));
        assert_eq!(error.win32, Some(5));
    }

    #[cfg(windows)]
    #[test]
    fn system_message_returns_text_for_access_denied() {
        // 5 = ERROR_ACCESS_DENIED，系统消息表必然存在该条目。
        let text = system_message(5).expect("ERROR_ACCESS_DENIED must have a message");
        assert!(!text.contains('\n'), "message must be single line: {text}");
    }

    #[cfg(not(windows))]
    #[test]
    fn system_message_is_none_off_windows() {
        assert!(system_message(5).is_none());
    }
}
