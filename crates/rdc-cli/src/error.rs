//! rdc-cli / 演示命令错误包装
//!
//! 层级：L4 验证入口
//! 用途：在保留固定人读文案的同时，把底层具体错误挂进 `source`，
//!       避免 `map_err(|_| ...)` 把真实原因丢掉（R-C-3）。
//! 调用方：`s4demo`、`walldemo`。

use std::error::Error;

/// 固定文案 + 底层错误的薄包装。
///
/// 为什么需要它：演示命令对外统一用 `Box<dyn Error + Send + Sync>`，
/// 直接写 `map_err(|_| "...")` 会丢掉 `Elapsed` 等具体错误；
/// 用本类型可以在不改变人读文案的前提下保留 `source` 链。
#[derive(Debug)]
pub struct WithSource {
    /// 对用户展示的固定文案（逐字保留旧实现）。
    message: String,
    /// 底层具体错误。
    source: Box<dyn Error + Send + Sync>,
}

impl WithSource {
    /// 构造。
    ///
    /// # Arguments
    /// * `message` - 固定人读文案
    /// * `source` - 底层错误
    ///
    /// # Returns
    /// 包装后的错误。
    pub fn new(message: impl Into<String>, source: impl Error + Send + Sync + 'static) -> Self {
        Self {
            message: message.into(),
            source: Box::new(source),
        }
    }
}

impl std::fmt::Display for WithSource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for WithSource {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(self.source.as_ref())
    }
}

/// 消费锁中毒并返回固定文案。
///
/// 为什么不能挂 source：`PoisonError<MutexGuard<'_, T>>` 不满足 `Send + Sync`，
/// 无法装进 `Box<dyn Error + Send + Sync>`；只能显式消费后保留人读文案。
///
/// # Arguments
/// * `message` - 固定人读文案
/// * `poisoned` - 锁中毒错误
///
/// # Returns
/// 固定文案。
pub fn lock_poisoned_text<T>(message: &str, poisoned: std::sync::PoisonError<T>) -> String {
    let _guard = poisoned.into_inner();
    message.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 文案逐字保留，source 可 downcast 到底层类型。
    #[test]
    fn with_source_keeps_message_and_source() {
        let source = std::io::Error::new(std::io::ErrorKind::TimedOut, "底层超时");
        let err = WithSource::new("固定文案", source);
        assert_eq!(err.to_string(), "固定文案");
        let source = std::error::Error::source(&err).expect("必须挂 source");
        assert!(source.downcast_ref::<std::io::Error>().is_some());
    }
}
