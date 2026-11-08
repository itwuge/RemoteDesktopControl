//! rdc-bridge / FFI 错误载体
//!
//! 层级：L4 桥接层
//! 用途：在桥接层内部保留错误链（IO / JSON / 设备列表 / 设置），只在 FFI 边界
//!       转成 Dart 侧约定的文本；文本与改造前的 `Display` 输出完全一致。
//! 调用方：`api.rs` 及各 `*_service` 模块（R-C-3 治理）。

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
        source: Box<dyn std::error::Error + Send + Sync>,
    },
    /// 设备列表读写。
    #[error(transparent)]
    Device(#[from] rdc_core::s5::devices::DeviceError),
    /// 设置读写。
    #[error(transparent)]
    Settings(#[from] rdc_core::s5::settings::SettingsError),
    /// 文件/网络 IO。
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// JSON 序列化。
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl ApiError {
    /// 按具体错误类型取源错误，保留 downcast 判断路径。
    ///
    /// 用途：桥接层若需要按具体错误类型分支（例如区分 `NotFound` 与格式错误），
    /// 通过本方法取回原始错误，而不是依赖 `Box<dyn Error>` 做字符串匹配。
    pub fn downcast_ref<E>(&self) -> Option<&E>
    where
        E: std::error::Error + 'static,
    {
        use std::error::Error as _;
        self.source().and_then(|source| source.downcast_ref::<E>())
    }
}

/// FFI 边界：把错误链渲染为 Dart 侧文本。
///
/// 文本与改造前 `error.to_string()` 完全一致；结构化链路到此为止，
/// 需要排查时在调用点用 `tracing::debug!` 记录 `?error`。
pub fn boundary_text(error: &ApiError) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    write!(out, "{error}").expect("String 写入不会失败");
    out
}
