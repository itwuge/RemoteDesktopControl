//! rdc-linux-e2e / 夹具错误载体
//!
//! 层级：演示 crate（不进入生产路径）
//! 用途：把演示夹具原本丢弃的底层错误（命令行解析的 `ParseIntError` 等）挂进
//!       `source()` 链，同时保持用户可见文案与改造前逐字一致。
//! 范围：本模块不依赖 X11，非 Linux 目标同样编译，便于在 Windows 上静态校验
//!       类型与错误链；真正的调用点位于 Linux-only 的 bins 与解码模块。

/// 演示夹具错误：固定文案 + 可选底层错误。
#[derive(Debug)]
pub struct E2eError {
    /// 用户可见文案（与改造前逐字一致）。
    message: String,
    /// 底层错误；互斥锁中毒等无法装箱的场景为 `None`。
    source: Option<Box<dyn std::error::Error + Send + Sync>>,
}

impl E2eError {
    /// 只带文案、没有底层错误的夹具错误。
    ///
    /// # Arguments
    /// * `message` - 用户可见文案
    ///
    /// # Returns
    /// 构造好的错误。
    ///
    /// # Errors
    /// 不会失败。
    pub fn message(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            source: None,
        }
    }

    /// 带底层错误的夹具错误。
    ///
    /// # Arguments
    /// * `message` - 用户可见文案（不得拼接底层错误的文本）
    /// * `source` - 底层错误（`ParseIntError` / `io::Error` 等）
    ///
    /// # Returns
    /// 构造好的错误，`source()` 可取回底层错误。
    ///
    /// # Errors
    /// 不会失败。
    pub fn with_source(
        message: impl Into<String>,
        source: impl Into<Box<dyn std::error::Error + Send + Sync>>,
    ) -> Self {
        Self {
            message: message.into(),
            source: Some(source.into()),
        }
    }
}

impl From<&'static str> for E2eError {
    /// 供 `ok_or("…缺值")?` 直接转换，文案保持原样。
    fn from(message: &'static str) -> Self {
        Self::message(message)
    }
}

impl std::fmt::Display for E2eError {
    /// 只输出用户可见文案，底层错误不进 Display。
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for E2eError {
    /// 返回底层错误引用，供调用方 `downcast_ref` 到具体类型。
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.source
            .as_ref()
            .map(|source| source.as_ref() as &(dyn std::error::Error + 'static))
    }
}

/// 互斥锁中毒的夹具文本。
///
/// 为什么不能挂 `#[source]`：`PoisonError<MutexGuard<'_, T>>` 不满足
/// `Send + Sync`，无法装入 `Box<dyn Error + Send + Sync>`；这属于「取不到
/// source」的边界。这里显式消费中毒错误，并沿用调用方给定的文案，
/// 输出与旧实现逐字一致。
///
/// # Arguments
/// * `message` - 旧实现使用的固定文案
/// * `poison` - 锁中毒错误（仅用于显式消费）
///
/// # Returns
/// 与旧实现逐字一致的文案。
///
/// # Errors
/// 不会失败。
pub fn lock_poisoned_text<T>(message: &'static str, poison: std::sync::PoisonError<T>) -> String {
    drop(poison);
    message.to_owned()
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 文案必须逐字保留，且 source 能追到具体的 `ParseIntError`。
    #[test]
    fn display_keeps_text_and_source_reaches_parse_error() {
        let parse_error = "not-a-number".parse::<u64>().expect_err("必须解析失败");
        let error = E2eError::with_source("--secs 需整数", parse_error);
        assert_eq!(error.to_string(), "--secs 需整数");
        let source = std::error::Error::source(&error).expect("source 必须存在");
        assert!(
            source.downcast_ref::<std::num::ParseIntError>().is_some(),
            "source 应能 downcast 到 ParseIntError"
        );
    }

    /// IO 错误同样可以挂进 source 链。
    #[test]
    fn source_reaches_io_error() {
        let io_error = std::io::Error::other("flush failed");
        let error = E2eError::with_source("发送缓冲冲刷失败", io_error);
        assert_eq!(error.to_string(), "发送缓冲冲刷失败");
        let source = std::error::Error::source(&error).expect("source 必须存在");
        assert!(
            source.downcast_ref::<std::io::Error>().is_some(),
            "source 应能 downcast 到 io::Error"
        );
    }

    /// `ok_or("…")?` 的字符串字面量转换保持原文案且无 source。
    #[test]
    fn str_conversion_keeps_text() {
        let error = E2eError::from("--secs 缺值");
        assert_eq!(error.to_string(), "--secs 缺值");
        assert!(std::error::Error::source(&error).is_none());
    }

    /// 锁中毒只能沿用固定文案，属于无法挂 source 的边界。
    #[test]
    fn lock_poisoned_text_keeps_message() {
        let mutex = std::sync::Mutex::new(0u8);
        let poisoned = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = mutex.lock().expect("首次加锁");
            panic!("刻意制造中毒");
        }));
        assert!(poisoned.is_err(), "测试必须让锁中毒");
        let poison = mutex.lock().expect_err("锁应已中毒");
        assert_eq!(lock_poisoned_text("编码器锁被污染", poison), "编码器锁被污染");
    }
}
