// ============================================================
// 文件用途：FFI 返回值对应的 Dart 值对象
//
// 层级：model（跨层数据契约，不含逻辑）
// 被谁调用：model/gateway/rust_gateway.dart、controller/home_controller.dart
// ============================================================

import 'dart:typed_data';

/// Rust 核心的版本与运行环境信息。
class CoreInfo {
  /// 创建信息快照。
  const CoreInfo({
    required this.coreVersion,
    required this.platform,
    required this.bridgeStatus,
  });

  /// 未初始化时的占位值（用于 UI 首帧，避免空字符串闪烁）。
  const CoreInfo.empty()
    : coreVersion = '-',
      platform = '-',
      bridgeStatus = '-';

  /// Rust 核心版本号。
  final String coreVersion;

  /// 构建平台（`os-arch`）。
  final String platform;

  /// FFI 链路状态，正常为 `OK`。
  final String bridgeStatus;

  /// 桥接是否正常。
  bool get isBridgeHealthy => bridgeStatus == 'OK';
}

/// 设备条目（来自 Rust 设备列表）。
class DeviceEntry {
  /// 创建设备条目。
  const DeviceEntry({
    required this.deviceId,
    required this.name,
    required this.lastOnline,
    required this.trusted,
    required this.sameAccount,
  });

  /// 设备标识。
  final String deviceId;

  /// 显示名。
  final String name;

  /// 最后在线时间；`null` 表示从未上线。
  final DateTime? lastOnline;

  /// 是否信任。
  final bool trusted;

  /// 是否同账号（仅字段，S5-2 不做判定）。
  final bool sameAccount;
}

/// 设备列表快照。
class DeviceListSnapshot {
  /// 创建快照。
  const DeviceListSnapshot({required this.entries, required this.filePath});

  /// 设备条目。
  final List<DeviceEntry> entries;

  /// 落盘路径。
  final String filePath;
}

/// 会话角色权限。
class RoleInfo {
  /// 创建角色信息。
  const RoleInfo({
    required this.role,
    required this.canInjectInput,
    required this.canViewWall,
  });

  /// `viewer` / `controller`。
  final String role;

  /// 是否可注入输入。
  final bool canInjectInput;

  /// 是否可拉子码流。
  final bool canViewWall;
}

/// 屏幕墙服务状态。
class WallStatusInfo {
  /// 创建状态快照。
  const WallStatusInfo({
    required this.running,
    required this.backend,
    required this.captureBackend,
    required this.fps,
    required this.encodedFrames,
    required this.decodedFrames,
    required this.droppedFrames,
    required this.width,
    required this.height,
    required this.latencyUs,
    required this.latestPtsUs,
    required this.lastError,
  });

  /// 是否运行。
  final bool running;

  /// 编码器后端名。
  final String backend;

  /// 采集后端名（如 `dxgi-desktop-duplication`）。
  final String captureBackend;

  /// 实测帧率。
  final double fps;

  /// 已编码帧数。
  final int encodedFrames;

  /// 已解码帧数。
  final int decodedFrames;

  /// 重组丢弃帧数。
  final int droppedFrames;

  /// 预览宽。
  final int width;

  /// 预览高。
  final int height;

  /// 最近端到端延迟（微秒）。
  final int latencyUs;

  /// 最近帧时间戳（微秒）。
  final int latestPtsUs;

  /// 最近错误。
  final String? lastError;
}

/// 控制页真实画面状态（外部纹理 + 实测指标）。
class ControlPreviewInfo {
  /// 创建状态快照。
  const ControlPreviewInfo({
    required this.running,
    required this.backend,
    required this.encoder,
    required this.decoder,
    required this.texturePath,
    required this.decodePath,
    required this.width,
    required this.height,
    required this.fps,
    required this.capturedFrames,
    required this.encodedFrames,
    required this.decodedFrames,
    required this.droppedFrames,
    required this.latencyLastUs,
    required this.latencyP50Us,
    required this.latencyP99Us,
    required this.readbackCount,
    required this.shareHandleA,
    required this.shareHandleB,
    required this.eventA,
    required this.eventB,
    required this.lastError,
  });

  /// 是否运行。
  final bool running;

  /// 采集后端。
  final String backend;

  /// 编码器名。
  final String encoder;

  /// 解码器名。
  final String decoder;

  /// 上屏通道。
  final String texturePath;

  /// 解码输出路径。
  final String decodePath;

  /// 画面宽。
  final int width;

  /// 画面高。
  final int height;

  /// 实测帧率。
  final double fps;

  /// 采集帧数。
  final int capturedFrames;

  /// 编码包数。
  final int encodedFrames;

  /// 解码帧数。
  final int decodedFrames;

  /// 采集丢帧数。
  final int droppedFrames;

  /// 最近端到端延迟（微秒）。
  final int latencyLastUs;

  /// 延迟 p50（微秒）。
  final int latencyP50Us;

  /// 延迟 p99（微秒）。
  final int latencyP99Us;

  /// 时间码回读次数。
  final int readbackCount;

  /// 共享纹理句柄 A。
  final int shareHandleA;

  /// 共享纹理句柄 B。
  final int shareHandleB;

  /// 新帧事件 A。
  final int eventA;

  /// 新帧事件 B。
  final int eventB;

  /// 最近错误。
  final String? lastError;
}

/// 设置快照（FFI 边界值对象，由 SettingsRepository 转成 AppSettings）。
class SettingsSnapshot {
  /// 创建设置快照。
  const SettingsSnapshot({
    required this.allowRelay,
    required this.relayAddress,
    required this.connectionPolicy,
    required this.orientation,
    required this.fileBandwidthRatio,
    required this.videoWidth,
    required this.videoHeight,
    required this.videoFps,
    required this.videoBitrateMbps,
    required this.autoReconnect,
    required this.remoteAccessEnabled,
    required this.concurrentMode,
    required this.takeoverPolicy,
    required this.takeoverTimeoutS,
    required this.permissionTemplate,
    required this.permissionBits,
    required this.localNotice,
    required this.auditRetentionDays,
    required this.localPairPolicy,
    required this.sameAccountPolicy,
    required this.passwordlessPolicy,
    required this.filePath,
  });

  /// 是否允许中继。
  final bool allowRelay;

  /// 中继地址。
  final String relayAddress;

  /// 打洞失败策略。
  final String connectionPolicy;

  /// 屏幕方向策略。
  final String orientation;

  /// 文件带宽占比。
  final double fileBandwidthRatio;

  /// 主码流宽。
  final int videoWidth;

  /// 主码流高。
  final int videoHeight;

  /// 主码流帧率。
  final int videoFps;

  /// 主码流码率（Mbps）。
  final double videoBitrateMbps;

  /// 断线自动重连。
  final bool autoReconnect;

  /// 允许远程访问。
  final bool remoteAccessEnabled;

  /// 并发模式。
  final String concurrentMode;

  /// 接管策略。
  final String takeoverPolicy;

  /// 本机确认超时（秒）。
  final int takeoverTimeoutS;

  /// 权限模板。
  final String permissionTemplate;

  /// 权限位图。
  final int permissionBits;

  /// 本机常驻提示。
  final bool localNotice;

  /// 审计保留天数。
  final int auditRetentionDays;

  /// 本地配对策略。
  final String localPairPolicy;

  /// 同账号配对策略。
  final String sameAccountPolicy;

  /// 免密策略。
  final bool passwordlessPolicy;

  /// 落盘路径。
  final String filePath;
}

/// 一帧屏幕墙预览（RGBA）。
class WallPreviewFrame {
  /// 创建预览帧。
  const WallPreviewFrame({
    required this.streamId,
    required this.frameId,
    required this.ptsUs,
    required this.width,
    required this.height,
    required this.latencyUs,
    required this.bytes,
  });

  /// 流编号。
  final int streamId;

  /// 帧号。
  final int frameId;

  /// 帧时间戳（微秒）。
  final int ptsUs;

  /// 宽。
  final int width;

  /// 高。
  final int height;

  /// 端到端延迟（微秒）。
  final int latencyUs;

  /// RGBA 像素。
  final Uint8List bytes;
}

/// 一个待注入的输入事件（Dart → Rust）。
///
/// 坐标是**被控端桌面物理像素**：由 [ViewportController] 的矩阵
/// （letterbox + DPI + 缩放/平移）换算，Rust 侧只做线格式往返与注入。
class InputEventSpec {
  /// 创建事件。
  const InputEventSpec({
    required this.kind,
    this.x = 0,
    this.y = 0,
    this.button = 0,
    this.pressed = false,
    this.deltaX = 0,
    this.deltaY = 0,
    this.scanCode = 0,
    this.modifiers = 0,
  });

  /// 指针移动（无按键）。
  const InputEventSpec.move({required int x, required int y})
    : this(kind: 'move', x: x, y: y);

  /// 指针按键（按下或抬起）。
  const InputEventSpec.button({
    required int x,
    required int y,
    required int button,
    required bool pressed,
  }) : this(kind: 'button', x: x, y: y, button: button, pressed: pressed);

  /// 滚轮（刻度）。
  const InputEventSpec.wheel({int deltaX = 0, int deltaY = 0})
    : this(kind: 'wheel', deltaX: deltaX, deltaY: deltaY);

  /// 键盘（扫描码）。
  const InputEventSpec.key({
    required int scanCode,
    required bool pressed,
    int modifiers = 0,
  }) : this(
         kind: 'key',
         scanCode: scanCode,
         pressed: pressed,
         modifiers: modifiers,
       );

  /// 事件类型：`move` / `button` / `wheel` / `key`。
  final String kind;

  /// 桌面横坐标（物理像素）。
  final int x;

  /// 桌面纵坐标（物理像素）。
  final int y;

  /// 指针按键：`0` 无、`1` 左、`2` 右、`3` 中。
  final int button;

  /// 按下为 `true`（指针与键盘共用）。
  final bool pressed;

  /// 滚轮水平刻度。
  final int deltaX;

  /// 滚轮垂直刻度（正值为下）。
  final int deltaY;

  /// PC/AT Set 1 扫描码。
  final int scanCode;

  /// 修饰键位掩码：`1=Shift`、`2=Ctrl`、`4=Alt`、`8=Meta`。
  final int modifiers;
}

/// 输入注入统计（Rust → Dart）。
class InputStatsInfo {
  /// 创建统计快照。
  const InputStatsInfo({
    required this.running,
    required this.received,
    required this.emitted,
    required this.injected,
    required this.rejected,
    required this.merged,
    required this.throttled,
    required this.pointerEvents,
    required this.keyEvents,
    required this.wheelEvents,
    required this.keysDown,
    required this.lastX,
    required this.lastY,
    required this.successRate,
    required this.lastError,
  });

  /// 未启动时的空快照。
  const InputStatsInfo.empty()
    : running = false,
      received = 0,
      emitted = 0,
      injected = 0,
      rejected = 0,
      merged = 0,
      throttled = 0,
      pointerEvents = 0,
      keyEvents = 0,
      wheelEvents = 0,
      keysDown = 0,
      lastX = 0,
      lastY = 0,
      successRate = 0,
      lastError = null;

  /// 通道是否运行。
  final bool running;

  /// 收到事件数。
  final int received;

  /// 合并器输出事件数。
  final int emitted;

  /// 注入成功数。
  final int injected;

  /// 注入被拒数。
  final int rejected;

  /// 合并计数。
  final int merged;

  /// 限频计数。
  final int throttled;

  /// 指针事件数。
  final int pointerEvents;

  /// 键盘事件数。
  final int keyEvents;

  /// 滚轮事件数。
  final int wheelEvents;

  /// 当前按下的键数。
  final int keysDown;

  /// 最近指针横坐标。
  final int lastX;

  /// 最近指针纵坐标。
  final int lastY;

  /// 注入成功率。
  final double successRate;

  /// 最近错误。
  final String? lastError;
}
