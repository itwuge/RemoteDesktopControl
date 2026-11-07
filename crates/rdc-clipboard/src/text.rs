//! rdc-clipboard / 文本换行归一化与大小上限
//!
//! 层级：L3 交互层
//! 用途：线上统一 LF，本机平台按需转换（Windows 剪贴板用 CRLF）；统一大小上限常量。
//! 调用方：rdc-clipboard 的线格式、同步状态机与平台实现
//!
//! 约定：只折叠 `\r\n`，不处理单独 `\r`（老式 Mac 换行不进协议，避免误改用户内容）。

use std::borrow::Cow;

/// 单条文本上限：1 MiB（docs/04 §2.1）。
pub const MAX_TEXT_BYTES: usize = 1 << 20;

// =====** Newlines =====**

/// 平台换行风格。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Newlines {
    /// LF（Unix / 线上统一格式）。
    Lf,
    /// CRLF（Windows 剪贴板）。
    CrLf,
}

// =====** 转换 =====**

/// 本地文本 → 线上格式：`\r\n` 折叠为 `\n`。
///
/// # Arguments
/// * `text` - 本机剪贴板原始文本
///
/// # Returns
/// 不含 `\r\n` 的文本；没有 `\r\n` 时借用原字符串，不分配。
///
/// # Errors
/// 不会失败。
pub fn to_network(text: &str) -> Cow<'_, str> {
    if text.contains("\r\n") {
        Cow::Owned(text.replace("\r\n", "\n"))
    } else {
        Cow::Borrowed(text)
    }
}

/// 线上文本 → 平台格式。
///
/// # Arguments
/// * `text` - 线上文本（LF）
/// * `style` - 目标平台换行风格
///
/// # Returns
/// 按 `style` 转换后的文本；无需转换时借用原字符串。
///
/// # Errors
/// 不会失败。
pub fn to_platform(text: &str, style: Newlines) -> Cow<'_, str> {
    match style {
        Newlines::Lf => to_network(text),
        Newlines::CrLf => {
            // 先折叠既有 CRLF，避免出现 \r\r\n。
            let normalized = to_network(text);
            if normalized.contains('\n') {
                Cow::Owned(normalized.replace('\n', "\r\n"))
            } else {
                normalized
            }
        }
    }
}

// =====** 自检 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// CRLF 折叠为 LF，且不误改单独 CR。
    #[test]
    fn network_normalization() {
        assert_eq!(to_network("a\r\nb").as_ref(), "a\nb");
        assert_eq!(to_network("a\rb").as_ref(), "a\rb");
        assert_eq!(to_network("no newline").as_ref(), "no newline");
        // 已经是 LF：借用，不产生新分配。
        assert!(matches!(to_network("a\nb"), Cow::Borrowed(_)));
    }

    /// Windows 方向：LF → CRLF，且不会重复加 CR。
    #[test]
    fn platform_crlf_conversion() {
        assert_eq!(to_platform("a\nb", Newlines::CrLf).as_ref(), "a\r\nb");
        assert_eq!(to_platform("a\r\nb", Newlines::CrLf).as_ref(), "a\r\nb");
        assert_eq!(
            to_platform("no newline", Newlines::CrLf).as_ref(),
            "no newline"
        );
        // 单独 CR 保持原样，不被当成换行。
        assert_eq!(to_platform("a\rb", Newlines::CrLf).as_ref(), "a\rb");
    }

    /// LF 平台不需要额外转换。
    #[test]
    fn platform_lf_platform() {
        assert_eq!(to_platform("a\r\nb", Newlines::Lf).as_ref(), "a\nb");
    }

    /// Unicode（含代理对/emoji）往返不丢字符。
    #[test]
    fn unicode_survives_conversion() {
        let text = "中文🙂\r\n组合 é\n";
        let network = to_network(text);
        assert_eq!(network.as_ref(), "中文🙂\n组合 é\n");
        let back = to_platform(network.as_ref(), Newlines::CrLf);
        assert_eq!(back.as_ref(), "中文🙂\r\n组合 é\r\n");
        assert_eq!(to_network(back.as_ref()).as_ref(), network.as_ref());
    }
}
