//! rdc-bridge / FFI 错误载体
//!
//! 层级：L4 桥接层
//! 用途：在桥接层内部保留错误链（IO / JSON / 设备列表 / 设置 / 授权存储），
//!       只在 FFI 边界转成 Dart 侧约定的文本；文本与改造前的 `Display` 输出完全一致。
//! 调用方：`api.rs` 及各 `*_service` 模块（R-C-3 治理）。

use std::error::Error as StdError;

use thiserror::Error;

/// 桥接层错误。
#[derive(Debug, Error)]
pub enum ApiError {
    /// 业务/参数错误：文本即用户可见文案（等价于旧的 `String` 错误）。
    #[error("{0}")]
    Message(String),
    /// 带上下文与源错误的包装；源链路保留，供日志与 downcast 排查。
    #[error("{context}: {source}")]
    Context {
        /// 出错的短上下文（如「读取设备列表」）。
        context: &'static str,
        /// 原始错误；保留 `source()` 链。
        #[source]
        source: Box<dyn StdError + Send + Sync>,
    },
    /// 对外只显示固定文案，同时保留源错误供排查。
    ///
    /// 为什么单独建这个变体：部分校验点的用户文案是固定短句（不含源错误细节），
    /// 但源错误本身仍要能取回，不能像 `map_err(|_| 常量)` 那样丢掉。
    #[error("{message}")]
    WithSource {
        /// 用户可见的固定文案。
        message: &'static str,
        /// 被固定文案遮蔽的源错误。
        #[source]
        source: Box<dyn StdError + Send + Sync>,
    },
    /// 设备列表读写。
    #[error("{0}")]
    Device(#[from] #[source] rdc_core::s5::devices::DeviceError),
    /// 设置读写。
    #[error("{0}")]
    Settings(#[from] #[source] rdc_core::s5::settings::SettingsError),
    /// 授权存储读写。
    #[error("{0}")]
    GrantStore(#[from] #[source] rdc_core::s5::grant::GrantStoreError),
    /// 文件/网络 IO。
    #[error("{0}")]
    Io(#[from] #[source] std::io::Error),
    /// JSON 序列化。
    #[error("{0}")]
    Json(#[from] #[source] serde_json::Error),
    /// 输入注入（平台相关）。
    #[error("{0}")]
    Input(#[from] #[source] rdc_input::InputError),
    /// 协议编解码。
    #[error("{0}")]
    Protocol(#[from] #[source] rdc_protocol::ProtocolError),
}

impl From<String> for ApiError {
    /// 旧接口以 `String` 表示错误，统一收敛为 [`ApiError::Message`]（文案不变）。
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

impl ApiError {
    /// 保留源错误但对外只显示固定文案。
    pub fn with_source<E>(message: &'static str, source: E) -> Self
    where
        E: StdError + Send + Sync + 'static,
    {
        Self::WithSource {
            message,
            source: Box::new(source),
        }
    }

    /// 按具体错误类型取源错误，保留 downcast 判断路径。
    ///
    /// 用途：桥接层若需要按具体错误类型分支（例如区分 `NotFound` 与格式错误），
    /// 通过本方法取回原始错误，而不是依赖 `Box<dyn Error>` 做字符串匹配。
    pub fn downcast_ref<E>(&self) -> Option<&E>
    where
        E: StdError + 'static,
    {
        self.source().and_then(|source| source.downcast_ref::<E>())
    }
}

/// FFI 边界：把错误链渲染为 Dart 侧文本。
///
/// 文本与改造前 `error.to_string()` 完全一致；结构化链路到此为止，
/// 需要排查时在调用点用 `tracing::debug!` 记录 `?error`。
pub fn boundary_text(error: impl Into<ApiError>) -> String {
    use std::fmt::Write as _;
    let error = error.into();
    let mut out = String::new();
    write!(out, "{error}").expect("String 写入不会失败");
    out
}

/// 互斥锁中毒的 FFI 文本。
///
/// 为什么不能挂 `#[source]`：`PoisonError<MutexGuard<'_, T>>` 不满足
/// `Send + Sync`，无法装入 `Box<dyn Error + Send + Sync>`；这属于「取不到 source」
/// 的边界。这里显式消费中毒错误并沿用调用方给定的文案，输出与旧实现逐字一致。
pub fn lock_poisoned_text<T>(message: &'static str, poison: std::sync::PoisonError<T>) -> String {
    drop(poison);
    message.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use rdc_core::s5::devices::DeviceError;

    /// `#[from]` + `#[source]` 同时挂上后，`source()` 必须取到 `DeviceError` 本体。
    ///
    /// 为什么必须断言：`#[error(transparent)]` 会把 `source()` 转发到内层错误的
    /// `source()`，导致 `downcast_ref::<DeviceError>()` 落空；改用 `#[error("{0}")]`
    /// 才能保住 downcast 路径，同时 Display 仍是原文案。
    #[test]
    fn device_variant_source_keeps_device_error() {
        let error = ApiError::from(DeviceError::UnsupportedVersion {
            found: 2,
            supported: 1,
        });
        let source = StdError::source(&error).expect("Device 变体的 source 应为 DeviceError");
        assert!(
            source.downcast_ref::<DeviceError>().is_some(),
            "source 必须是 DeviceError 本体"
        );
        assert!(error.downcast_ref::<DeviceError>().is_some());
        assert_eq!(
            boundary_text(error),
            "设备列表版本不支持：文件 2，本端上限 1"
        );
    }

    /// `WithSource` 必须同时保留固定文案与源错误。
    #[test]
    fn with_source_keeps_message_and_source() {
        let error = ApiError::with_source(
            "公钥格式非法",
            std::io::Error::new(std::io::ErrorKind::InvalidData, "inner"),
        );
        let source = StdError::source(&error).expect("WithSource 的 source 必须存在");
        assert!(
            source.downcast_ref::<std::io::Error>().is_some(),
            "source 必须是原始错误"
        );
        assert_eq!(boundary_text(error), "公钥格式非法");
    }

    /// 旧 `String` 错误收敛后文案逐字不变。
    #[test]
    fn string_error_text_is_unchanged() {
        let message = "授权不存在：abc".to_owned();
        assert_eq!(boundary_text(message.clone()), message);
    }

    /// 设备列表：真实 JSON 解析失败下，旧 `to_string()` 与新边界文本逐字一致。
    #[test]
    fn device_text_parity_with_legacy_display() {
        let json_error = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
        let legacy = rdc_core::s5::devices::DeviceError::Format(json_error);
        let before = legacy.to_string();
        let after = boundary_text(ApiError::from(legacy));
        assert_eq!(after, before);
        assert_eq!(
            after,
            "设备列表格式错误：EOF while parsing an object at line 1 column 1"
        );
    }

    /// 设置：真实 JSON 解析失败下，旧 `to_string()` 与新边界文本逐字一致。
    #[test]
    fn settings_text_parity_with_legacy_display() {
        let json_error = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
        let legacy = rdc_core::s5::settings::SettingsError::Format(json_error);
        let before = legacy.to_string();
        let after = boundary_text(ApiError::from(legacy));
        assert_eq!(after, before);
        assert_eq!(
            after,
            "设置格式错误：EOF while parsing an object at line 1 column 1"
        );
    }

    /// 授权存储：真实 JSON 解析失败下，旧 `to_string()` 与新边界文本逐字一致。
    #[test]
    fn grant_store_text_parity_with_legacy_display() {
        let json_error = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
        let legacy = rdc_core::s5::grant::GrantStoreError::Format(json_error);
        let before = legacy.to_string();
        let after = boundary_text(ApiError::from(legacy));
        assert_eq!(after, before);
        assert_eq!(
            after,
            "授权存储格式错误：EOF while parsing an object at line 1 column 1"
        );
    }
}
