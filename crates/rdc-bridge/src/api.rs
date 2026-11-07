//! rdc-bridge / 对外 API
//!
//! 层级：L4 桥接层
//! 用途：flutter_rust_bridge 的扫描入口（rust_input: crate::api）；
//!       每个 `pub fn` 会生成一个同名 Dart 函数
//! 调用方：生成的 Dart 绑定，再由 model/gateway/rust_gateway.dart 统一收口

use std::path::PathBuf;

use rdc_core::s5::devices::{now_ms, DeviceList, DeviceRecord};
use rdc_core::s5::multi::{authorize_input, wall_capabilities, SessionRole};
use rdc_core::s5::settings::AppSettings;
use rdc_protocol::Capabilities;

use crate::wall_service;

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
#[derive(Debug, Clone)]
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
    #[cfg(not(windows))]
    {
        Err("控制页外部纹理仅支持 Windows 桌面".to_owned())
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
    #[cfg(not(windows))]
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
    #[cfg(not(windows))]
    {
        Ok(ControlStatusDto::default())
    }
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
