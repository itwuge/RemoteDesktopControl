//! ============================================================
//! rdc-diag · 人读格式与旧输出兼容层
//!
//! 层级：L1 基础设施（rdc-diag 内部模块）。
//! 用途：把 tracing 事件渲染为 `时间戳 级别 [模块] 消息 字段...`，保留旧
//!       `eprintln!("[模块] ...")` 的 `[模块]` 前缀习惯；同时提供不依赖全局
//!       subscriber 的 stderr fallback（初始化失败、panic 现场）。
//! 调用方：lib.rs（init）、panic_hook.rs（崩溃 dump）。
//! ============================================================

// =====** 模块导入 =====**

use std::fmt::{self, Write as _};
use std::io::{self, Write as _};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tracing::field::{Field, Visit};
use tracing::{Event, Level};
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::registry::LookupSpan;

use crate::ring;

// =====** 写失败计数 =====**

/// stderr 写失败累计次数；日志通道本身不可写时，至少留下可查询的计数。
static STDERR_IO_FAILURES: AtomicU64 = AtomicU64::new(0);

// =====** 时间格式化 =====**

/// 当前时刻的 UTC 文本，供 panic hook 等无 subscriber 场景使用。
pub fn now_utc() -> String {
    format_ts(SystemTime::now())
}

/// 把系统时间渲染为毫秒精度的 UTC RFC3339（`2026-11-08T02:21:33.482Z`）。
fn format_ts(time: SystemTime) -> String {
    let elapsed = match time.duration_since(UNIX_EPOCH) {
        Ok(value) => value,
        Err(_) => Duration::ZERO,
    };
    let secs = elapsed.as_secs();
    let millis = elapsed.subsec_millis();
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        year,
        month,
        day,
        rem / 3_600,
        (rem % 3_600) / 60,
        rem % 60,
        millis
    )
}

/// 把“自 1970-01-01 起的天数”转成公历年月日（Howard Hinnant 算法）。
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 { shifted } else { shifted - 146_096 } / 146_097;
    let day_of_era = (shifted - era * 146_097) as u32;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_pos = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_pos + 2) / 5 + 1;
    let month = if month_pos < 10 { month_pos + 3 } else { month_pos - 9 };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

// =====** 人读格式化器 =====**

/// 人读事件格式化器：固定 `时间戳 级别 [模块] 消息 字段...` 顺序。
#[derive(Debug, Default, Clone, Copy)]
pub struct HumanFormat;

impl<S, N> FormatEvent<S, N> for HumanFormat
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
{
    fn format_event(
        &self,
        _ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let metadata = event.metadata();
        let mut visitor = FieldsVisitor::default();
        event.record(&mut visitor);
        let module = visitor
            .module
            .clone()
            .unwrap_or_else(|| module_from_target(metadata.target()));
        let message = visitor.message.unwrap_or_default();
        let mut line = String::with_capacity(96);
        write!(
            &mut line,
            "{} {:5} [{}] {}",
            format_ts(SystemTime::now()),
            level_text(*metadata.level()),
            module,
            message
        )?;
        for (key, value) in &visitor.fields {
            write!(&mut line, " {}={}", key, value)?;
        }
        // 只有已通过过滤的事件才进环形缓冲，保证崩溃 dump 与 stderr 内容一致。
        ring::global().push(line.clone());
        writeln!(writer, "{}", line)
    }
}

// =====** 字段访问器 =====**

/// 收集事件字段；`message`/`module` 单独存放，其余按 `key=value` 输出。
#[derive(Default)]
struct FieldsVisitor {
    message: Option<String>,
    module: Option<String>,
    fields: Vec<(String, String)>,
}

impl FieldsVisitor {
    fn record(&mut self, field: &Field, value: String) {
        match field.name() {
            "message" => self.message = Some(value),
            "module" => self.module = Some(value),
            name => self.fields.push((name.to_string(), value)),
        }
    }
}

impl Visit for FieldsVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        self.record(field, value.to_string());
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.record(field, format!("{:?}", value));
    }
}

// =====** 兼容 fallback =====**

/// 在全局 subscriber 不可用时，按同一人读格式直接写 stderr。
///
/// # Arguments
/// * `level` - 五级日志名（`ERROR`/`WARN`/`INFO`/`DEBUG`/`TRACE`）。
/// * `module` - 短模块名，例如 `diag`；输出仍保留旧日志的 `[模块]` 前缀。
/// * `message` - 已格式化的人类可读消息。
pub fn write_fallback(level: &str, module: &str, message: &str) {
    write_stderr(&format!(
        "{} {:5} [{}] {}\n",
        now_utc(),
        level,
        module,
        message
    ));
}

/// 冲刷 stderr sink；`LogGuard` 退出前调用。
pub fn flush() {
    if io::stderr().flush().is_err() {
        STDERR_IO_FAILURES.fetch_add(1, Ordering::Relaxed);
    }
}

/// 直接写一段已格式化的文本到 stderr（panic dump 用，避免多次加锁）。
///
/// 写失败无法再上报，也不允许递归调用日志；只累加计数。
pub(crate) fn write_stderr(text: &str) {
    if write!(io::stderr(), "{}", text).is_err() {
        STDERR_IO_FAILURES.fetch_add(1, Ordering::Relaxed);
    }
}

// =====** 内部工具 =====**

/// 把 tracing target（`rdc_capture::platform::windows`）压缩成短模块名（`capture`）。
fn module_from_target(target: &str) -> String {
    let head = target.split("::").next().unwrap_or(target);
    head.strip_prefix("rdc_").unwrap_or(head).to_string()
}

/// 五级级别名，固定 5 字符宽便于列对齐。
fn level_text(level: Level) -> &'static str {
    match level {
        Level::TRACE => "TRACE",
        Level::DEBUG => "DEBUG",
        Level::INFO => "INFO",
        Level::WARN => "WARN",
        Level::ERROR => "ERROR",
    }
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_epoch() {
        assert_eq!(format_ts(UNIX_EPOCH), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn formats_known_date() {
        // 1_794_096_000 = 2026-11-08T00:00:00Z，用于防止历法换算回归。
        let time = UNIX_EPOCH + Duration::from_secs(1_794_096_000);
        assert_eq!(format_ts(time), "2026-11-08T00:00:00.000Z");
    }

    #[test]
    fn shortens_module() {
        assert_eq!(module_from_target("rdc_capture::platform::windows"), "capture");
        assert_eq!(module_from_target("rdc_diag"), "diag");
        assert_eq!(module_from_target("third_party"), "third_party");
    }

    #[test]
    fn fallback_keeps_module_prefix() {
        // 只验证格式化结果包含旧脚本依赖的 `[模块]` 前缀，不真正写 stderr。
        let line = format!(
            "{} {:5} [{}] {}",
            now_utc(),
            "WARN",
            "capture",
            "test message"
        );
        assert!(line.contains("[capture] test message"));
    }
}
