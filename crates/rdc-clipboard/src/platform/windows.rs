//! rdc-clipboard / Windows 剪贴板实现
//!
//! 层级：L3 交互层 · 平台适配
//! 用途：读写 `CF_UNICODETEXT`，并用 `GetClipboardSequenceNumber` 识别自写回环。
//! 调用方：rdc-clipboard 的平台工厂（由 rdc-core 剪贴板轮询线程持有）
//!
//! 回环防护：
//! - 写入成功后记下序列号；轮询时若序列号未变 → 没有新变化；
//! - 序列号等于上次自写序列号 → 是本次写入引起的通知，直接返回 `None`；
//! - 这是第二道防线，第一道是 `sync` 层的内容哈希（见 `ClipboardSync`）。
//!
//! 换行：Windows 剪贴板惯用 CRLF；线上用 LF。读入折叠为 LF，写入前展开为 CRLF。

use std::mem::size_of;

use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber,
    IsClipboardFormatAvailable, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};

use super::Clipboard;
use crate::error::ClipboardError;
use crate::text::{self, Newlines};

/// 剪贴板文本格式（`CF_UNICODETEXT`，Win32 头文件里的常量未由 windows crate 导出）。
const CF_UNICODETEXT: u32 = 13;

// =====** PlatformClipboard =====**

/// Windows 剪贴板访问。
///
/// 线程约定：只由剪贴板轮询线程独占使用；`OpenClipboard` 打开期间不要执行阻塞操作。
pub struct PlatformClipboard {
    /// 上次观察到的剪贴板序列号（含自写）。
    last_seen_seq: u32,
    /// 上次由本进程写入后的序列号，用于识别回环。
    own_seq: Option<u32>,
}

impl PlatformClipboard {
    /// 构造实现，并记录当前序列号作为基线。
    ///
    /// # Returns
    /// 以当前剪贴板状态为基线（会话开始前已有的内容不会自动同步）。
    ///
    /// # Errors
    /// 不会失败；序列号查询不会报错。
    pub fn new() -> Self {
        let seq = unsafe { GetClipboardSequenceNumber() };
        Self {
            last_seen_seq: seq,
            own_seq: None,
        }
    }

    /// 读取当前剪贴板文本（未打开失败时返回错误）。
    fn read_platform_text() -> Result<Option<String>, ClipboardError> {
        unsafe {
            OpenClipboard(None).map_err(platform_error)?;
            let result = (|| {
                if IsClipboardFormatAvailable(CF_UNICODETEXT).is_err() {
                    return Ok(None);
                }
                let handle = GetClipboardData(CF_UNICODETEXT).map_err(platform_error)?;
                let hglobal = HGLOBAL(handle.0 as *mut core::ffi::c_void);
                let ptr = GlobalLock(hglobal) as *const u16;
                if ptr.is_null() {
                    return Err(ClipboardError::Platform("GlobalLock 返回空指针".into()));
                }
                // GlobalSize 是字节数，按 u16 数截断，防止末尾不齐。
                let units = GlobalSize(hglobal) / size_of::<u16>();
                let mut len = 0usize;
                while len < units && *ptr.add(len) != 0 {
                    len += 1;
                }
                let text = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
                let _ = GlobalUnlock(hglobal);
                Ok(Some(text))
            })();
            let _ = CloseClipboard();
            result
        }
    }

    /// 写入文本（已转换为平台换行）。
    fn write_text_platform(text: &str) -> Result<(), ClipboardError> {
        let mut utf16: Vec<u16> = text.encode_utf16().collect();
        utf16.push(0); // CF_UNICODETEXT 要求以 NUL 结尾
        let bytes = utf16.len() * size_of::<u16>();
        unsafe {
            let hmem = GlobalAlloc(GMEM_MOVEABLE, bytes).map_err(platform_error)?;
            let ptr = GlobalLock(hmem) as *mut u16;
            if ptr.is_null() {
                let _ = GlobalFree(hmem);
                return Err(ClipboardError::Platform("GlobalLock 返回空指针".into()));
            }
            std::ptr::copy_nonoverlapping(utf16.as_ptr(), ptr, utf16.len());
            let _ = GlobalUnlock(hmem);

            if let Err(err) = OpenClipboard(None) {
                let _ = GlobalFree(hmem);
                return Err(platform_error(err));
            }
            let result = (|| {
                EmptyClipboard().map_err(platform_error)?;
                // 成功后内存所有权交给系统，不能再 free。
                SetClipboardData(CF_UNICODETEXT, HANDLE(hmem.0 as isize))
                    .map_err(platform_error)?;
                Ok(())
            })();
            let _ = CloseClipboard();
            if result.is_err() {
                let _ = GlobalFree(hmem);
            }
            result
        }
    }
}

impl Clipboard for PlatformClipboard {
    fn poll_text(&mut self) -> Result<Option<String>, ClipboardError> {
        let seq = unsafe { GetClipboardSequenceNumber() };
        if seq == self.last_seen_seq {
            return Ok(None); // 没有新变化
        }
        self.last_seen_seq = seq;
        if self.own_seq == Some(seq) {
            return Ok(None); // 本次变化是我们自己写入引起的
        }
        let Some(text) = Self::read_platform_text()? else {
            return Ok(None);
        };
        if text.is_empty() {
            return Ok(None);
        }
        Ok(Some(text::to_network(&text).into_owned()))
    }

    fn read_text(&mut self) -> Result<Option<String>, ClipboardError> {
        Ok(Self::read_platform_text()?.map(|text| text::to_network(&text).into_owned()))
    }

    fn write_text(&mut self, text: &str) -> Result<(), ClipboardError> {
        let platform_text = text::to_platform(text, Newlines::CrLf);
        Self::write_text_platform(platform_text.as_ref())?;
        let seq = unsafe { GetClipboardSequenceNumber() };
        self.last_seen_seq = seq;
        self.own_seq = Some(seq);
        Ok(())
    }
}

/// 系统错误 → 平台错误（保留原文便于日志定位）。
fn platform_error(err: windows::core::Error) -> ClipboardError {
    ClipboardError::Platform(err.message())
}

// =====** 自检：只验证纯函数，不做真实剪贴板读写 =====**

#[cfg(test)]
mod tests {
    use super::*;
    use crate::text::Newlines;

    /// 写入方向的换行转换：LF → CRLF，且不会重复加 CR。
    #[test]
    fn write_converts_to_crlf() {
        assert_eq!(text::to_platform("a\nb", Newlines::CrLf).as_ref(), "a\r\nb");
        assert_eq!(
            text::to_platform("a\r\nb", Newlines::CrLf).as_ref(),
            "a\r\nb"
        );
    }

    /// 读取方向的换行折叠：CRLF → LF。
    #[test]
    fn read_folds_to_lf() {
        assert_eq!(text::to_network("a\r\nb\r\n").as_ref(), "a\nb\n");
    }

    /// UTF-16 编码以 NUL 结尾（`CF_UNICODETEXT` 的硬性要求）。
    #[test]
    fn utf16_is_nul_terminated() {
        let mut units: Vec<u16> = "ab".encode_utf16().collect();
        units.push(0);
        assert_eq!(units, vec![0x61, 0x62, 0]);
        assert_eq!(units.len() * size_of::<u16>(), 6);
    }
}
