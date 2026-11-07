//! rdc-core / S5 本机设备列表
//!
//! 层级：L3 编排层
//! 用途：维护本机已知设备（设备 ID、名称、最后在线时间、是否信任、是否同账号），
//!       以本地 JSON 文件持久化，写入走「临时文件 + rename」的原子写。
//! 调用方：rdc-core 多会话编排、rdc-bridge（展示设备列表）。
//!
//! 范围说明：`same_account` 只保留字段与读写，不参与任何判定（同账号免密互信在
//! 账号服务落地后接入，见 docs/04 §2.4 bit12）。

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

// =====** 常量 =====**

/// 设备列表文件格式版本；升级字段时必须同步调整解析。
pub const DEVICE_FILE_VERSION: u32 = 1;

/// 原子写使用的临时文件后缀。
const TEMP_SUFFIX: &str = ".tmp";

// =====** DeviceRecord =====**

/// 单台设备的记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceRecord {
    /// 设备 ID（对端身份公钥指纹或稳定机器标识，由调用方决定）。
    pub device_id: String,
    /// 用户可读名称。
    pub name: String,
    /// 最后在线时间（Unix 毫秒）；0 表示从未在线。
    pub last_online_ms: u64,
    /// 是否信任该设备（本地策略，不由对端声明）。
    pub trusted: bool,
    /// 是否与当前账号同源。
    ///
    /// 为什么现在就留字段：协议 bit12 已定义，UI/账号服务未落地；字段随列表一起
    /// 持久化可避免后续迁移文件格式。
    pub same_account: bool,
}

impl DeviceRecord {
    /// 构造一条设备记录。
    ///
    /// # Arguments
    /// * `device_id` - 设备标识
    /// * `name` - 显示名
    ///
    /// # Returns
    /// 未信任、不同账号、最后在线为 0 的记录。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new(device_id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            device_id: device_id.into(),
            name: name.into(),
            last_online_ms: 0,
            trusted: false,
            same_account: false,
        }
    }
}

// =====** DeviceListFile =====**

/// 设备列表的磁盘表示（版本 + 设备数组）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct DeviceListFile {
    /// 文件格式版本。
    version: u32,
    /// 设备记录，按 `device_id` 排序保证输出稳定。
    devices: Vec<DeviceRecord>,
}

// =====** DeviceError =====**

/// 设备列表错误。
#[derive(Debug, thiserror::Error)]
pub enum DeviceError {
    /// 文件读写失败。
    #[error("设备列表 IO 失败：{0}")]
    Io(#[from] std::io::Error),
    /// JSON 解析/序列化失败。
    #[error("设备列表格式错误：{0}")]
    Format(#[from] serde_json::Error),
    /// 文件版本超出本端支持范围。
    #[error("设备列表版本不支持：文件 {found}，本端上限 {supported}")]
    UnsupportedVersion {
        /// 文件中的版本号。
        found: u32,
        /// 本端支持的最高版本。
        supported: u32,
    },
}

// =====** DeviceList =====**

/// 本机设备列表（内存视图 + 原子持久化）。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeviceList {
    /// 以 `device_id` 为键，保证查找与输出顺序稳定。
    devices: BTreeMap<String, DeviceRecord>,
}

impl DeviceList {
    /// 空列表。
    ///
    /// # Returns
    /// 不含任何设备的列表。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new() -> Self {
        Self::default()
    }

    /// 从 JSON 文件加载；文件不存在时返回空列表。
    ///
    /// # Arguments
    /// * `path` - 设备列表文件路径
    ///
    /// # Returns
    /// 解析后的列表。
    ///
    /// # Errors
    /// 文件存在但不可读、JSON 损坏或版本不受支持时返回 [`DeviceError`]。
    pub fn load(path: impl AsRef<Path>) -> Result<Self, DeviceError> {
        let path = path.as_ref();
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Self::new()),
            Err(err) => return Err(err.into()),
        };
        let file: DeviceListFile = serde_json::from_slice(&bytes)?;
        if file.version > DEVICE_FILE_VERSION {
            return Err(DeviceError::UnsupportedVersion {
                found: file.version,
                supported: DEVICE_FILE_VERSION,
            });
        }
        let mut list = Self::new();
        for record in file.devices {
            list.devices.insert(record.device_id.clone(), record);
        }
        Ok(list)
    }

    /// 原子写入 JSON 文件。
    ///
    /// 为什么用临时文件 + rename：设备列表是可信状态，半截文件会让下次启动
    /// 读到不完整数据；rename 在同一目录内是原子替换（Windows/Linux 同语义）。
    ///
    /// # Arguments
    /// * `path` - 目标文件路径；父目录不存在时自动创建
    ///
    /// # Returns
    /// 写入成功返回 `Ok(())`。
    ///
    /// # Errors
    /// 目录创建、临时文件写入、flush/sync 或 rename 失败时返回 [`DeviceError`]。
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), DeviceError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let file = DeviceListFile {
            version: DEVICE_FILE_VERSION,
            devices: self.devices.values().cloned().collect(),
        };
        let bytes = serde_json::to_vec_pretty(&file)?;
        let temp = temp_path(path);
        {
            let mut handle = std::fs::File::create(&temp)?;
            handle.write_all(&bytes)?;
            handle.sync_all()?;
        }
        std::fs::rename(&temp, path)?;
        Ok(())
    }

    /// 设备数量。
    ///
    /// # Returns
    /// 列表中的设备条数。
    ///
    /// # Errors
    /// 不会失败。
    pub fn len(&self) -> usize {
        self.devices.len()
    }

    /// 是否为空。
    ///
    /// # Returns
    /// 无设备时返回 `true`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
    }

    /// 按设备 ID 查找。
    ///
    /// # Arguments
    /// * `device_id` - 设备标识
    ///
    /// # Returns
    /// 命中返回记录引用。
    ///
    /// # Errors
    /// 不会失败。
    pub fn get(&self, device_id: &str) -> Option<&DeviceRecord> {
        self.devices.get(device_id)
    }

    /// 全部设备（按 ID 升序）。
    ///
    /// # Returns
    /// 记录切片（由调用方只读使用）。
    ///
    /// # Errors
    /// 不会失败。
    pub fn devices(&self) -> Vec<&DeviceRecord> {
        self.devices.values().collect()
    }

    /// 上线/更新一条设备记录，并返回是否为首次出现。
    ///
    /// 名称与最后在线时间由本次上线刷新；`trusted` / `same_account` 属于本地策略，
    /// **不**被对端声明覆盖，只有 [`DeviceList::set_trusted`] 等显式调用可改。
    ///
    /// # Arguments
    /// * `device_id` - 设备标识
    /// * `name` - 本次看到的名称
    /// * `now_ms` - 当前 Unix 毫秒
    ///
    /// # Returns
    /// 首次出现返回 `true`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn upsert_online(&mut self, device_id: &str, name: &str, now_ms: u64) -> bool {
        match self.devices.get_mut(device_id) {
            Some(record) => {
                record.name = name.to_owned();
                record.last_online_ms = now_ms;
                false
            }
            None => {
                let mut record = DeviceRecord::new(device_id, name);
                record.last_online_ms = now_ms;
                self.devices.insert(device_id.to_owned(), record);
                true
            }
        }
    }

    /// 修改显示名（本地别名），不刷新最后在线时间。
    ///
    /// # Arguments
    /// * `device_id` - 设备标识
    /// * `name` - 新显示名；空串视为非法，直接返回 `false`
    ///
    /// # Returns
    /// 设备存在且名称非空时返回 `true`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn set_name(&mut self, device_id: &str, name: &str) -> bool {
        let name = name.trim();
        if name.is_empty() {
            return false;
        }
        match self.devices.get_mut(device_id) {
            Some(record) => {
                record.name = name.to_owned();
                true
            }
            None => false,
        }
    }

    /// 设置信任位。
    ///
    /// # Arguments
    /// * `device_id` - 设备标识
    /// * `trusted` - 是否信任
    ///
    /// # Returns
    /// 设备存在时返回 `true`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn set_trusted(&mut self, device_id: &str, trusted: bool) -> bool {
        match self.devices.get_mut(device_id) {
            Some(record) => {
                record.trusted = trusted;
                true
            }
            None => false,
        }
    }

    /// 设置同账号位（仅字段，不参与判定）。
    ///
    /// # Arguments
    /// * `device_id` - 设备标识
    /// * `same_account` - 是否同账号
    ///
    /// # Returns
    /// 设备存在时返回 `true`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn set_same_account(&mut self, device_id: &str, same_account: bool) -> bool {
        match self.devices.get_mut(device_id) {
            Some(record) => {
                record.same_account = same_account;
                true
            }
            None => false,
        }
    }

    /// 移除一台设备。
    ///
    /// # Arguments
    /// * `device_id` - 设备标识
    ///
    /// # Returns
    /// 原先存在并移除时返回 `true`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn remove(&mut self, device_id: &str) -> bool {
        self.devices.remove(device_id).is_some()
    }
}

/// 当前 Unix 毫秒。
///
/// # Returns
/// 系统时间不可用（早于 1970）时返回 0。
///
/// # Errors
/// 不会失败。
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// 由目标路径推导同目录临时文件路径。
fn temp_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "devices.json".to_owned());
    name.push_str(TEMP_SUFFIX);
    path.with_file_name(name)
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个测试用独立目录，避免并发测试互相覆盖。
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rdc-s5-devices-{}-{tag}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(&dir).expect("创建临时目录");
        dir
    }

    /// D1：保存→加载往返一致，字段齐全。
    #[test]
    fn save_load_roundtrip() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("devices.json");
        let mut list = DeviceList::new();
        list.upsert_online("dev-a", "工作站 A", 1_000);
        list.set_trusted("dev-a", true);
        list.set_same_account("dev-a", true);
        list.upsert_online("dev-b", "笔记本 B", 2_000);
        list.save(&path).expect("保存");

        let loaded = DeviceList::load(&path).expect("加载");
        assert_eq!(loaded.len(), 2);
        let a = loaded.get("dev-a").expect("A 存在");
        assert_eq!(a.name, "工作站 A");
        assert_eq!(a.last_online_ms, 1_000);
        assert!(a.trusted);
        assert!(a.same_account);
        let b = loaded.get("dev-b").expect("B 存在");
        assert!(!b.trusted);
        // 临时文件不得残留（原子写的可见证据）。
        assert!(!dir.join("devices.json.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// D2：缺失文件是空列表；损坏 JSON 返回错误而不是 panic。
    #[test]
    fn missing_and_corrupt_files() {
        let dir = temp_dir("corrupt");
        let path = dir.join("missing.json");
        assert!(DeviceList::load(&path).expect("缺失文件应视为空").is_empty());

        let bad = dir.join("bad.json");
        std::fs::write(&bad, b"{not json").expect("写入坏文件");
        assert!(DeviceList::load(&bad).is_err());

        let future = dir.join("future.json");
        std::fs::write(&future, br#"{"version":99,"devices":[]}"#).expect("写入");
        assert!(matches!(
            DeviceList::load(&future),
            Err(DeviceError::UnsupportedVersion { found: 99, .. })
        ));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// D3：再次上线只刷新名称与时间，保留本地信任位。
    #[test]
    fn upsert_preserves_local_policy() {
        let mut list = DeviceList::new();
        assert!(list.upsert_online("dev-a", "旧名", 100));
        list.set_trusted("dev-a", true);
        list.set_same_account("dev-a", true);
        assert!(!list.upsert_online("dev-a", "新名", 500));
        let record = list.get("dev-a").expect("存在");
        assert_eq!(record.name, "新名");
        assert_eq!(record.last_online_ms, 500);
        assert!(record.trusted, "本地信任位不得被上线刷新覆盖");
        assert!(record.same_account);
        assert!(list.remove("dev-a"));
        assert!(list.is_empty());
        assert!(!list.set_trusted("dev-a", true));
    }

    /// D4：原子写不会留下临时文件，并且覆盖已有文件。
    #[test]
    fn atomic_overwrite() {
        let dir = temp_dir("atomic");
        let path = dir.join("devices.json");
        let mut list = DeviceList::new();
        list.upsert_online("dev-a", "A", 10);
        list.save(&path).expect("首次保存");
        list.upsert_online("dev-b", "B", 20);
        list.save(&path).expect("覆盖保存");
        let loaded = DeviceList::load(&path).expect("加载");
        assert_eq!(loaded.len(), 2);
        assert!(!dir.join("devices.json.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
