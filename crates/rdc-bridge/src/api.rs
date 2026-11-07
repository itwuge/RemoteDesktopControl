//! rdc-bridge / 对外 API
//!
//! 层级：L4 桥接层
//! 用途：flutter_rust_bridge 的扫描入口（rust_input: crate::api）；
//!       每个 `pub fn` 会生成一个同名 Dart 函数
//! 调用方：生成的 Dart 绑定，再由 model/gateway/rust_gateway.dart 统一收口

use std::io::Write as _;
use std::path::PathBuf;

use rdc_core::audit::AuditLog;
use rdc_core::s5::devices::{now_ms, DeviceList, DeviceRecord};
use rdc_core::s5::grant::{
    issue_grant, template_bits, GrantRecord, GrantSpec, GrantStore, PermBits, DAY_MS,
    DEFAULT_VALIDITY_MS, MAX_VALIDITY_MS, REVOCATION_UI_TEXT,
};
use rdc_core::s5::multi::{authorize_input, wall_capabilities, SessionRole};
use rdc_core::s5::passwordless::decide_for_record;
use rdc_core::s5::settings::AppSettings;
use rdc_protocol::Capabilities;
use rdc_security::{fingerprint_from_b64, IdentityKeyPair};

use crate::clipboard_service;
use crate::file_service;
use crate::input_service;
use crate::session_service;
use crate::wall_service;

/// 输入事件 DTO（定义见 `input_service`，此处再导出供 flutter_rust_bridge 扫描）。
pub use crate::input_service::{InputEventDto, InputStatsDto};

/// 剪贴板状态 DTO（定义见 `clipboard_service`）。
pub use crate::clipboard_service::ClipboardStatusDto;

/// 文件传输结果 DTO（定义见 `file_service`）。
pub use crate::file_service::FileTransferDto;

/// 会话状态与收尾报告 DTO（定义见 `session_service`）。
pub use crate::session_service::{SessionStatusDto, SessionTeardownDto, TeardownStepDto};

// =====** 版本与平台 =====**

/// 返回 Rust 核心版本号。
///
/// # Returns
/// 形如 `"0.1.0"` 的字符串；Flutter 首页显示在版本卡片上。
///
/// # Errors
/// 不会失败。
pub fn core_version() -> String {
    rdc_core::core_version()
}

/// 返回构建平台（`os-arch`）。
///
/// # Returns
/// 形如 `"windows-x86_64"` 的字符串。
///
/// # Errors
/// 不会失败。
pub fn platform_name() -> String {
    rdc_core::platform_name()
}

// =====** 桥接状态 =====**

/// 探测 FFI 链路是否真正可用。
///
/// # Returns
/// 固定返回 `"OK"`：能执行到本函数即证明 Dart → Rust 调用链已打通。
///
/// # Errors
/// 不会失败。
pub fn bridge_status() -> String {
    // 这是 S1 的最小闭环验证点：Dart 侧拿到 "OK" 即代表动态库装载、符号解析均成功。
    "OK".to_owned()
}

// =====** 自检 =====**

/// 返回 5 行环境自检结果（docs/10 §2.2 第 3 条）。
///
/// # Returns
/// 每条为 `[OK] 名称: 详情` 的字符串数组，顺序固定。
///
/// # Errors
/// 不会失败；单项失败以文本标注，不抛错。
pub fn doctor_report() -> Vec<String> {
    rdc_core::run_doctor().iter().map(|check| check.line()).collect()
}

// =====** 配置路径 =====**

/// 本机配置目录（`%APPDATA%\RDC`），缺失时退回临时目录。
fn config_dir() -> PathBuf {
    if let Ok(appdata) = std::env::var("APPDATA") {
        if !appdata.trim().is_empty() {
            return PathBuf::from(appdata).join("RDC");
        }
    }
    std::env::temp_dir().join("rdc")
}

/// 设备列表文件路径。
fn device_path() -> PathBuf {
    config_dir().join("devices.json")
}

/// 设置文件路径。
fn settings_path() -> PathBuf {
    config_dir().join("settings.json")
}

// =====** 设备列表 DTO =====**

/// 设备条目（跨 FFI 的可复制快照）。
#[derive(Debug, Clone)]
pub struct DeviceDto {
    /// 设备标识。
    pub device_id: String,
    /// 显示名。
    pub name: String,
    /// 最后在线时间（Unix 毫秒，0 表示从未在线）。
    pub last_online_ms: u64,
    /// 是否信任设备。
    pub trusted: bool,
    /// 是否同账号（S5-2 只存字段，不做身份判定）。
    pub same_account: bool,
}

/// 设备列表快照。
#[derive(Debug, Clone)]
pub struct DeviceListDto {
    /// 设备条目，按设备 ID 升序。
    pub devices: Vec<DeviceDto>,
    /// 实际落盘路径（页面用于展示与排障）。
    pub path: String,
}

/// 设备记录转 DTO。
fn to_device_dto(record: &DeviceRecord) -> DeviceDto {
    DeviceDto {
        device_id: record.device_id.clone(),
        name: record.name.clone(),
        last_online_ms: record.last_online_ms,
        trusted: record.trusted,
        same_account: record.same_account,
    }
}

/// 读取设备列表。
///
/// # Returns
/// 设备条目与文件路径；文件不存在时返回空列表。
///
/// # Errors
/// 文件损坏或版本超前时返回错误文本。
pub fn devices_load() -> Result<DeviceListDto, String> {
    let path = device_path();
    let list = DeviceList::load(&path).map_err(|error| error.to_string())?;
    Ok(DeviceListDto {
        devices: list.devices().into_iter().map(to_device_dto).collect(),
        path: path.display().to_string(),
    })
}

/// 保存整份设备列表（用于导入/恢复）。
///
/// # Arguments
/// * `devices` - 完整设备条目
///
/// # Returns
/// 保存后的列表快照。
///
/// # Errors
/// 原子写失败时返回错误文本。
pub fn devices_save(devices: Vec<DeviceDto>) -> Result<DeviceListDto, String> {
    let path = device_path();
    let mut list = DeviceList::load(&path).map_err(|error| error.to_string())?;
    for device in devices {
        list.upsert_online(&device.device_id, &device.name, device.last_online_ms);
        list.set_trusted(&device.device_id, device.trusted);
        list.set_same_account(&device.device_id, device.same_account);
    }
    list.save(&path).map_err(|error| error.to_string())?;
    Ok(DeviceListDto {
        devices: list.devices().into_iter().map(to_device_dto).collect(),
        path: path.display().to_string(),
    })
}

/// 新增设备（以「刚上线」写入，最后在线时间取当前时刻）。
///
/// # Arguments
/// * `device_id` - 设备标识
/// * `name` - 显示名
/// * `trusted` - 是否信任
///
/// # Returns
/// 更新后的列表快照。
///
/// # Errors
/// 读写失败时返回错误文本。
pub fn device_add(
    device_id: String,
    name: String,
    trusted: bool,
) -> Result<DeviceListDto, String> {
    let path = device_path();
    let mut list = DeviceList::load(&path).map_err(|error| error.to_string())?;
    list.upsert_online(&device_id, &name, now_ms());
    list.set_trusted(&device_id, trusted);
    list.save(&path).map_err(|error| error.to_string())?;
    Ok(DeviceListDto {
        devices: list.devices().into_iter().map(to_device_dto).collect(),
        path: path.display().to_string(),
    })
}

/// 更新设备名称与信任/同账号标记（不刷新最后在线时间）。
///
/// # Arguments
/// * `device_id` - 设备标识
/// * `name` - 新显示名
/// * `trusted` - 是否信任
/// * `same_account` - 同账号字段
///
/// # Returns
/// 更新后的列表快照。
///
/// # Errors
/// 设备不存在或读写失败时返回错误文本。
pub fn device_update(
    device_id: String,
    name: String,
    trusted: bool,
    same_account: bool,
) -> Result<DeviceListDto, String> {
    let path = device_path();
    let mut list = DeviceList::load(&path).map_err(|error| error.to_string())?;
    if !list.set_name(&device_id, &name) {
        return Err(format!("设备不存在或名称为空：{device_id}"));
    }
    list.set_trusted(&device_id, trusted);
    list.set_same_account(&device_id, same_account);
    list.save(&path).map_err(|error| error.to_string())?;
    Ok(DeviceListDto {
        devices: list.devices().into_iter().map(to_device_dto).collect(),
        path: path.display().to_string(),
    })
}

/// 删除设备。
///
/// # Arguments
/// * `device_id` - 设备标识
///
/// # Returns
/// 删除后的列表快照。
///
/// # Errors
/// 读写失败时返回错误文本（设备不存在视为已删除，不报错）。
pub fn device_remove(device_id: String) -> Result<DeviceListDto, String> {
    let path = device_path();
    let mut list = DeviceList::load(&path).map_err(|error| error.to_string())?;
    list.remove(&device_id);
    list.save(&path).map_err(|error| error.to_string())?;
    Ok(DeviceListDto {
        devices: list.devices().into_iter().map(to_device_dto).collect(),
        path: path.display().to_string(),
    })
}

// =====** 设置 DTO =====**

/// 设置快照（字段名与 Dart `AppSettings` 一致）。
#[derive(Debug, Clone)]
pub struct SettingsDto {
    /// 是否允许中继。
    pub allow_relay: bool,
    /// 中继地址。
    pub relay_address: String,
    /// 打洞失败策略。
    pub connection_policy: String,
    /// 屏幕方向策略。
    pub orientation: String,
    /// 文件带宽占比。
    pub file_bandwidth_ratio: f64,
    /// 主码流宽。
    pub video_width: u32,
    /// 主码流高。
    pub video_height: u32,
    /// 主码流帧率。
    pub video_fps: u32,
    /// 主码流码率（Mbps）。
    pub video_bitrate_mbps: f64,
    /// 断线自动重连。
    pub auto_reconnect: bool,
    /// 允许远程访问本机。
    pub remote_access_enabled: bool,
    /// 并发模式。
    pub concurrent_mode: String,
    /// 接管策略。
    pub takeover_policy: String,
    /// 本机确认超时（秒）。
    pub takeover_timeout_s: u32,
    /// 权限模板。
    pub permission_template: String,
    /// 权限位图。
    pub permission_bits: u32,
    /// 本机常驻提示。
    pub local_notice: bool,
    /// 审计保留天数。
    pub audit_retention_days: u32,
    /// 本地配对策略。
    pub local_pair_policy: String,
    /// 同账号配对策略。
    pub same_account_policy: String,
    /// 免密策略。
    pub passwordless_policy: bool,
    /// 实际落盘路径。
    pub path: String,
}

/// 设置转 DTO。
fn to_settings_dto(settings: AppSettings, path: PathBuf) -> SettingsDto {
    SettingsDto {
        allow_relay: settings.allow_relay,
        relay_address: settings.relay_address,
        connection_policy: settings.connection_policy,
        orientation: settings.orientation,
        file_bandwidth_ratio: settings.file_bandwidth_ratio,
        video_width: settings.video_width,
        video_height: settings.video_height,
        video_fps: settings.video_fps,
        video_bitrate_mbps: settings.video_bitrate_mbps,
        auto_reconnect: settings.auto_reconnect,
        remote_access_enabled: settings.remote_access_enabled,
        concurrent_mode: settings.concurrent_mode,
        takeover_policy: settings.takeover_policy,
        takeover_timeout_s: settings.takeover_timeout_s,
        permission_template: settings.permission_template,
        permission_bits: settings.permission_bits,
        local_notice: settings.local_notice,
        audit_retention_days: settings.audit_retention_days,
        local_pair_policy: settings.local_pair_policy,
        same_account_policy: settings.same_account_policy,
        passwordless_policy: settings.passwordless_policy,
        path: path.display().to_string(),
    }
}

/// DTO 转设置。
fn from_settings_dto(dto: SettingsDto) -> AppSettings {
    AppSettings {
        allow_relay: dto.allow_relay,
        relay_address: dto.relay_address,
        connection_policy: dto.connection_policy,
        orientation: dto.orientation,
        file_bandwidth_ratio: dto.file_bandwidth_ratio,
        video_width: dto.video_width,
        video_height: dto.video_height,
        video_fps: dto.video_fps,
        video_bitrate_mbps: dto.video_bitrate_mbps,
        auto_reconnect: dto.auto_reconnect,
        remote_access_enabled: dto.remote_access_enabled,
        concurrent_mode: dto.concurrent_mode,
        takeover_policy: dto.takeover_policy,
        takeover_timeout_s: dto.takeover_timeout_s,
        permission_template: dto.permission_template,
        permission_bits: dto.permission_bits,
        local_notice: dto.local_notice,
        audit_retention_days: dto.audit_retention_days,
        local_pair_policy: dto.local_pair_policy,
        same_account_policy: dto.same_account_policy,
        passwordless_policy: dto.passwordless_policy,
    }
}

/// 读取设置。
///
/// # Returns
/// 设置快照；文件缺失时返回安全默认值。
///
/// # Errors
/// 文件损坏或版本超前时返回错误文本。
pub fn settings_load() -> Result<SettingsDto, String> {
    let path = settings_path();
    let settings = AppSettings::load(&path).map_err(|error| error.to_string())?;
    Ok(to_settings_dto(settings, path))
}

/// 原子保存设置。
///
/// # Arguments
/// * `settings` - 完整设置快照
///
/// # Returns
/// 保存后的设置（已净化）。
///
/// # Errors
/// 写入失败时返回错误文本。
pub fn settings_save(settings: SettingsDto) -> Result<SettingsDto, String> {
    let path = settings_path();
    let settings = from_settings_dto(settings);
    settings.save(&path).map_err(|error| error.to_string())?;
    let mut reloaded = AppSettings::load(&path).map_err(|error| error.to_string())?;
    reloaded.sanitize();
    Ok(to_settings_dto(reloaded, path))
}

// =====** 会话角色与能力 =====**

/// 会话角色信息。
#[derive(Debug, Clone)]
pub struct RoleInfoDto {
    /// 角色字符串：`viewer` / `controller`。
    pub role: String,
    /// 是否可注入输入。
    pub can_inject_input: bool,
    /// 是否可拉屏幕墙子码流。
    pub can_view_wall: bool,
}

/// 解析角色（未知字符串统一按观看者处理，最小权限）。
fn parse_role(role: &str) -> SessionRole {
    match role {
        "controller" => SessionRole::Controller,
        _ => SessionRole::Viewer,
    }
}

/// 查询角色权限。
///
/// # Arguments
/// * `role` - `viewer` 或 `controller`
///
/// # Returns
/// 角色、可注入输入、可看墙三项。
///
/// # Errors
/// 不会失败。
pub fn role_info(role: String) -> RoleInfoDto {
    let role = parse_role(&role);
    RoleInfoDto {
        role: role.as_str().to_owned(),
        can_inject_input: role.can_inject_input(),
        can_view_wall: role.can_view_wall(),
    }
}

/// 判定某角色能否注入输入（授权模块的唯一入口）。
///
/// # Arguments
/// * `role` - `viewer` 或 `controller`
///
/// # Returns
/// 允许返回 `true`；观看者恒为 `false`。
///
/// # Errors
/// 不会失败。
pub fn input_allowed(role: String) -> bool {
    authorize_input(parse_role(&role)).allowed
}

/// 屏幕墙所需的能力位掩码。
///
/// # Returns
/// bit7 子码流 + bit10 多会话 + bit11 屏幕墙 的组合值。
///
/// # Errors
/// 不会失败。
pub fn wall_capability_bits() -> u64 {
    let capabilities = wall_capabilities(Capabilities::DEFAULT);
    capabilities.0
}

// =====** 屏幕墙预览 =====**

/// 屏幕墙服务状态。
#[derive(Debug, Clone)]
pub struct WallStatusDto {
    /// 是否运行。
    pub running: bool,
    /// 缩略编码器后端。
    pub backend: String,
    /// 采集后端（如 `dxgi-desktop-duplication`）。
    pub capture_backend: String,
    /// 实测帧率（1 秒窗口）。
    pub fps: f64,
    /// 已编码帧数。
    pub encoded_frames: u64,
    /// 已解码帧数。
    pub decoded_frames: u64,
    /// 重组丢弃帧数。
    pub dropped_frames: u64,
    /// 预览宽。
    pub width: u32,
    /// 预览高。
    pub height: u32,
    /// 最近端到端延迟（微秒）。
    pub latency_us: u64,
    /// 最近帧时间戳（微秒）。
    pub latest_pts_us: u64,
    /// 最近错误。
    pub last_error: Option<String>,
}

/// 一帧预览。
#[derive(Debug, Clone)]
pub struct WallPreviewDto {
    /// 流编号。
    pub stream_id: u32,
    /// 帧号。
    pub frame_id: u32,
    /// 帧时间戳（微秒）。
    pub pts_us: u64,
    /// 宽。
    pub width: u32,
    /// 高。
    pub height: u32,
    /// 端到端延迟（微秒）。
    pub latency_us: u64,
    /// RGBA 像素。
    pub bytes: Vec<u8>,
}

/// 启动本机回环屏幕墙预览（1 路真实缩略流）。
///
/// # Returns
/// 启动后的状态。
///
/// # Errors
/// 编码器/解码器创建失败时返回错误文本。
pub fn wall_start() -> Result<WallStatusDto, String> {
    wall_service::start().map(to_wall_status)
}

/// 停止预览并等待线程退出。
///
/// # Returns
/// 停止后的状态。
///
/// # Errors
/// 内部锁异常时返回错误文本。
pub fn wall_stop() -> Result<WallStatusDto, String> {
    wall_service::stop().map(to_wall_status)
}

/// 读取预览状态。
///
/// # Returns
/// 状态快照。
///
/// # Errors
/// 内部锁异常时返回错误文本。
pub fn wall_status() -> Result<WallStatusDto, String> {
    wall_service::status().map(to_wall_status)
}

/// 读取最新预览帧。
///
/// # Returns
/// 尚无帧时返回 `None`。
///
/// # Errors
/// 内部锁异常时返回错误文本。
pub fn wall_latest() -> Result<Option<WallPreviewDto>, String> {
    wall_service::latest().map(|frame| {
        frame.map(|frame| WallPreviewDto {
            stream_id: frame.stream_id,
            frame_id: frame.frame_id,
            pts_us: frame.pts_us,
            width: frame.width,
            height: frame.height,
            latency_us: frame.latency_us,
            bytes: frame.bytes,
        })
    })
}

/// 快照转 DTO。
fn to_wall_status(snapshot: wall_service::WallSnapshot) -> WallStatusDto {
    WallStatusDto {
        running: snapshot.running,
        backend: snapshot.backend,
        capture_backend: snapshot.capture_backend,
        fps: snapshot.fps,
        encoded_frames: snapshot.encoded_frames,
        decoded_frames: snapshot.decoded_frames,
        dropped_frames: snapshot.dropped_frames,
        width: snapshot.width,
        height: snapshot.height,
        latency_us: snapshot.latency_us,
        latest_pts_us: snapshot.latest_pts_us,
        last_error: snapshot.last_error,
    }
}

// =====** 控制页真实画面（外部纹理） =====**

/// 控制页预览状态。
///
/// 非 Windows 平台（Linux/macOS）的 `control_start`/`control_stop`/`control_status` 分支
/// 使用 `Default` 表达「预览不可用」的空状态，因此这里显式派生 `Default`。
#[derive(Debug, Clone, Default)]
pub struct ControlStatusDto {
    /// 是否运行。
    pub running: bool,
    /// 采集后端。
    pub backend: String,
    /// 编码器名称。
    pub encoder: String,
    /// 解码器名称。
    pub decoder: String,
    /// 上屏通道（`dxgi-shared-texture`）。
    pub texture_path: String,
    /// 解码输出路径（`dxgi-texture` / `system-memory`）。
    pub decode_path: String,
    /// 画面宽。
    pub width: u32,
    /// 画面高。
    pub height: u32,
    /// 实测帧率。
    pub fps: f64,
    /// 采集帧数。
    pub captured_frames: u64,
    /// 编码包数。
    pub encoded_frames: u64,
    /// 解码帧数。
    pub decoded_frames: u64,
    /// 采集丢帧数。
    pub dropped_frames: u64,
    /// 编码输出累计字节数。
    pub encoded_bytes: u64,
    /// 实测码率（千比特/秒）。
    pub bitrate_kbps: u64,
    /// 最近端到端延迟（微秒）。
    pub latency_last_us: u64,
    /// 延迟 p50（微秒）。
    pub latency_p50_us: u64,
    /// 延迟 p99（微秒）。
    pub latency_p99_us: u64,
    /// 时间码回读次数。
    pub readback_count: u64,
    /// 共享纹理句柄 A。
    pub share_handle_a: i64,
    /// 共享纹理句柄 B。
    pub share_handle_b: i64,
    /// 新帧事件 A。
    pub event_a: i64,
    /// 新帧事件 B。
    pub event_b: i64,
    /// 最近错误。
    pub last_error: Option<String>,
}

/// 启动控制页真实画面预览。
///
/// # Returns
/// 启动后的状态（含共享纹理句柄与事件句柄，供 runner 注册外部纹理）。
///
/// # Errors
/// 非 Windows 平台或采集/编解码资源创建失败时返回错误文本。
pub fn control_start() -> Result<ControlStatusDto, String> {
    #[cfg(windows)]
    {
        crate::control_service::start().map(to_control_status)
    }
    #[cfg(target_os = "linux")]
    {
        crate::control_service_linux::start().map(to_control_status_linux)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        Err("控制页实时画面前仅支持 Windows / Linux 桌面".to_owned())
    }
}

/// 停止控制页预览。
///
/// # Returns
/// 停止后的状态。
///
/// # Errors
/// 内部锁异常时返回错误文本。
pub fn control_stop() -> Result<ControlStatusDto, String> {
    #[cfg(windows)]
    {
        crate::control_service::stop().map(to_control_status)
    }
    #[cfg(target_os = "linux")]
    {
        crate::control_service_linux::stop().map(to_control_status_linux)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        Ok(ControlStatusDto::default())
    }
}

/// 读取控制页预览状态。
///
/// # Returns
/// 实时状态快照。
///
/// # Errors
/// 内部锁异常时返回错误文本。
pub fn control_status() -> Result<ControlStatusDto, String> {
    #[cfg(windows)]
    {
        crate::control_service::status().map(to_control_status)
    }
    #[cfg(target_os = "linux")]
    {
        crate::control_service_linux::status().map(to_control_status_linux)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        Ok(ControlStatusDto::default())
    }
}

/// 控制页最新一帧 RGBA 画面（S8-3 Linux 降级上屏通道）。
///
/// Windows 走外部纹理，不使用本接口，恒返回 `None`。
/// Linux 上 `control_start` 成功后由解码线程更新，Dart 侧按 15~30Hz 轮询。
///
/// # Returns
/// 已有解码帧返回 `Some`；尚未出首帧或未启动返回 `None`。
///
/// # Errors
/// 内部锁异常或平台未实现时返回错误文本。
pub fn control_frame() -> Result<Option<ControlFrameDto>, String> {
    #[cfg(target_os = "linux")]
    {
        crate::control_service_linux::latest_frame().map(|frame| {
            frame.map(|frame| ControlFrameDto {
                frame_id: frame.frame_id,
                width: frame.width,
                height: frame.height,
                pts_us: frame.pts_us,
                latency_us: frame.latency_us,
                bytes: frame.bytes,
            })
        })
    }
    #[cfg(not(target_os = "linux"))]
    {
        Ok(None)
    }
}

/// 一帧 RGBA 画面。
///
/// `bytes` 为 RGBA8888，长度必须为 `width * height * 4`。
#[derive(Debug, Clone)]
pub struct ControlFrameDto {
    /// 帧序号（单调递增，Dart 侧据此跳过重复帧）。
    pub frame_id: u64,
    /// 宽（像素）。
    pub width: u32,
    /// 高（像素）。
    pub height: u32,
    /// 采集时间戳（微秒，单调时钟）。
    pub pts_us: u64,
    /// 端到端延迟（微秒）。
    pub latency_us: u64,
    /// RGBA8888 像素。
    pub bytes: Vec<u8>,
}

/// 控制快照转 DTO。
#[cfg(windows)]
fn to_control_status(snapshot: crate::control_service::ControlSnapshot) -> ControlStatusDto {
    ControlStatusDto {
        running: snapshot.running,
        backend: snapshot.backend,
        encoder: snapshot.encoder,
        decoder: snapshot.decoder,
        texture_path: snapshot.texture_path,
        decode_path: snapshot.decode_path,
        width: snapshot.width,
        height: snapshot.height,
        fps: snapshot.fps,
        captured_frames: snapshot.captured_frames,
        encoded_frames: snapshot.encoded_frames,
        decoded_frames: snapshot.decoded_frames,
        dropped_frames: snapshot.dropped_frames,
        encoded_bytes: snapshot.encoded_bytes,
        bitrate_kbps: snapshot.bitrate_kbps,
        latency_last_us: snapshot.latency_last_us,
        latency_p50_us: snapshot.latency_p50_us,
        latency_p99_us: snapshot.latency_p99_us,
        readback_count: snapshot.readback_count,
        share_handle_a: snapshot.share_handle_a,
        share_handle_b: snapshot.share_handle_b,
        event_a: snapshot.event_a,
        event_b: snapshot.event_b,
        last_error: snapshot.last_error,
    }
}

/// 控制快照转 DTO（Linux：外部纹理句柄不适用，恒为 0，画面走 `control_frame`）。
#[cfg(target_os = "linux")]
fn to_control_status_linux(
    snapshot: crate::control_service_linux::ControlSnapshot,
) -> ControlStatusDto {
    ControlStatusDto {
        running: snapshot.running,
        backend: snapshot.backend,
        encoder: snapshot.encoder,
        decoder: snapshot.decoder,
        texture_path: snapshot.texture_path,
        decode_path: snapshot.decode_path,
        width: snapshot.width,
        height: snapshot.height,
        fps: snapshot.fps,
        captured_frames: snapshot.captured_frames,
        encoded_frames: snapshot.encoded_frames,
        decoded_frames: snapshot.decoded_frames,
        dropped_frames: snapshot.dropped_frames,
        encoded_bytes: snapshot.encoded_bytes,
        bitrate_kbps: snapshot.bitrate_kbps,
        latency_last_us: snapshot.latency_last_us,
        latency_p50_us: snapshot.latency_p50_us,
        latency_p99_us: snapshot.latency_p99_us,
        readback_count: snapshot.readback_count,
        share_handle_a: snapshot.share_handle_a,
        share_handle_b: snapshot.share_handle_b,
        event_a: snapshot.event_a,
        event_b: snapshot.event_b,
        last_error: (!snapshot.last_error.is_empty()).then_some(snapshot.last_error),
    }
}

// =====** 控制页输入注入（S5-4 块 1） =====**

/// 启动本机回环输入通道（与 `control_start` 配对调用）。
///
/// # Arguments
/// * `desktop_width` - 被控端桌面物理宽（来自控制页状态）
/// * `desktop_height` - 被控端桌面物理高
///
/// # Returns
/// 启动后的统计快照。
///
/// # Errors
/// 平台注入器创建失败时返回错误文本。
pub fn input_start(desktop_width: u32, desktop_height: u32) -> Result<InputStatsDto, String> {
    input_service::start(desktop_width, desktop_height)
}

/// 推送一批输入事件；坐标已由 Dart 侧 `ViewportController` 映射为桌面物理像素。
///
/// # Arguments
/// * `events` - 事件数组，注入顺序与数组顺序一致
///
/// # Returns
/// 处理后的统计快照。
///
/// # Errors
/// 通道未启动或事件非法时返回错误文本。
pub fn input_push(events: Vec<InputEventDto>) -> Result<InputStatsDto, String> {
    input_service::push(events)
}

/// 冲刷合并器中挂起的移动（指针抬起后调用，避免最后一个点丢失）。
///
/// # Returns
/// 处理后的统计快照。
///
/// # Errors
/// 通道未启动时返回错误文本。
pub fn input_flush() -> Result<InputStatsDto, String> {
    input_service::flush()
}

/// 停止输入通道并抬起所有仍按下的键/钮。
///
/// # Returns
/// 收尾后的统计快照（`keys_down` 必须为 0）。
///
/// # Errors
/// 不会失败。
pub fn input_stop() -> Result<InputStatsDto, String> {
    input_service::stop()
}

/// 读取输入统计。
///
/// # Returns
/// 运行中为实时快照，停止后为最近一次收尾快照。
///
/// # Errors
/// 不会失败。
pub fn input_status() -> Result<InputStatsDto, String> {
    input_service::status()
}

// =====** 剪贴板与文件（S5-4 块 2） =====**

/// 启动剪贴板同步（回环两端各一个状态机）。
///
/// # Returns
/// 启动后的状态快照。
///
/// # Errors
/// 平台剪贴板不可用时返回错误文本。
pub fn clipboard_start() -> Result<ClipboardStatusDto, String> {
    clipboard_service::start()
}

/// 把本机剪贴板文本发送到远端（模拟对端写入其剪贴板）。
///
/// # Returns
/// 同步后的状态快照。
///
/// # Errors
/// 通道未启动或本机剪贴板为空时返回错误文本。
pub fn clipboard_send() -> Result<ClipboardStatusDto, String> {
    clipboard_service::send_local()
}

/// 把远端内容取回本机剪贴板，并回读校验。
///
/// # Returns
/// 同步后的状态快照（`verified` 表示回读一致）。
///
/// # Errors
/// 通道未启动、远端为空或写入失败时返回错误文本。
pub fn clipboard_pull() -> Result<ClipboardStatusDto, String> {
    clipboard_service::pull_remote()
}

/// 模拟对端修改剪贴板（用于验证远端 → 本机方向）。
///
/// # Arguments
/// * `text` - 对端新的剪贴板文本
///
/// # Returns
/// 更新后的状态快照。
///
/// # Errors
/// 通道未启动时返回错误文本。
pub fn clipboard_set_remote(text: String) -> Result<ClipboardStatusDto, String> {
    clipboard_service::set_remote_text(text)
}

/// 读取远端缓冲区内容（UI 展示用，不写回本机）。
///
/// # Returns
/// 远端文本。
///
/// # Errors
/// 通道未启动时返回错误文本。
pub fn clipboard_remote_text() -> Result<String, String> {
    clipboard_service::remote_text()
}

/// 读取本机剪贴板文本（UI 预览 / 验证）。
///
/// # Returns
/// 本机剪贴板文本；无文本时为空串。
///
/// # Errors
/// 通道未启动或读取失败时返回错误文本。
pub fn clipboard_local_text() -> Result<String, String> {
    clipboard_service::local_text()
}

/// 停止剪贴板同步并释放平台剪贴板句柄。
///
/// # Returns
/// 收尾快照（`running=false`）。
///
/// # Errors
/// 不会失败。
pub fn clipboard_stop() -> Result<ClipboardStatusDto, String> {
    clipboard_service::stop()
}

/// 读取剪贴板同步状态。
///
/// # Returns
/// 运行中为实时快照，停止后为空快照。
///
/// # Errors
/// 不会失败。
pub fn clipboard_status() -> Result<ClipboardStatusDto, String> {
    clipboard_service::status()
}

/// 传输一个文件到接收目录，并校验两端 SHA-256。
///
/// # Arguments
/// * `path` - 源文件绝对路径
/// * `interrupt_after_chunks` - 大于 0 时在该块后中断并续传（验证断点续传）
/// * `chunk_size` - 分块大小，`0` 为默认 4 MiB
///
/// # Returns
/// 两端哈希、字节数、续传信息。
///
/// # Errors
/// 源文件不存在或校验失败时返回错误文本。
pub fn file_transfer(
    path: String,
    interrupt_after_chunks: u32,
    chunk_size: u64,
) -> Result<FileTransferDto, String> {
    file_service::send(path, interrupt_after_chunks, chunk_size)
}

// =====** 会话收尾（S5-4 块 3） =====**

/// 建立会话链路（回环阶段为进程内 UDP 回环桩）。
///
/// # Arguments
/// * `role` - 控制端角色名（controller / viewer）
///
/// # Returns
/// 会话状态（含控制端与被控端回环端口）。
///
/// # Errors
/// 套接字绑定失败或锁异常时返回错误文本。
pub fn session_start(role: String) -> Result<SessionStatusDto, String> {
    session_service::start(&role)
}

/// 读取会话状态。
///
/// # Returns
/// 未建立会话时返回 `running=false` 的快照。
///
/// # Errors
/// 锁异常时返回错误文本。
pub fn session_status() -> Result<SessionStatusDto, String> {
    session_service::status()
}

/// 断开会话：发 `bye`、等回执、按序停服务并核查句柄与端口。
///
/// # Arguments
/// * `reason` - 结束原因（写入 bye 报文，便于对端审计）
///
/// # Returns
/// 收尾报告（`ok` 为真表示 bye 往返成功、服务全停、端口释放、句柄无残留）。
///
/// # Errors
/// 锁异常时返回错误文本；bye 失败只记录在报告中。
pub fn session_disconnect(reason: String) -> Result<SessionTeardownDto, String> {
    session_service::disconnect(&reason)
}

// =====** S5-5 授权与权限页 =====**

/// 授权条目 DTO（跨 FFI）。
#[derive(Debug, Clone)]
pub struct GrantDto {
    /// 授权 ID。
    pub grant_id: String,
    /// 被授权方公钥（Base64）。
    pub grantee_pub: String,
    /// 被授权方显示名。
    pub grantee_name: String,
    /// 被授权方公钥指纹（冒号分隔）。
    pub grantee_fingerprint: String,
    /// 模板名：`readonly` / `operate` / `full` / `custom`。
    pub template: String,
    /// 令牌权限位（u16）。
    pub perms: u32,
    /// 本机生效权限位（只减不增）。
    pub effective_perms: u32,
    /// 生效时间（Unix 毫秒）。
    pub not_before_ms: u64,
    /// 到期时间（Unix 毫秒）。
    pub expires_at_ms: u64,
    /// 是否已吊销。
    pub revoked: bool,
    /// 创建时间。
    pub created_ms: u64,
    /// 最后修改时间。
    pub updated_ms: u64,
}

/// 信任设备 DTO（带公钥指纹与免密可用性）。
#[derive(Debug, Clone)]
pub struct TrustDeviceDto {
    /// 设备 ID。
    pub device_id: String,
    /// 显示名。
    pub name: String,
    /// 是否信任。
    pub trusted: bool,
    /// 是否同账号。
    pub same_account: bool,
    /// 公钥指纹；未采集公钥时为空。
    pub fingerprint: String,
    /// 是否已具备免密条件（信任 + 有公钥）。
    pub passwordless_ready: bool,
    /// 本次实际会走的认证方式：`passwordless` / `pairing_code`。
    pub auth_mode: String,
    /// 最后在线（Unix 毫秒）。
    pub last_online_ms: u64,
}

/// 审计条目 DTO。
#[derive(Debug, Clone)]
pub struct AuditEntryDto {
    /// Unix 毫秒。
    pub ts_ms: u64,
    /// 事件类别。
    pub kind: String,
    /// 主体。
    pub subject: String,
    /// 动作。
    pub action: String,
    /// 旧值。
    pub old_value: String,
    /// 新值。
    pub new_value: String,
    /// 结果。
    pub outcome: String,
}

/// 权限页快照（真实内核数据）。
#[derive(Debug, Clone)]
pub struct PermissionSnapshotDto {
    /// 远程访问总开关。
    pub remote_access_enabled: bool,
    /// 是否处于「已停止，恢复需二次确认」状态。
    pub resume_requires_confirm: bool,
    /// 并发模式。
    pub concurrent_mode: String,
    /// 接管策略。
    pub takeover_policy: String,
    /// 本机确认超时（秒）。
    pub takeover_timeout_s: u32,
    /// 本机允许权限位（u16）。
    pub local_perms: u32,
    /// 本次运行可授予的最大权限位（u16，含敏感位）。
    pub max_grant_perms: u32,
    /// 未吊销授权数。
    pub active_grants: u32,
    /// 授权列表（含已吊销，按 ID 升序）。
    pub grants: Vec<GrantDto>,
    /// 撤销世代。
    pub revoked_epoch: u64,
    /// 已信任设备。
    pub trust_devices: Vec<TrustDeviceDto>,
    /// 当前是否有远程会话。
    pub session_running: bool,
    /// 当前会话角色（`controller` / `viewer` / 空）。
    pub session_role: String,
    /// 「谁在连接」展示文案。
    pub connected_label: String,
    /// 撤销最终一致文案（不得含「立即生效」）。
    pub revocation_note: String,
    /// 最近审计（新的在后，最多 20 条）。
    pub audit_tail: Vec<AuditEntryDto>,
    /// 授权存储路径。
    pub grants_path: String,
    /// 本机身份公钥指纹。
    pub identity_fingerprint: String,
    /// 非致命提示（如「尚无信任设备」）。
    pub hint: String,
}

/// 立即停止远程访问报告。
#[derive(Debug, Clone)]
pub struct RemoteStopDto {
    /// 输入通道是否已停。
    pub input_stopped: bool,
    /// 会话是否已断。
    pub session_stopped: bool,
    /// 收尾报告是否通过。
    pub teardown_ok: bool,
    /// 收尾摘要。
    pub teardown_summary: String,
    /// 审计是否落盘。
    pub audit_written: bool,
    /// 恢复是否需要本机二次确认。
    pub resume_requires_confirm: bool,
    /// 停止时刻（Unix 毫秒）。
    pub stopped_at_ms: u64,
    /// 停止后的权限页快照。
    pub snapshot: PermissionSnapshotDto,
}

// =====** S5-5 路径与身份 =====**

/// 授权存储文件路径。
fn grants_path() -> PathBuf {
    config_dir().join("grants.json")
}

/// 本机身份种子路径（仅本用户可读）。
fn identity_path() -> PathBuf {
    config_dir().join("identity.seed")
}

/// 审计 JSONL 路径。
fn audit_path() -> PathBuf {
    config_dir().join("s5-audit.jsonl")
}

/// 「已停止远程访问」闸门文件；存在即要求本机二次确认才能恢复。
fn resume_gate_path() -> PathBuf {
    config_dir().join("remote_access_stop.json")
}

/// 十六进制编码（小写）。
fn hex_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// 十六进制解码（允许空白）。
fn hex_decode(text: &str) -> Result<Vec<u8>, String> {
    let clean: String = text.chars().filter(|ch| !ch.is_whitespace()).collect();
    if clean.len() % 2 != 0 {
        return Err("十六进制长度为奇数".to_owned());
    }
    let mut bytes = Vec::with_capacity(clean.len() / 2);
    let chars: Vec<char> = clean.chars().collect();
    for pair in chars.chunks(2) {
        let hi = pair[0].to_digit(16).ok_or_else(|| "非十六进制字符".to_owned())?;
        let lo = pair[1].to_digit(16).ok_or_else(|| "非十六进制字符".to_owned())?;
        bytes.push(((hi << 4) | lo) as u8);
    }
    Ok(bytes)
}

/// 读取或创建本机身份密钥。
///
/// 为什么落盘：授权令牌的 owner 公钥必须跨进程稳定，否则签名与地址簿全部失效。
fn load_or_create_identity() -> Result<IdentityKeyPair, String> {
    let path = identity_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => {
            let bytes = hex_decode(text.trim())?;
            let seed: [u8; 32] = bytes
                .as_slice()
                .try_into()
                .map_err(|_| "identity.seed 必须是 32 字节（64 位十六进制）".to_owned())?;
            Ok(IdentityKeyPair::from_seed(&seed))
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            let identity = IdentityKeyPair::generate();
            let mut file = std::fs::File::create(&path).map_err(|error| error.to_string())?;
            file.write_all(hex_encode(&identity.to_seed_bytes()).as_bytes())
                .map_err(|error| error.to_string())?;
            file.sync_all().map_err(|error| error.to_string())?;
            Ok(identity)
        }
        Err(err) => Err(format!("读取 identity.seed 失败：{err}")),
    }
}

/// 安全计算公钥指纹（非法公钥返回空串，不中断页面）。
fn safe_fingerprint(public_key_b64: &str) -> String {
    if public_key_b64.is_empty() {
        return String::new();
    }
    fingerprint_from_b64(public_key_b64).unwrap_or_default()
}

/// 读取授权存储。
fn load_grants() -> Result<GrantStore, String> {
    GrantStore::load(grants_path()).map_err(|error| error.to_string())
}

/// 保存授权存储。
fn save_grants(store: &GrantStore) -> Result<(), String> {
    store.save(grants_path()).map_err(|error| error.to_string())
}

/// 追加审计 JSONL。
fn append_audit(log: &AuditLog) -> Result<(), String> {
    if log.is_empty() {
        return Ok(());
    }
    let path = audit_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|error| error.to_string())?;
    file.write_all(log.to_jsonl().as_bytes())
        .map_err(|error| error.to_string())?;
    file.write_all(b"\n").map_err(|error| error.to_string())?;
    Ok(())
}

/// 读取最近审计（文件尾部）。
fn read_audit_tail(limit: usize) -> Vec<AuditEntryDto> {
    let Ok(text) = std::fs::read_to_string(audit_path()) else {
        return Vec::new();
    };
    let lines: Vec<&str> = text.lines().filter(|line| !line.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(limit);
    lines[start..]
        .iter()
        .filter_map(|line| {
            let value: serde_json::Value = serde_json::from_str(line).ok()?;
            Some(AuditEntryDto {
                ts_ms: value.get("ts_ms")?.as_u64()?,
                kind: value.get("kind")?.as_str()?.to_owned(),
                subject: value.get("subject")?.as_str()?.to_owned(),
                action: value.get("action")?.as_str()?.to_owned(),
                old_value: value
                    .get("old_value")
                    .and_then(|item| item.as_str())
                    .unwrap_or_default()
                    .to_owned(),
                new_value: value
                    .get("new_value")
                    .and_then(|item| item.as_str())
                    .unwrap_or_default()
                    .to_owned(),
                outcome: value.get("outcome")?.as_str()?.to_owned(),
            })
        })
        .collect()
}

/// 由权限位推断模板名。
fn template_of(perms: u16) -> String {
    let bits = PermBits::new(perms);
    let readonly = PermBits(PermBits::VIEW);
    let operate = PermBits(PermBits::VIEW | PermBits::CONTROL | PermBits::CLIPBOARD_TEXT);
    let full = PermBits(PermBits::SAFE_FULL);
    if bits == readonly {
        "readonly".to_owned()
    } else if bits == operate {
        "operate".to_owned()
    } else if bits == full {
        "full".to_owned()
    } else {
        "custom".to_owned()
    }
}

/// 授权记录转 DTO。
fn to_grant_dto(record: &GrantRecord) -> GrantDto {
    GrantDto {
        grant_id: record.token.grant_id.clone(),
        grantee_pub: record.token.grantee_pub.clone(),
        grantee_name: record.grantee_name.clone(),
        grantee_fingerprint: safe_fingerprint(&record.token.grantee_pub),
        template: template_of(record.token.perms),
        perms: u32::from(record.token.perms),
        effective_perms: u32::from(record.local_perms().to_bits()),
        not_before_ms: record.token.not_before,
        expires_at_ms: record.token.expires_at,
        revoked: record.revoked,
        created_ms: record.created_ms,
        updated_ms: record.updated_ms,
    }
}

/// 构建设备 DTO（带认证方式）。
fn to_trust_dto(record: &DeviceRecord, passwordless_enabled: bool) -> TrustDeviceDto {
    // 认证方式统一由内核判定，避免页面与握手侧规则漂移。
    let decision = decide_for_record(
        Some(record),
        &record.public_key_b64,
        true,
        passwordless_enabled,
    )
    .unwrap_or_else(|_| rdc_core::s5::passwordless::PairingDecision {
        mode: rdc_core::s5::passwordless::PairingMode::PairingCode,
        requires_pairing_code: true,
        used_local_fallback: false,
        reason: "public_key_mismatch",
    });
    TrustDeviceDto {
        device_id: record.device_id.clone(),
        name: record.name.clone(),
        trusted: record.trusted,
        same_account: record.same_account,
        fingerprint: safe_fingerprint(&record.public_key_b64),
        // 「可免密」在页面上表示「有可验签的公钥」，与免密开关无关；
        // 实际本次走哪种方式由 auth_mode 表达。
        passwordless_ready: record.trusted && !record.public_key_b64.is_empty(),
        auth_mode: decision.mode.as_str().to_owned(),
        last_online_ms: record.last_online_ms,
    }
}

/// 组装权限页快照。
fn build_snapshot() -> Result<PermissionSnapshotDto, String> {
    let settings = AppSettings::load(settings_path()).map_err(|error| error.to_string())?;
    let identity = load_or_create_identity()?;
    let store = load_grants()?;
    let devices = DeviceList::load(device_path()).map_err(|error| error.to_string())?;
    let status = session_service::status().unwrap_or_default();

    let grants: Vec<GrantDto> = store.grants().iter().map(|record| to_grant_dto(record)).collect();
    let trust_devices: Vec<TrustDeviceDto> = devices
        .devices()
        .iter()
        .filter(|record| record.trusted)
        .map(|record| to_trust_dto(record, settings.passwordless_policy))
        .collect();

    let connected_label = if status.running {
        let role_zh = if status.role == "controller" {
            "控制端"
        } else {
            "观看端"
        };
        format!("正在连接：{role_zh}（{}）", status.session_id)
    } else {
        "无远程会话".to_owned()
    };
    let hint = if trust_devices.is_empty() {
        "尚无信任设备：先在设备列表配对并标记信任，才能签发授权。".to_owned()
    } else if trust_devices.iter().all(|device| device.fingerprint.is_empty()) {
        "信任设备尚未采集公钥：需要重新配对一次才能免密直连。".to_owned()
    } else {
        String::new()
    };

    Ok(PermissionSnapshotDto {
        remote_access_enabled: settings.remote_access_enabled,
        resume_requires_confirm: resume_gate_path().exists(),
        concurrent_mode: settings.concurrent_mode.clone(),
        takeover_policy: settings.takeover_policy.clone(),
        takeover_timeout_s: settings.takeover_timeout_s,
        local_perms: settings.permission_bits,
        max_grant_perms: u32::from(PermBits::ALL),
        active_grants: store.active_count() as u32,
        grants,
        revoked_epoch: store.revoked_epoch(),
        trust_devices,
        session_running: status.running,
        session_role: status.role.clone(),
        connected_label,
        revocation_note: REVOCATION_UI_TEXT.to_owned(),
        audit_tail: read_audit_tail(20),
        grants_path: grants_path().display().to_string(),
        identity_fingerprint: identity.fingerprint(),
        hint,
    })
}

/// 校验有效期天数（0 视为默认 30，超过 90 拒绝）。
fn validity_from_days(days: u32) -> Result<u64, String> {
    if days == 0 {
        return Ok(DEFAULT_VALIDITY_MS);
    }
    let millis = u64::from(days)
        .checked_mul(DAY_MS)
        .ok_or_else(|| "有效期天数溢出".to_owned())?;
    if millis > MAX_VALIDITY_MS {
        return Err("有效期超过 90 天，拒绝签发".to_owned());
    }
    Ok(millis)
}

/// 内部签发：校验公钥、签名、落盘并审计。
fn issue_and_store(
    grantee_pub: &str,
    grantee_name: &str,
    template: &str,
    validity_days: u32,
    now: u64,
    audit: &mut AuditLog,
    epoch_base: Option<u64>,
) -> Result<(), String> {
    if grantee_pub.trim().is_empty() {
        return Err("被授权方公钥不能为空（请先从已配对设备选择）".to_owned());
    }
    fingerprint_from_b64(grantee_pub).map_err(|_| "被授权方公钥格式非法".to_owned())?;
    let bits = template_bits(template).ok_or_else(|| format!("未知模板：{template}"))?;
    let validity = validity_from_days(validity_days)?;
    let identity = load_or_create_identity()?;
    let owner_pub = identity.public_key_b64();
    let mut store = load_grants()?;
    let issued = issue_grant(
        &identity,
        &GrantSpec {
            host_pub: &owner_pub,
            grantee_pub,
            perms: bits,
            validity_ms: Some(validity),
            revoked_epoch_base: epoch_base.unwrap_or_else(|| store.revoked_epoch()),
        },
        now,
    )
    .map_err(|error| format!("{}: {}", error.code(), error))?;
    store
        .add(
            GrantRecord {
                token: issued.token,
                signature: issued.signature,
                grantee_name: grantee_name.to_owned(),
                created_ms: now,
                updated_ms: now,
                revoked: false,
                effective_perms: bits.to_bits(),
            },
            now,
            audit,
        )
        .map_err(|error| error.to_string())?;
    save_grants(&store)
}

/// 读取权限页快照。
///
/// # Returns
/// 真实授权/信任设备/会话/审计数据。
///
/// # Errors
/// 授权文件或设备文件损坏、身份无法落盘时返回错误文本。
pub fn permission_snapshot() -> Result<PermissionSnapshotDto, String> {
    build_snapshot()
}

/// 签发授权（按模板）。
///
/// # Arguments
/// * `grantee_pub` - 被授权方公钥（Base64）
/// * `grantee_name` - 显示名
/// * `template` - `readonly` / `operate` / `full`
/// * `validity_days` - 有效期天数（0=30，最大 90）
/// * `now_ms` - 当前 Unix 毫秒
///
/// # Returns
/// 更新后的快照。
///
/// # Errors
/// 公钥非法、模板未知、超 90 天或落盘失败时返回错误文本。
pub fn grant_create(
    grantee_pub: String,
    grantee_name: String,
    template: String,
    validity_days: u32,
    now_ms: u64,
) -> Result<PermissionSnapshotDto, String> {
    let mut audit = AuditLog::new(0);
    issue_and_store(
        &grantee_pub,
        &grantee_name,
        &template,
        validity_days,
        now_ms,
        &mut audit,
        None,
    )?;
    append_audit(&audit)?;
    build_snapshot()
}

/// 为已配对设备签发授权。
///
/// # Arguments
/// * `device_id` - 地址簿设备 ID
/// * `template` - 模板名
/// * `validity_days` - 有效期天数
/// * `now_ms` - 当前 Unix 毫秒
///
/// # Returns
/// 更新后的快照。
///
/// # Errors
/// 设备不存在、未信任、未采集公钥或签发失败时返回错误文本。
pub fn grant_create_for_device(
    device_id: String,
    template: String,
    validity_days: u32,
    now_ms: u64,
) -> Result<PermissionSnapshotDto, String> {
    let devices = DeviceList::load(device_path()).map_err(|error| error.to_string())?;
    let record = devices
        .get(&device_id)
        .ok_or_else(|| format!("设备不存在：{device_id}"))?;
    if !record.trusted {
        return Err("该设备尚未信任，不能签发授权".to_owned());
    }
    if record.public_key_b64.is_empty() {
        return Err("该设备尚未采集公钥，请重新配对后再授权".to_owned());
    }
    let mut audit = AuditLog::new(0);
    issue_and_store(
        &record.public_key_b64,
        &record.name,
        &template,
        validity_days,
        now_ms,
        &mut audit,
        None,
    )?;
    append_audit(&audit)?;
    build_snapshot()
}

/// 削减授权权限（只能减，不能加）。
///
/// # Arguments
/// * `grant_id` - 授权 ID
/// * `perms` - 新权限位（u16）
/// * `now_ms` - 当前 Unix 毫秒
///
/// # Returns
/// 更新后的快照。
///
/// # Errors
/// 授权不存在、已吊销或尝试追加权限时返回错误文本。
pub fn grant_update_perms(
    grant_id: String,
    perms: u32,
    now_ms: u64,
) -> Result<PermissionSnapshotDto, String> {
    let mut audit = AuditLog::new(0);
    let mut store = load_grants()?;
    store
        .update_perms(&grant_id, PermBits::new(perms as u16), now_ms, &mut audit)
        .map_err(|error| error.to_string())?;
    save_grants(&store)?;
    append_audit(&audit)?;
    build_snapshot()
}

/// 换模板重签（权限提升必须换新令牌，不能篡改旧令牌）。
///
/// # Arguments
/// * `grant_id` - 旧授权 ID
/// * `template` - 新模板
/// * `validity_days` - 新有效期
/// * `now_ms` - 当前 Unix 毫秒
///
/// # Returns
/// 更新后的快照。
///
/// # Errors
/// 旧授权不存在/已吊销、模板非法或落盘失败时返回错误文本。
pub fn grant_reissue(
    grant_id: String,
    template: String,
    validity_days: u32,
    now_ms: u64,
) -> Result<PermissionSnapshotDto, String> {
    let mut audit = AuditLog::new(0);
    let mut store = load_grants()?;
    let old = store
        .get(&grant_id)
        .ok_or_else(|| format!("授权不存在：{grant_id}"))?
        .clone();
    if old.revoked {
        return Err("授权已吊销，不能重签".to_owned());
    }
    issue_and_store(
        &old.token.grantee_pub,
        &old.grantee_name,
        &template,
        validity_days,
        now_ms,
        &mut audit,
        // 新令牌基线取「吊销后」的世代，避免下一跳把新令牌判成回退。
        Some(store.revoked_epoch().saturating_add(1)),
    )?;
    // issue_and_store 自己完成了加载/保存，这里必须重新加载，
    // 否则会用旧内存快照保存并丢掉刚签发的令牌。
    store = load_grants()?;
    store
        .revoke(&grant_id, now_ms, &mut audit)
        .map_err(|error| error.to_string())?;
    save_grants(&store)?;
    append_audit(&audit)?;
    build_snapshot()
}

/// 吊销授权（最终一致，UI 不得承诺「立即生效」）。
///
/// # Arguments
/// * `grant_id` - 授权 ID
/// * `now_ms` - 当前 Unix 毫秒
///
/// # Returns
/// 更新后的快照。
///
/// # Errors
/// 授权不存在或落盘失败时返回错误文本。
pub fn grant_revoke(grant_id: String, now_ms: u64) -> Result<PermissionSnapshotDto, String> {
    let mut audit = AuditLog::new(0);
    let mut store = load_grants()?;
    store
        .revoke(&grant_id, now_ms, &mut audit)
        .map_err(|error| error.to_string())?;
    save_grants(&store)?;
    append_audit(&audit)?;
    build_snapshot()
}

/// 从本机列表移除授权记录（不广播撤销）。
///
/// # Arguments
/// * `grant_id` - 授权 ID
///
/// # Returns
/// 更新后的快照。
///
/// # Errors
/// 落盘失败时返回错误文本。
pub fn grant_remove(grant_id: String) -> Result<PermissionSnapshotDto, String> {
    let mut store = load_grants()?;
    store.remove(&grant_id);
    save_grants(&store)?;
    build_snapshot()
}

/// 立即停止远程访问：停输入 → 断会话 → 落闸门 → 审计。
///
/// # Arguments
/// * `now_ms` - 当前 Unix 毫秒
///
/// # Returns
/// 停止报告（含恢复需二次确认状态）。
///
/// # Errors
/// 设置或闸门落盘失败时返回错误文本。
pub fn remote_access_stop(now_ms: u64) -> Result<RemoteStopDto, String> {
    let input_stopped = input_service::stop().is_ok();
    let teardown = session_service::disconnect("owner_stop_remote_access");
    let (session_stopped, teardown_ok, teardown_summary) = match &teardown {
        Ok(report) => (
            true,
            report.ok,
            format!(
                "bye_sent={} acked={} services_running_after={} ports_released={}",
                report.bye_sent, report.bye_acked, report.services_running_after,
                report.ports_released
            ),
        ),
        Err(error) => (true, false, error.clone()),
    };

    let mut settings = AppSettings::load(settings_path()).map_err(|error| error.to_string())?;
    settings.remote_access_enabled = false;
    settings
        .save(settings_path())
        .map_err(|error| error.to_string())?;
    let gate = serde_json::json!({ "stopped_ms": now_ms });
    std::fs::write(resume_gate_path(), gate.to_string()).map_err(|error| error.to_string())?;

    let mut audit = AuditLog::new(0);
    audit.record(
        now_ms,
        "remote_access_stop",
        "local",
        "stop_remote_access",
        Some("remote_access_enabled=true"),
        Some("remote_access_enabled=false"),
        if teardown_ok { "ok" } else { "partial" },
    );
    let audit_written = append_audit(&audit).is_ok();
    Ok(RemoteStopDto {
        input_stopped,
        session_stopped,
        teardown_ok,
        teardown_summary,
        audit_written,
        resume_requires_confirm: true,
        stopped_at_ms: now_ms,
        snapshot: build_snapshot()?,
    })
}

/// 恢复远程访问（必须显式二次确认）。
///
/// # Arguments
/// * `confirm` - 本机二次确认结果；`false` 一律拒绝
/// * `now_ms` - 当前 Unix 毫秒
///
/// # Returns
/// 更新后的快照。
///
/// # Errors
/// 未确认、设置或闸门写入失败时返回错误文本。
pub fn remote_access_resume(
    confirm: bool,
    now_ms: u64,
) -> Result<PermissionSnapshotDto, String> {
    if !confirm {
        return Err("恢复远程访问需要本机二次确认".to_owned());
    }
    let mut settings = AppSettings::load(settings_path()).map_err(|error| error.to_string())?;
    settings.remote_access_enabled = true;
    settings
        .save(settings_path())
        .map_err(|error| error.to_string())?;
    match std::fs::remove_file(resume_gate_path()) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(format!("清理恢复闸门失败：{err}")),
    }
    let mut audit = AuditLog::new(0);
    audit.record(
        now_ms,
        "remote_access_resume",
        "local",
        "resume_remote_access",
        Some("remote_access_enabled=false"),
        Some("remote_access_enabled=true"),
        "allowed",
    );
    append_audit(&audit)?;
    build_snapshot()
}

/// 写入一台设备的身份公钥（配对成功后调用；页面无此入口，供配对流程使用）。
///
/// # Arguments
/// * `device_id` - 设备 ID
/// * `public_key_b64` - 对端公钥
///
/// # Returns
/// 更新后的信任设备快照。
///
/// # Errors
/// 设备不存在、公钥非法或落盘失败时返回错误文本。
pub fn device_set_public_key(
    device_id: String,
    public_key_b64: String,
) -> Result<PermissionSnapshotDto, String> {
    let mut devices = DeviceList::load(device_path()).map_err(|error| error.to_string())?;
    if devices.get(&device_id).is_none() {
        return Err(format!("设备不存在：{device_id}"));
    }
    if !public_key_b64.is_empty() {
        fingerprint_from_b64(&public_key_b64).map_err(|_| "公钥格式非法".to_owned())?;
    }
    devices.set_public_key(&device_id, &public_key_b64);
    devices
        .save(device_path())
        .map_err(|error| error.to_string())?;
    build_snapshot()
}
