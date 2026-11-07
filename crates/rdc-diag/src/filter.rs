//! ============================================================
//! rdc-diag · RDC_LOG 过滤表达式解析
//!
//! 层级：L1 基础设施（rdc-diag 内部模块）。
//! 用途：把模块别名展开为 crate target 前缀，并给出未知别名，供 init 打印一次告警；
//!       本模块不读取环境变量（由 lib.rs 在进程入口读一次）。
//! 默认值：`RDC_LOG` 未设置或为空 → `info`；`RDC_LOG=off` → 关闭全部 RDC target。
//! 调用方：lib.rs 的 init。
//! ============================================================

// =====** 常量 =====**

/// 未设置 `RDC_LOG` 时的默认过滤级别（只保留生命周期与异常，不刷屏）。
pub const DEFAULT_FILTER: &str = "info";

/// 模块别名 → crate target 前缀；新增 crate 时必须同步登记（docs/16 §2.2）。
const ALIASES: &[(&str, &str)] = &[
    ("capture", "rdc_capture"),
    ("encode", "rdc_codec"),
    ("codec", "rdc_codec"),
    ("decode", "rdc_decode"),
    ("transport", "rdc_transport"),
    ("net", "rdc_transport"),
    ("connect", "rdc_connect"),
    ("security", "rdc_security"),
    ("input", "rdc_input"),
    ("clipboard", "rdc_clipboard"),
    ("file", "rdc_filetransfer"),
    ("core", "rdc_core"),
    ("bridge", "rdc_bridge"),
    ("app", "rdc_app"),
    ("signal", "rdc_signal"),
    ("relay", "rdc_relay"),
    ("diag", "rdc_diag"),
];

// =====** 结果类型 =====**

/// `RDC_LOG` 的解析结果。
pub struct ParsedFilter {
    /// 展开别名后的 `EnvFilter` 表达式，可直接交给 `EnvFilter::try_new`。
    pub expression: String,
    /// 不在别名表且不以 `rdc_` 开头的 target；初始化时应 warn 一次，不得静默丢弃。
    pub unknown_aliases: Vec<String>,
}

// =====** 解析入口 =====**

/// 解析 `RDC_LOG` 原文。
///
/// # Arguments
/// * `raw` - 环境变量原文；`None` 或空串表示未设置，使用 [`DEFAULT_FILTER`]。
///
/// # Returns
/// 展开后的表达式与未知别名列表；未知别名按原样保留。
pub fn parse(raw: Option<&str>) -> ParsedFilter {
    let text = match raw {
        Some(value) if !value.trim().is_empty() => value.trim(),
        _ => DEFAULT_FILTER,
    };
    let mut parts: Vec<String> = Vec::new();
    let mut unknown: Vec<String> = Vec::new();
    for directive in text.split(',') {
        let directive = directive.trim();
        if directive.is_empty() {
            continue;
        }
        if let Some(expanded) = expand_directive(directive, &mut unknown) {
            parts.push(expanded);
        }
    }
    if parts.is_empty() {
        // 全空指令（例如只写了逗号）等价于未设置，不能让过滤器变成空白。
        parts.push(DEFAULT_FILTER.to_string());
    }
    ParsedFilter {
        expression: parts.join(","),
        unknown_aliases: unknown,
    }
}

// =====** 内部解析 =====**

/// 展开单条 directive；返回 `None` 表示该指令格式不完整，应跳过。
fn expand_directive(directive: &str, unknown: &mut Vec<String>) -> Option<String> {
    match directive.split_once('=') {
        Some((head, level)) => {
            let head = head.trim();
            let level = level.trim();
            if head.is_empty() || level.is_empty() {
                return None;
            }
            let target = match alias_target(head) {
                Some(value) => value,
                None => {
                    if !is_level(head) && !head.starts_with("rdc_") {
                        unknown.push(head.to_string());
                    }
                    head
                }
            };
            Some(format!("{}={}", target, level))
        }
        None => {
            if is_level(directive) {
                return Some(directive.to_string());
            }
            match alias_target(directive) {
                Some(target) => Some(target.to_string()),
                None => {
                    if !directive.starts_with("rdc_") {
                        unknown.push(directive.to_string());
                    }
                    Some(directive.to_string())
                }
            }
        }
    }
}

/// 查别名表；未登记返回 `None`。
fn alias_target(name: &str) -> Option<&'static str> {
    ALIASES
        .iter()
        .find(|(alias, _)| *alias == name)
        .map(|(_, target)| *target)
}

/// 判断 directive 是否为级别名（大小写不敏感）。
fn is_level(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "off" | "error" | "warn" | "info" | "debug" | "trace"
    )
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_info() {
        assert_eq!(parse(None).expression, "info");
        assert_eq!(parse(Some("   ")).expression, "info");
    }

    #[test]
    fn expands_aliases() {
        let parsed = parse(Some("capture=debug,encode=trace"));
        assert_eq!(parsed.expression, "rdc_capture=debug,rdc_codec=trace");
        assert!(parsed.unknown_aliases.is_empty());
    }

    #[test]
    fn keeps_off() {
        assert_eq!(parse(Some("off")).expression, "off");
    }

    #[test]
    fn reports_unknown_alias() {
        let parsed = parse(Some("foo=debug"));
        assert_eq!(parsed.expression, "foo=debug");
        assert_eq!(parsed.unknown_aliases, vec!["foo".to_string()]);
    }

    #[test]
    fn accepts_raw_crate_target() {
        // 已经写成 crate target 的输入不应当被当作未知别名告警（docs/16 §2.2 示例）。
        let parsed = parse(Some("rdc_bridge=debug,diag=debug"));
        assert_eq!(parsed.expression, "rdc_bridge=debug,rdc_diag=debug");
        assert!(parsed.unknown_aliases.is_empty());
    }

    #[test]
    fn empty_directives_fall_back() {
        assert_eq!(parse(Some(", ,")).expression, "info");
    }
}
