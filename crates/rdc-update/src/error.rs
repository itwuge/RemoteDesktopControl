//! rdc-update / 统一错误与错误码
//!
//! 层级：L2 能力层
//! 用途：把网络、校验、升级、回滚失败收敛成「固定错误码 + 中文说明」，
//!       保证 UI 与日志能直接给出可诊断信息，而不是一句 “更新失败”。
//! 调用方：rdc-update 内部模块、rdc-updater CLI、Flutter 更新服务。

/// 自更新错误码（禁止复用于其它模块）。
pub mod codes {
    /// 网络请求失败（DNS/连接/超时）。
    pub const NET_REQUEST: &str = "RDC-UPD-NET-001";
    /// HTTP 状态码非 2xx。
    pub const NET_STATUS: &str = "RDC-UPD-NET-002";
    /// 响应体超过安全上限。
    pub const NET_TOO_LARGE: &str = "RDC-UPD-NET-003";
    /// 发布信息 JSON 无法解析。
    pub const JSON_INVALID: &str = "RDC-UPD-JSON-001";
    /// 版本号非法。
    pub const VER_INVALID: &str = "RDC-UPD-VER-001";
    /// Release 标签与清单版本不一致（疑似拼装/篡改）。
    pub const VER_TAG_MISMATCH: &str = "RDC-UPD-VER-002";
    /// 发布缺少更新清单。
    pub const MANIFEST_MISSING: &str = "RDC-UPD-MANIFEST-001";
    /// 清单字段非法。
    pub const MANIFEST_INVALID: &str = "RDC-UPD-MANIFEST-002";
    /// 缺少签名文件。
    pub const SIG_MISSING: &str = "RDC-UPD-SIG-001";
    /// 签名格式非法。
    pub const SIG_INVALID: &str = "RDC-UPD-SIG-002";
    /// 签名校验失败。
    pub const SIG_UNVERIFIED: &str = "RDC-UPD-SIG-003";
    /// 本端未配置发布公钥（开发构建常见）。
    pub const SIG_KEY_MISSING: &str = "RDC-UPD-SIG-004";
    /// 缺少 `.sha256` 资产。
    pub const HASH_MISSING: &str = "RDC-UPD-HASH-001";
    /// 哈希不匹配。
    pub const HASH_MISMATCH: &str = "RDC-UPD-HASH-002";
    /// `.sha256` 文件格式非法或与清单不一致。
    pub const HASH_FILE_INVALID: &str = "RDC-UPD-HASH-003";
    /// 本地文件读写失败。
    pub const IO: &str = "RDC-UPD-IO-001";
    /// 当前平台不支持自动升级。
    pub const APPLY_UNSUPPORTED: &str = "RDC-UPD-APPLY-001";
    /// 安装器启动失败。
    pub const APPLY_LAUNCH: &str = "RDC-UPD-APPLY-002";
    /// 安装器退出码非 0。
    pub const APPLY_EXIT: &str = "RDC-UPD-APPLY-003";
    /// 安装后版本与目标不符。
    pub const APPLY_VERSION: &str = "RDC-UPD-APPLY-004";
    /// 回滚失败（需要人工用备份目录恢复）。
    pub const ROLLBACK_FAILED: &str = "RDC-UPD-ROLLBACK-001";
    /// 运行环境缺组件（如 rdc-updater 未随包安装）。
    pub const ENV_MISSING: &str = "RDC-UPD-ENV-001";
}

/// 自更新错误：错误码必须可被日志与 UI 直接展示。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateError {
    /// 固定错误码。
    pub code: &'static str,
    /// 中文可读说明（含关键路径/状态码）。
    pub message: String,
}

impl UpdateError {
    /// 构造错误。
    ///
    /// # Arguments
    /// * `code` - [`codes`] 中的常量
    /// * `message` - 中文说明
    ///
    /// # Returns
    /// 错误值。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

impl std::fmt::Display for UpdateError {
    /// 输出 `[错误码] 说明`，便于直接写入日志。
    ///
    /// # Arguments
    /// * `f` - 格式化器
    ///
    /// # Returns
    /// 格式化结果。
    ///
    /// # Errors
    /// 写入失败时返回 `std::fmt::Error`。
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for UpdateError {}

impl From<std::io::Error> for UpdateError {
    /// IO 错误统一映射为 [`codes::IO`]。
    ///
    /// # Arguments
    /// * `error` - 原始 IO 错误
    ///
    /// # Returns
    /// 自更新错误。
    ///
    /// # Errors
    /// 不会失败。
    fn from(error: std::io::Error) -> Self {
        Self::new(codes::IO, format!("本地文件操作失败：{error}"))
    }
}

/// crate 内统一结果类型。
pub type Result<T> = std::result::Result<T, UpdateError>;
