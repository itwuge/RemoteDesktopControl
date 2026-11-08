//! rdc-core / S5 本机设置持久化
//!
//! 层级：L3 编排层
//! 用途：把设置页与远程访问权限页需要的配置收敛为一份带版本号的 JSON，
//!       以「临时文件 + rename」原子写落盘，供 rdc-bridge 暴露给 Flutter。
//! 调用方：rdc-bridge 的 `settings_load` / `settings_save`。
//!
//! 与 Dart 旧实现的关系：字段名与 `app/lib/model/settings/app_settings_model.dart`
//! 保持 snake_case 一致，旧文件可直接读取；新增字段一律有默认值，缺字段不报错。

use std::io::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// =====** 常量 =====**

/// 设置文件格式版本。
pub const SETTINGS_FILE_VERSION: u32 = 1;

/// 原子写临时文件后缀。
const TEMP_SUFFIX: &str = ".tmp";

// =====** SettingsError =====**

/// 设置读写错误。
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    /// 文件读写失败。
    #[error("设置 IO 失败：{0}")]
    Io(#[from] std::io::Error),
    /// JSON 解析/序列化失败。
    #[error("设置格式错误：{0}")]
    Format(#[from] serde_json::Error),
    /// 文件版本超出本端支持范围。
    #[error("设置版本不支持：文件 {found}，本端上限 {supported}")]
    UnsupportedVersion {
        /// 文件中的版本号。
        found: u32,
        /// 本端支持的最高版本。
        supported: u32,
    },
}

// =====** AppSettings =====**

/// 本机设置（含设置页与权限页的全部可持久化项）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppSettings {
    /// 是否允许使用中继（默认关，关闭时零中继连接）。
    pub allow_relay: bool,
    /// 中继服务器地址。
    pub relay_address: String,
    /// 打洞失败策略：`strict_p2p` / `ask_then_fallback` / `auto_fallback`。
    pub connection_policy: String,
    /// 屏幕方向策略：`follow_controller` / `landscape` / `portrait`。
    pub orientation: String,
    /// 文件传输带宽上限占可用上行比例（0.1~0.7）。
    pub file_bandwidth_ratio: f64,
    // =====** 编码参数 =====**
    /// 主码流目标宽（像素）。
    pub video_width: u32,
    /// 主码流目标高（像素）。
    pub video_height: u32,
    /// 主码流目标帧率。
    pub video_fps: u32,
    /// 主码流目标码率（Mbps）。
    pub video_bitrate_mbps: f64,
    /// 断线是否自动重连。
    pub auto_reconnect: bool,
    // =====** 远程访问权限 =====**
    /// 是否允许远程访问本机（默认关）。
    pub remote_access_enabled: bool,
    /// 并发模式：`exclusive` / `one_control_many_watch`。
    pub concurrent_mode: String,
    /// 接管策略：`auto` / `confirm` / `forbid`。
    pub takeover_policy: String,
    /// 本机确认窗口（秒，10~60）。
    pub takeover_timeout_s: u32,
    /// 当前权限模板：`readonly` / `standard` / `full` / `custom`。
    pub permission_template: String,
    /// 七位权限位图（bit0 VIEW … bit6 DISPLAY_SWITCH）。
    pub permission_bits: u32,
    /// 连接时是否常驻本机提示。
    pub local_notice: bool,
    /// 审计日志保留天数（7/30/90）。
    pub audit_retention_days: u32,
    /// 本地配对设备默认策略：`ask` / `auto`。
    pub local_pair_policy: String,
    /// 同账号设备默认策略：`ask` / `auto`。
    pub same_account_policy: String,
    /// 是否允许完全免密（仅内网 + 二次确认，默认关）。
    pub passwordless_policy: bool,
    // =====** 首启引导 =====**
    /// 首启「允许被连 / 不允许」引导是否已完成。
    ///
    /// 只做本地持久化，不参与握手、capabilities 或协议判定；
    /// 旧配置缺少该字段时按 `false` 读取（即仍展示一次引导，不会读崩）。
    pub first_run_completed: bool,
}

impl Default for AppSettings {
    /// 安全默认：中继关、严格 P2P、只读模板、请求确认、保留 30 天。
    fn default() -> Self {
        Self {
            allow_relay: false,
            relay_address: String::new(),
            connection_policy: "strict_p2p".to_owned(),
            orientation: "follow_controller".to_owned(),
            file_bandwidth_ratio: 0.4,
            video_width: 1920,
            video_height: 1080,
            video_fps: 30,
            video_bitrate_mbps: 8.0,
            auto_reconnect: true,
            remote_access_enabled: false,
            concurrent_mode: "exclusive".to_owned(),
            takeover_policy: "confirm".to_owned(),
            takeover_timeout_s: 30,
            permission_template: "readonly".to_owned(),
            permission_bits: 1,
            local_notice: true,
            audit_retention_days: 30,
            local_pair_policy: "ask".to_owned(),
            same_account_policy: "auto".to_owned(),
            passwordless_policy: false,
            first_run_completed: false,
        }
    }
}

impl AppSettings {
    /// 把越界或非法字段拉回合法区间。
    ///
    /// 为什么在写入前净化：配置文件可能被手工编辑，非法值不应传播到编码器或权限判定。
    ///
    /// # Returns
    /// 无返回值；原地修正 `self`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn sanitize(&mut self) {
        self.file_bandwidth_ratio = self.file_bandwidth_ratio.clamp(0.1, 0.7);
        // 编码尺寸只需偶数（YUV420 色度半采样），不强求 16 对齐：1080 是合法高度。
        self.video_width = (self.video_width.clamp(320, 3840) / 2) * 2;
        self.video_height = (self.video_height.clamp(180, 2160) / 2) * 2;
        self.video_fps = self.video_fps.clamp(5, 120);
        self.video_bitrate_mbps = self.video_bitrate_mbps.clamp(0.5, 60.0);
        self.takeover_timeout_s = self.takeover_timeout_s.clamp(10, 60);
        if ![7, 30, 90].contains(&self.audit_retention_days) {
            self.audit_retention_days = 30;
        }
        // VIEW 位不可取消：最低位强制为 1。
        // 掩码按 docs/07 §4 的九位（0x1FF）收敛，避免 DISPLAY_SWITCH / RECONNECT
        // 被旧 0x7F 掩码静默丢弃（docs/13 的「七位」表述与 docs/07 冲突，记决策待办）。
        self.permission_bits = (self.permission_bits & 0x1FF) | 0x01;
        if !matches!(
            self.connection_policy.as_str(),
            "strict_p2p" | "ask_then_fallback" | "auto_fallback"
        ) {
            self.connection_policy = "strict_p2p".to_owned();
        }
        if !matches!(self.orientation.as_str(), "follow_controller" | "landscape" | "portrait") {
            self.orientation = "follow_controller".to_owned();
        }
        if !matches!(
            self.concurrent_mode.as_str(),
            "exclusive" | "one_control_many_watch"
        ) {
            self.concurrent_mode = "exclusive".to_owned();
        }
        if !matches!(self.takeover_policy.as_str(), "auto" | "confirm" | "forbid") {
            self.takeover_policy = "confirm".to_owned();
        }
        if !matches!(
            self.permission_template.as_str(),
            "readonly" | "standard" | "full" | "custom"
        ) {
            self.permission_template = "readonly".to_owned();
        }
        if !matches!(self.local_pair_policy.as_str(), "ask" | "auto") {
            self.local_pair_policy = "ask".to_owned();
        }
        if !matches!(self.same_account_policy.as_str(), "ask" | "auto") {
            self.same_account_policy = "auto".to_owned();
        }
    }

    /// 从文件加载；不存在时返回默认设置。
    ///
    /// # Arguments
    /// * `path` - 设置文件路径
    ///
    /// # Returns
    /// 已净化的设置。
    ///
    /// # Errors
    /// 文件可读但 JSON 损坏、或版本超前时返回 [`SettingsError`]。
    pub fn load(path: impl AsRef<Path>) -> Result<Self, SettingsError> {
        let bytes = match std::fs::read(path.as_ref()) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(err) => return Err(err.into()),
        };
        // 版本字段独立于设置项：先探测再反序列化，避免旧代码把未来版本当普通未知字段。
        let probe: serde_json::Value = serde_json::from_slice(&bytes)?;
        let version = probe
            .get("version")
            .and_then(|value| value.as_u64())
            .unwrap_or(SETTINGS_FILE_VERSION as u64);
        if version > SETTINGS_FILE_VERSION as u64 {
            return Err(SettingsError::UnsupportedVersion {
                found: version as u32,
                supported: SETTINGS_FILE_VERSION,
            });
        }
        let mut settings: Self = serde_json::from_slice(&bytes)?;
        settings.sanitize();
        Ok(settings)
    }

    /// 原子写入文件。
    ///
    /// # Arguments
    /// * `path` - 目标路径；父目录不存在时自动创建
    ///
    /// # Returns
    /// 写入成功返回 `Ok(())`。
    ///
    /// # Errors
    /// 目录创建、写入、flush/sync 或 rename 失败时返回 [`SettingsError`]。
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), SettingsError> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let mut clean = self.clone();
        clean.sanitize();
        let document = serde_json::json!({
            "version": SETTINGS_FILE_VERSION,
        });
        let mut value = serde_json::to_value(&clean)?;
        if let Some(map) = value.as_object_mut() {
            map.insert("version".to_owned(), document["version"].clone());
        }
        let bytes = serde_json::to_vec_pretty(&value)?;
        let temp = temp_path(path);
        // `create` 成功即代表这个临时文件归本次调用所有，后续任何失败都只删它。
        let mut handle = std::fs::File::create(&temp)?;
        let written = handle.write_all(&bytes).and_then(|()| handle.sync_all());
        drop(handle);
        if let Err(err) = written {
            // 清理失败不改变“写入失败”这一主因，但把清理错误并入返回信息，避免静默丢失上下文。
            let cleanup = std::fs::remove_file(&temp);
            return Err(match cleanup {
                Ok(()) => err.into(),
                Err(cleanup_err) => SettingsError::Io(std::io::Error::new(
                    err.kind(),
                    format!("{err}；写入失败后清理临时文件失败：{cleanup_err}"),
                )),
            });
        }
        if let Err(err) = std::fs::rename(&temp, path) {
            // 目标只读/被占用时 rename 失败：写了一半的临时文件不能留到下次启动。
            let cleanup = std::fs::remove_file(&temp);
            return Err(match cleanup {
                Ok(()) => err.into(),
                Err(cleanup_err) => SettingsError::Io(std::io::Error::new(
                    err.kind(),
                    format!("{err}；重命名失败后清理临时文件失败：{cleanup_err}"),
                )),
            });
        }
        Ok(())
    }

    /// 清理上次异常退出遗留的临时文件（启动时调用一次）。
    ///
    /// 只在主文件存在且能按当前版本完整解析（视为「完好」）时删除 `*.tmp`；
    /// 主文件缺失或损坏时保留临时文件，避免误删唯一可能可恢复的副本。
    ///
    /// # Arguments
    /// * `path` - 主设置文件路径
    ///
    /// # Returns
    /// 删除成功返回被删的临时文件路径；未删除时返回 `None`。
    ///
    /// # Errors
    /// 不会失败；读不到主文件或删除失败一律按「不清理」处理。
    pub fn cleanup_stale_temp(path: impl AsRef<Path>) -> Option<PathBuf> {
        let path = path.as_ref();
        let temp = temp_path(path);
        if !temp.is_file() {
            return None;
        }
        match std::fs::metadata(path) {
            Ok(meta) if meta.is_file() && meta.len() > 0 => {}
            _ => return None,
        }
        if Self::load(path).is_err() {
            return None;
        }
        if std::fs::remove_file(&temp).is_err() {
            return None;
        }
        tracing::warn!("清理上次残留的临时文件: {}", temp.display());
        Some(temp)
    }
}

/// 由目标路径推导同目录临时文件路径。
fn temp_path(path: &Path) -> PathBuf {
    let mut name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "settings.json".to_owned());
    name.push_str(TEMP_SUFFIX);
    path.with_file_name(name)
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 独立临时目录，避免用例互相覆盖。
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rdc-s5-settings-{}-{tag}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0)
        ));
        std::fs::create_dir_all(&dir).expect("创建临时目录");
        dir
    }

    /// 往返一致且默认值符合安全基线。
    #[test]
    fn save_load_roundtrip() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("settings.json");
        let defaults = AppSettings::default();
        assert!(!defaults.allow_relay);
        assert_eq!(defaults.connection_policy, "strict_p2p");
        assert_eq!(defaults.permission_template, "readonly");
        assert_eq!(defaults.permission_bits & 1, 1);

        let mut changed = defaults.clone();
        changed.video_fps = 60;
        changed.takeover_policy = "auto".to_owned();
        changed.save(&path).expect("保存");
        let loaded = AppSettings::load(&path).expect("加载");
        assert_eq!(loaded, changed);
        assert!(!dir.join("settings.json.tmp").exists());
        let _ = std::fs::remove_dir_all(&dir); // gate-allow R-C-2: 用例收尾删除独立临时目录，失败不影响断言结论
    }

    /// 写入失败时不得残留本次创建的临时文件。
    ///
    /// 构造方式：在目标文件位置放一个目录，使最后的 rename 必然失败，
    /// 模拟只读/被占用导致保存失败的现实路径。
    #[test]
    fn save_failure_leaves_no_temp() {
        let dir = temp_dir("save-fail");
        let path = dir.join("settings.json");
        std::fs::create_dir_all(&path).expect("用目录占住目标路径");
        let error = AppSettings::default().save(&path).expect_err("rename 失败应报错");
        assert!(matches!(error, SettingsError::Io(_)));
        assert!(!dir.join("settings.json.tmp").exists(), "失败后临时文件必须清理");
        let _ = std::fs::remove_dir_all(&dir); // gate-allow R-C-2: 用例收尾删除独立临时目录，失败不影响断言结论
    }

    /// 主文件完好时，启动清理删除上次遗留的临时文件。
    #[test]
    fn cleanup_stale_temp_when_main_intact() {
        let dir = temp_dir("cleanup-intact");
        let path = dir.join("settings.json");
        AppSettings::default().save(&path).expect("保存主文件");
        let temp = dir.join("settings.json.tmp");
        std::fs::write(&temp, b"{half").expect("伪造残留临时文件");

        let removed = AppSettings::cleanup_stale_temp(&path);
        assert_eq!(removed.as_deref(), Some(temp.as_path()));
        assert!(!temp.exists());
        assert!(path.exists(), "主文件不能被误删");
        let _ = std::fs::remove_dir_all(&dir); // gate-allow R-C-2: 用例收尾删除独立临时目录，失败不影响断言结论
    }

    /// 主文件损坏或缺失时保留临时文件，避免误删可恢复副本。
    #[test]
    fn cleanup_stale_temp_keeps_when_main_broken() {
        let dir = temp_dir("cleanup-broken");
        let path = dir.join("settings.json");
        let temp = dir.join("settings.json.tmp");

        // 主文件缺失：保留。
        std::fs::write(&temp, b"{half").expect("伪造残留临时文件");
        assert!(AppSettings::cleanup_stale_temp(&path).is_none());
        assert!(temp.exists());

        // 主文件损坏：同样保留。
        std::fs::write(&path, b"{not json").expect("写入坏主文件");
        assert!(AppSettings::cleanup_stale_temp(&path).is_none());
        assert!(temp.exists());
        let _ = std::fs::remove_dir_all(&dir); // gate-allow R-C-2: 用例收尾删除独立临时目录，失败不影响断言结论
    }

    /// 缺失文件回落默认；损坏与超前版本报错。
    #[test]
    fn missing_corrupt_and_future() {
        let dir = temp_dir("corrupt");
        let defaults = AppSettings::load(dir.join("missing.json")).expect("缺失应回落默认");
        assert!(!defaults.allow_relay);

        let bad = dir.join("bad.json");
        std::fs::write(&bad, b"{not json").expect("写入坏文件");
        assert!(AppSettings::load(&bad).is_err());

        let future = dir.join("future.json");
        std::fs::write(&future, br#"{"version":99,"allow_relay":true}"#).expect("写入");
        assert!(matches!(
            AppSettings::load(&future),
            Err(SettingsError::UnsupportedVersion { found: 99, .. })
        ));
        let _ = std::fs::remove_dir_all(&dir); // gate-allow R-C-2: 用例收尾删除独立临时目录，失败不影响断言结论
    }

    /// 非法值被拉回合法区间，VIEW 位不可取消。
    #[test]
    fn sanitize_clamps() {
        let mut settings = AppSettings {
            video_width: 300,
            video_height: 100,
            video_fps: 1,
            file_bandwidth_ratio: 0.95,
            takeover_timeout_s: 5,
            permission_bits: 0,
            audit_retention_days: 12,
            ..AppSettings::default()
        };
        settings.sanitize();
        assert_eq!(settings.video_width, 320);
        assert_eq!(settings.video_height, 180);
        assert_eq!(settings.video_fps, 5);
        assert_eq!(settings.file_bandwidth_ratio, 0.7);
        assert_eq!(settings.takeover_timeout_s, 10);
        assert_eq!(settings.permission_bits & 1, 1);
        assert_eq!(settings.audit_retention_days, 30);
    }

    /// 旧配置缺少首启字段：按安全默认 false 读取，不报错；写入后能往返保留。
    #[test]
    fn legacy_config_without_first_run_field() {
        let dir = temp_dir("legacy-first-run");
        let path = dir.join("settings.json");
        // 模拟只有旧字段、没有 first_run_completed 的配置文件。
        std::fs::write(
            &path,
            br#"{"version":1,"allow_relay":false,"remote_access_enabled":true}"#,
        )
        .expect("写入旧配置");
        let mut loaded = AppSettings::load(&path).expect("旧配置应可读");
        assert!(!loaded.first_run_completed);
        assert!(loaded.remote_access_enabled);

        loaded.first_run_completed = true;
        loaded.save(&path).expect("保存");
        let reloaded = AppSettings::load(&path).expect("重新加载");
        assert!(reloaded.first_run_completed);
        let _ = std::fs::remove_dir_all(&dir); // gate-allow R-C-2: 用例收尾删除独立临时目录，失败不影响断言结论
    }
}
