//! ============================================================
//! rdc-diag · 诊断码表与结构化错误
//!
//! 层级：L1 基础设施（rdc-diag 内部模块）。
//! 用途：集中维护 `RDC-<域>-<编号>` 诊断码、可重试性与用户提示，并提供可跨 FFI
//!       传输的 `RdcErrorJson` 与平台错误旁证 `PlatformError`（docs/16 §3）。
//! 调用方：各业务 crate 的错误映射、rdc-bridge 的 FFI 返回值、doctor 报告。
//! 纪律：新增错误变体必须同时追加码表与单测；码表编号不允许重复。
//! ============================================================

// =====** 模块导入 =====**

use std::fmt;

use serde::{Deserialize, Serialize};

// =====** 码表 =====**

/// 一条诊断码的静态描述。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiagEntry {
    /// 诊断码，形如 `RDC-CAP-001`。
    pub code: &'static str,
    /// 域缩写：`CAP`/`COD`/`DEC`/`TRN`/`CON`/`SEC`/`RT`。
    pub domain: &'static str,
    /// 是否建议用户重试（写进 UI 与 doctor）。
    pub retryable: bool,
    /// 面向用户的一句话建议。
    pub hint: &'static str,
}

/// 完整诊断码表；顺序即展示顺序，新增条目追加在对应域末尾。
///
/// 与 docs/16 §3.2 一一对应；未列入此表的码视为非法，`doctor` 必须报错。
pub const CODES: &[DiagEntry] = &[
    // --- 采集域 ---
    DiagEntry { code: "RDC-CAP-001", domain: "CAP", retryable: false, hint: "请在系统设置中允许屏幕录制" },
    DiagEntry { code: "RDC-CAP-002", domain: "CAP", retryable: true, hint: "显示器已变化，请重新选择" },
    DiagEntry { code: "RDC-CAP-003", domain: "CAP", retryable: true, hint: "显示器被其他程序独占" },
    DiagEntry { code: "RDC-CAP-004", domain: "CAP", retryable: false, hint: "当前平台不支持该采集方式" },
    DiagEntry { code: "RDC-CAP-005", domain: "CAP", retryable: false, hint: "采集失败，请查看诊断日志" },
    DiagEntry { code: "RDC-CAP-006", domain: "CAP", retryable: true, hint: "桌面无变化或采集被阻止" },
    // --- 编码域 ---
    DiagEntry { code: "RDC-COD-001", domain: "COD", retryable: false, hint: "无可用硬件编码器" },
    DiagEntry { code: "RDC-COD-002", domain: "COD", retryable: false, hint: "编码配置不受支持" },
    DiagEntry { code: "RDC-COD-003", domain: "COD", retryable: true, hint: "编码失败，请重试" },
    DiagEntry { code: "RDC-COD-004", domain: "COD", retryable: false, hint: "编码器参数设置失败" },
    DiagEntry { code: "RDC-COD-005", domain: "COD", retryable: false, hint: "当前平台不支持该编码能力" },
    // --- 解码域 ---
    DiagEntry { code: "RDC-DEC-001", domain: "DEC", retryable: false, hint: "解码器不可用" },
    DiagEntry { code: "RDC-DEC-002", domain: "DEC", retryable: true, hint: "解码失败，请重试" },
    DiagEntry { code: "RDC-DEC-003", domain: "DEC", retryable: true, hint: "呈现失败，请重启会话" },
    DiagEntry { code: "RDC-DEC-004", domain: "DEC", retryable: false, hint: "解码参数非法" },
    // --- 传输域 ---
    DiagEntry { code: "RDC-TRN-001", domain: "TRN", retryable: true, hint: "网络发送失败，请检查链路" },
    DiagEntry { code: "RDC-TRN-002", domain: "TRN", retryable: true, hint: "发送队列拥塞，已丢弃最旧帧" },
    DiagEntry { code: "RDC-TRN-003", domain: "TRN", retryable: true, hint: "分片丢失且未能恢复" },
    // --- 连接域 ---
    DiagEntry { code: "RDC-CON-001", domain: "CON", retryable: true, hint: "连接超时，请检查对端是否在线" },
    DiagEntry { code: "RDC-CON-002", domain: "CON", retryable: true, hint: "握手失败，请确认两端版本" },
    // --- 安全域 ---
    DiagEntry { code: "RDC-SEC-001", domain: "SEC", retryable: false, hint: "安全校验失败，请重新配对" },
    DiagEntry { code: "RDC-SEC-002", domain: "SEC", retryable: false, hint: "凭据已过期，请重新获取" },
    // --- 运行期 ---
    DiagEntry { code: "RDC-RT-003", domain: "RT", retryable: true, hint: "内部线程异常，已隔离" },
];

/// 按码查表；未登记返回 `None`。
pub fn lookup(code: &str) -> Option<&'static DiagEntry> {
    CODES.iter().find(|entry| entry.code == code)
}

/// 判断诊断码是否在码表内。
pub fn is_known(code: &str) -> bool {
    lookup(code).is_some()
}

/// 返回某域的全部诊断码（`CAP`/`COD`/`DEC`/...），保持表内顺序。
pub fn by_domain(domain: &str) -> Vec<&'static DiagEntry> {
    CODES.iter().filter(|entry| entry.domain == domain).collect()
}

// =====** 平台错误旁证 =====**

/// 平台错误的原始数值与系统文案；不吞掉 HRESULT/Win32 码（docs/16 §3）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformError {
    /// Windows HRESULT（可为负），非 Windows 平台为 `None`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hresult: Option<i32>,
    /// Win32 错误码（`GetLastError`），无则 `None`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub win32: Option<u32>,
    /// 系统给出的可读文案（`FormatMessage`）；取不到时为 `None`。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_message: Option<String>,
}

impl PlatformError {
    /// 仅带 HRESULT 的构造。
    pub fn from_hresult(hresult: i32) -> Self {
        Self {
            hresult: Some(hresult),
            win32: None,
            system_message: None,
        }
    }

    /// 是否没有任何可用旁证（用于省略空字段）。
    pub fn is_empty(&self) -> bool {
        self.hresult.is_none() && self.win32.is_none() && self.system_message.is_none()
    }
}

impl fmt::Display for PlatformError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut parts: Vec<String> = Vec::new();
        if let Some(hresult) = self.hresult {
            parts.push(format!("hresult={}", crate::platform_windows::format_hresult(hresult)));
        }
        if let Some(win32) = self.win32 {
            parts.push(format!("win32={}", crate::platform_windows::format_win32(win32)));
        }
        if let Some(message) = &self.system_message {
            parts.push(format!("system_message=\"{}\"", message));
        }
        write!(formatter, "{}", parts.join(" "))
    }
}

// =====** 跨 FFI 错误 JSON =====**

/// 可跨 FFI 传输的结构化错误；字段与 docs/16 §3 的 JSONL 片段一致。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RdcErrorJson {
    /// 诊断码，必须在 [`CODES`] 内。
    pub code: String,
    /// 面向开发者的事实描述（不含敏感信息）。
    pub message: String,
    /// 面向用户的建议。
    pub hint: String,
    /// 是否建议重试。
    pub retryable: bool,
    /// 平台旁证；为空时不序列化。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<PlatformError>,
}

impl RdcErrorJson {
    /// 由诊断码构造；码表内取提示与可重试性，未知码按不可重试处理并保留原码便于排查。
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        let (hint, retryable) = match lookup(code) {
            Some(entry) => (entry.hint.to_string(), entry.retryable),
            None => ("未知诊断码，请查看诊断日志".to_string(), false),
        };
        Self {
            code: code.to_string(),
            message: message.into(),
            hint,
            retryable,
            platform: None,
        }
    }

    /// 附带平台旁证。
    pub fn with_platform(mut self, platform: PlatformError) -> Self {
        self.platform = if platform.is_empty() { None } else { Some(platform) };
        self
    }

    /// 序列化为单行 JSON；序列化失败时退回只含码与消息的保守文本，避免二次 panic。
    pub fn to_json(&self) -> String {
        match serde_json::to_string(self) {
            Ok(text) => text,
            Err(error) => format!(
                "{{\"code\":\"{}\",\"message\":\"serialize failed: {}\"}}",
                self.code, error
            ),
        }
    }
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique() {
        // 码表唯一性是硬约束：重复编号会让 doctor 与 UI 提示错乱。
        let mut seen = std::collections::BTreeSet::new();
        for entry in CODES {
            assert!(seen.insert(entry.code), "duplicate diag code: {}", entry.code);
        }
    }

    #[test]
    fn documented_codes_are_present() {
        for code in [
            "RDC-CAP-001",
            "RDC-COD-003",
            "RDC-DEC-004",
            "RDC-RT-003",
        ] {
            assert!(is_known(code), "missing diag code: {}", code);
        }
        assert!(!is_known("RDC-XXX-999"));
    }

    #[test]
    fn code_format_is_domain_prefixed() {
        for entry in CODES {
            assert!(entry.code.starts_with("RDC-"), "bad prefix: {}", entry.code);
            assert_eq!(entry.code.split('-').count(), 3, "bad shape: {}", entry.code);
        }
    }

    #[test]
    fn error_json_roundtrip() {
        let error = RdcErrorJson::new("RDC-COD-003", "encoder failed")
            .with_platform(PlatformError::from_hresult(-2147467259));
        let json = error.to_json();
        let parsed: RdcErrorJson = serde_json::from_str(&json).expect("json must roundtrip");
        assert_eq!(parsed, error);
        assert!(json.contains("\"hresult\":-2147467259"));
        assert!(json.contains("RDC-COD-003"));
    }

    #[test]
    fn platform_error_display_keeps_values() {
        let platform = PlatformError {
            hresult: Some(-2147467259),
            win32: Some(5),
            system_message: Some("Access is denied.".to_string()),
        };
        let text = platform.to_string();
        assert!(text.contains("hresult=0x80004005"), "got {text}");
        assert!(text.contains("win32=0x5"), "got {text}");
    }
}
