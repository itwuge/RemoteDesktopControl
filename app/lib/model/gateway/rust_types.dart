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
    required this.encodedBytes,
    required this.bitrateKbps,
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

  /// 编码输出累计字节数。
  final int encodedBytes;

  /// 实测码率（千比特/秒，1 秒窗口）。
  final int bitrateKbps;

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

/// 控制页最新一帧 RGBA 画面（S8-3：Linux 无外部纹理时的降级上屏通道）。
class ControlFrameInfo {
  /// 创建帧快照。
  const ControlFrameInfo({
    required this.frameId,
    required this.width,
    required this.height,
    required this.ptsUs,
    required this.latencyUs,
    required this.bytes,
  });

  /// 帧序号（单调递增）。
  final int frameId;

  /// 宽。
  final int width;

  /// 高。
  final int height;

  /// 采集时间戳（微秒）。
  final int ptsUs;

  /// 端到端延迟（微秒）。
  final int latencyUs;

  /// RGBA8888 像素。
  final Uint8List bytes;
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
    this.deleteUnits = 0,
    this.text = '',
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

  /// 文本/输入法编辑：先删除 [deleteUnits] 个 UTF-16 码元，再插入 [text]。
  ///
  /// 用“删除 + 插入”而不是逐字符扫描码，才能表达输入法组合串、候选词替换
  /// 与 emoji（BMP 外字符占两个码元）。`text` 为空且 [deleteUnits] > 0 时
  /// 等价于退格；两者都为 0 是空操作，Rust 侧会拒绝。
  const InputEventSpec.text({required String text, int deleteUnits = 0}) : this(
    kind: 'text',
    text: text,
    deleteUnits: deleteUnits,
  );

  /// 事件类型：`move` / `button` / `wheel` / `key` / `text`。
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

  /// 文本事件：插入前先删除的 UTF-16 码元数（退格次数）。
  final int deleteUnits;

  /// 文本事件：要插入的 UTF-8 文本；空串表示纯删除。
  final String text;

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

/// 剪贴板同步状态（Rust → Dart）。
class ClipboardSyncInfo {
  /// 创建快照。
  const ClipboardSyncInfo({
    required this.running,
    required this.remoteChars,
    required this.localChars,
    required this.sent,
    required this.applied,
    required this.skippedDuplicates,
    required this.skippedEchoes,
    required this.rejected,
    required this.lastHash,
    required this.verified,
    required this.lastError,
  });

  /// 服务是否运行。
  final bool running;

  /// 远端缓冲区字符数。
  final int remoteChars;

  /// 最近外发文本字符数。
  final int localChars;

  /// 外发条数。
  final int sent;

  /// 写回条数。
  final int applied;

  /// 重复内容抑制次数。
  final int skippedDuplicates;

  /// 回环抑制次数。
  final int skippedEchoes;

  /// 超限拒绝次数。
  final int rejected;

  /// 最近内容 SHA-256。
  final String lastHash;

  /// 回读校验是否一致。
  final bool verified;

  /// 最近错误。
  final String? lastError;
}

// =====** S5-5 授权与权限页 =====**

/// 授权条目。
class GrantEntry {
  /// 创建条目。
  const GrantEntry({
    required this.grantId,
    required this.granteePub,
    required this.granteeName,
    required this.granteeFingerprint,
    required this.template,
    required this.perms,
    required this.effectivePerms,
    required this.notBeforeMs,
    required this.expiresAtMs,
    required this.revoked,
    required this.createdMs,
    required this.updatedMs,
  });

  /// 授权 ID。
  final String grantId;

  /// 被授权方公钥。
  final String granteePub;

  /// 被授权方显示名。
  final String granteeName;

  /// 公钥指纹。
  final String granteeFingerprint;

  /// 模板名。
  final String template;

  /// 令牌权限位。
  final int perms;

  /// 本机生效权限位。
  final int effectivePerms;

  /// 生效时间（Unix 毫秒）。
  final int notBeforeMs;

  /// 到期时间（Unix 毫秒）。
  final int expiresAtMs;

  /// 是否已吊销。
  final bool revoked;

  /// 创建时间。
  final int createdMs;

  /// 最后修改时间。
  final int updatedMs;
}

/// 信任设备（权限页视角）。
class TrustDeviceEntry {
  /// 创建条目。
  const TrustDeviceEntry({
    required this.deviceId,
    required this.name,
    required this.trusted,
    required this.sameAccount,
    required this.fingerprint,
    required this.passwordlessReady,
    required this.authMode,
    required this.lastOnlineMs,
  });

  /// 设备 ID。
  final String deviceId;

  /// 显示名。
  final String name;

  /// 是否信任。
  final bool trusted;

  /// 是否同账号。
  final bool sameAccount;

  /// 公钥指纹。
  final String fingerprint;

  /// 是否具备免密条件。
  final bool passwordlessReady;

  /// 实际认证方式。
  final String authMode;

  /// 最后在线（Unix 毫秒）。
  final int lastOnlineMs;
}

/// 审计条目。
class PermissionAuditEntry {
  /// 创建条目。
  const PermissionAuditEntry({
    required this.tsMs,
    required this.kind,
    required this.subject,
    required this.action,
    required this.oldValue,
    required this.newValue,
    required this.outcome,
  });

  /// Unix 毫秒。
  final int tsMs;

  /// 事件类别。
  final String kind;

  /// 主体。
  final String subject;

  /// 动作。
  final String action;

  /// 旧值。
  final String oldValue;

  /// 新值。
  final String newValue;

  /// 结果。
  final String outcome;
}

/// 权限页快照。
class PermissionSnapshot {
  /// 创建快照。
  const PermissionSnapshot({
    required this.remoteAccessEnabled,
    required this.resumeRequiresConfirm,
    required this.concurrentMode,
    required this.takeoverPolicy,
    required this.takeoverTimeoutS,
    required this.localPerms,
    required this.maxGrantPerms,
    required this.activeGrants,
    required this.grants,
    required this.revokedEpoch,
    required this.trustDevices,
    required this.sessionRunning,
    required this.sessionRole,
    required this.connectedLabel,
    required this.revocationNote,
    required this.auditTail,
    required this.grantsPath,
    required this.identityFingerprint,
    required this.hint,
  });

  /// 远程访问总开关。
  final bool remoteAccessEnabled;

  /// 是否处于「已停止，恢复需二次确认」状态。
  final bool resumeRequiresConfirm;

  /// 并发模式。
  final String concurrentMode;

  /// 接管策略。
  final String takeoverPolicy;

  /// 本机确认超时（秒）。
  final int takeoverTimeoutS;

  /// 本机允许权限位。
  final int localPerms;

  /// 可授予最大权限位。
  final int maxGrantPerms;

  /// 未吊销授权数。
  final int activeGrants;

  /// 授权列表。
  final List<GrantEntry> grants;

  /// 撤销世代。
  final int revokedEpoch;

  /// 信任设备。
  final List<TrustDeviceEntry> trustDevices;

  /// 是否有远程会话。
  final bool sessionRunning;

  /// 当前角色。
  final String sessionRole;

  /// 「谁在连接」文案。
  final String connectedLabel;

  /// 撤销最终一致说明。
  final String revocationNote;

  /// 最近审计。
  final List<PermissionAuditEntry> auditTail;

  /// 授权存储路径。
  final String grantsPath;

  /// 本机身份指纹。
  final String identityFingerprint;

  /// 非致命提示。
  final String hint;
}

/// 立即停止远程访问报告。
class RemoteStopReport {
  /// 创建报告。
  const RemoteStopReport({
    required this.inputStopped,
    required this.sessionStopped,
    required this.teardownOk,
    required this.teardownSummary,
    required this.auditWritten,
    required this.resumeRequiresConfirm,
    required this.stoppedAtMs,
    required this.snapshot,
  });

  /// 输入通道是否已停。
  final bool inputStopped;

  /// 会话是否已断。
  final bool sessionStopped;

  /// 收尾是否通过。
  final bool teardownOk;

  /// 收尾摘要。
  final String teardownSummary;

  /// 审计是否落盘。
  final bool auditWritten;

  /// 恢复是否需要二次确认。
  final bool resumeRequiresConfirm;

  /// 停止时刻。
  final int stoppedAtMs;

  /// 停止后的快照。
  final PermissionSnapshot snapshot;
}

/// 文件传输结果（Rust → Dart）。
class FileTransferInfo {
  /// 创建结果。
  const FileTransferInfo({
    required this.source,
    required this.dest,
    required this.bytes,
    required this.chunks,
    required this.sourceSha256,
    required this.destSha256,
    required this.equal,
    required this.resumed,
    required this.resumedBytes,
    required this.interrupted,
  });

  /// 源文件路径。
  final String source;

  /// 落盘路径。
  final String dest;

  /// 字节数。
  final int bytes;

  /// 分块数。
  final int chunks;

  /// 源文件 SHA-256。
  final String sourceSha256;

  /// 落盘文件 SHA-256。
  final String destSha256;

  /// 两端是否一致。
  final bool equal;

  /// 是否续传过。
  final bool resumed;

  /// 续传跳过的字节数。
  final int resumedBytes;

  /// 是否按测试参数中断过。
  final bool interrupted;
}

/// 会话状态（Rust → Dart）。
class SessionStatusInfo {
  /// 创建快照。
  const SessionStatusInfo({
    required this.running,
    required this.sessionId,
    required this.role,
    required this.localPort,
    required this.peerPort,
    required this.elapsedMs,
    required this.byeSent,
    required this.byeAcked,
  });

  /// 是否在会话中。
  final bool running;

  /// 会话 ID。
  final String sessionId;

  /// 角色。
  final String role;

  /// 控制端回环端口。
  final int localPort;

  /// 被控端回环端口。
  final int peerPort;

  /// 已运行时长（毫秒）。
  final int elapsedMs;

  /// 是否已发出 bye。
  final bool byeSent;

  /// 是否收到回执。
  final bool byeAcked;
}

/// 单个收尾步骤。
class TeardownStepInfo {
  /// 创建步骤结果。
  const TeardownStepInfo({
    required this.name,
    required this.ok,
    required this.detail,
  });

  /// 步骤名。
  final String name;

  /// 是否成功。
  final bool ok;

  /// 说明。
  final String detail;
}

/// 会话收尾报告（bye 往返 + 资源核查）。
class SessionTeardownInfo {
  /// 创建报告。
  const SessionTeardownInfo({
    required this.sessionId,
    required this.role,
    required this.byeSent,
    required this.byeReceived,
    required this.byeAcked,
    required this.ackLatencyUs,
    required this.byeBytes,
    required this.steps,
    required this.servicesRunningAfter,
    required this.handlesBefore,
    required this.handlesAfter,
    required this.handlesDelta,
    required this.portsReleased,
    required this.ok,
    required this.lastError,
  });

  /// 会话 ID。
  final String sessionId;

  /// 角色。
  final String role;

  /// bye 是否发出。
  final bool byeSent;

  /// 对端是否解出 bye。
  final bool byeReceived;

  /// 是否收到回执。
  final bool byeAcked;

  /// 回执往返（微秒）。
  final int ackLatencyUs;

  /// bye 报文字节数。
  final int byeBytes;

  /// 按序收尾步骤。
  final List<TeardownStepInfo> steps;

  /// 收尾后仍在运行的服务数。
  final int servicesRunningAfter;

  /// 断开前句柄数。
  final int handlesBefore;

  /// 收尾后句柄数。
  final int handlesAfter;

  /// 句柄差值。
  final int handlesDelta;

  /// 端口是否释放。
  final bool portsReleased;

  /// 整体是否通过。
  final bool ok;

  /// 最近错误。
  final String? lastError;
}
