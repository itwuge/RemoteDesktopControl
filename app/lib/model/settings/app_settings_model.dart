// ============================================================
// 文件用途：应用设置模型与序列化
//
// 层级：model（唯一设置真源；不直接读写文件）
// 被谁调用：model/settings/settings_repository.dart、controller/settings_controller.dart
//          controller/permission_controller.dart
// 边界：字段默认值必须与 docs/06、docs/13 一致；新增字段要同步 fromJson/toJson
// ============================================================

/// 打洞失败后的策略（与 Rust `connection_policy` 一一对应）。
enum ConnectionPolicy {
  /// 严格 P2P：失败即报错。
  strictP2p,

  /// 询问后降级。
  askThenFallback,

  /// 自动降级（仍需中继总开关开启）。
  autoFallback,
}

/// 屏幕方向策略（移动端生效，桌面端仅保存）。
enum OrientationPolicy { followController, landscape, portrait }

/// 应用设置（设置页 + 权限页共用一份文件）。
class AppSettings {
  /// 创建设置快照。
  const AppSettings({
    required this.allowRelay,
    required this.relayAddress,
    required this.connectionPolicy,
    required this.orientation,
    required this.fileBandwidthRatio,
    this.videoWidth = 1920,
    this.videoHeight = 1080,
    this.videoFps = 30,
    this.videoBitrateMbps = 8.0,
    this.autoReconnect = true,
    this.remoteAccessEnabled = false,
    this.concurrentMode = 'exclusive',
    this.takeoverPolicy = 'confirm',
    this.takeoverTimeoutS = 30,
    this.permissionTemplate = 'readonly',
    this.permissionBits = 1,
    this.localNotice = true,
    this.auditRetentionDays = 30,
    this.localPairPolicy = 'ask',
    this.sameAccountPolicy = 'auto',
    this.passwordlessPolicy = false,
  });

  /// 默认设置：默认不允许中继、严格 P2P、跟随主控端、文件上限 40%。
  const AppSettings.defaults()
      : allowRelay = false,
        relayAddress = '',
        connectionPolicy = ConnectionPolicy.strictP2p,
        orientation = OrientationPolicy.followController,
        fileBandwidthRatio = 0.4,
        videoWidth = 1920,
        videoHeight = 1080,
        videoFps = 30,
        videoBitrateMbps = 8.0,
        autoReconnect = true,
        remoteAccessEnabled = false,
        concurrentMode = 'exclusive',
        takeoverPolicy = 'confirm',
        takeoverTimeoutS = 30,
        permissionTemplate = 'readonly',
        permissionBits = 1,
        localNotice = true,
        auditRetentionDays = 30,
        localPairPolicy = 'ask',
        sameAccountPolicy = 'auto',
        passwordlessPolicy = false;

  /// 是否允许使用中继（默认关闭，docs/01 C2）。
  final bool allowRelay;

  /// 中继服务器地址（手填，不做服务端发现）。
  final String relayAddress;

  /// 打洞失败策略。
  final ConnectionPolicy connectionPolicy;

  /// 方向策略。
  final OrientationPolicy orientation;

  /// 文件传输带宽上限占可用上行比例（0.1~0.7）。
  final double fileBandwidthRatio;

  /// 主码流目标宽。
  final int videoWidth;

  /// 主码流目标高。
  final int videoHeight;

  /// 主码流目标帧率。
  final int videoFps;

  /// 主码流目标码率（Mbps）。
  final double videoBitrateMbps;

  /// 断线自动重连。
  final bool autoReconnect;

  /// 是否允许远程访问本机。
  final bool remoteAccessEnabled;

  /// 并发模式：`exclusive` / `one_control_many_watch`。
  final String concurrentMode;

  /// 接管策略：`auto` / `confirm` / `forbid`。
  final String takeoverPolicy;

  /// 本机确认超时（秒）。
  final int takeoverTimeoutS;

  /// 权限模板：`readonly` / `standard` / `full` / `custom`。
  final String permissionTemplate;

  /// 七位权限位图。
  final int permissionBits;

  /// 连接时是否常驻本机提示。
  final bool localNotice;

  /// 审计日志保留天数。
  final int auditRetentionDays;

  /// 本地配对默认策略。
  final String localPairPolicy;

  /// 同账号默认策略。
  final String sameAccountPolicy;

  /// 是否允许完全免密。
  final bool passwordlessPolicy;

  /// 复制一份并覆盖部分字段。
  ///
  /// 参数均为可选覆盖项；未传字段保持原值。
  AppSettings copyWith({
    bool? allowRelay,
    String? relayAddress,
    ConnectionPolicy? connectionPolicy,
    OrientationPolicy? orientation,
    double? fileBandwidthRatio,
    int? videoWidth,
    int? videoHeight,
    int? videoFps,
    double? videoBitrateMbps,
    bool? autoReconnect,
    bool? remoteAccessEnabled,
    String? concurrentMode,
    String? takeoverPolicy,
    int? takeoverTimeoutS,
    String? permissionTemplate,
    int? permissionBits,
    bool? localNotice,
    int? auditRetentionDays,
    String? localPairPolicy,
    String? sameAccountPolicy,
    bool? passwordlessPolicy,
  }) {
    return AppSettings(
      allowRelay: allowRelay ?? this.allowRelay,
      relayAddress: relayAddress ?? this.relayAddress,
      connectionPolicy: connectionPolicy ?? this.connectionPolicy,
      orientation: orientation ?? this.orientation,
      fileBandwidthRatio: fileBandwidthRatio ?? this.fileBandwidthRatio,
      videoWidth: videoWidth ?? this.videoWidth,
      videoHeight: videoHeight ?? this.videoHeight,
      videoFps: videoFps ?? this.videoFps,
      videoBitrateMbps: videoBitrateMbps ?? this.videoBitrateMbps,
      autoReconnect: autoReconnect ?? this.autoReconnect,
      remoteAccessEnabled: remoteAccessEnabled ?? this.remoteAccessEnabled,
      concurrentMode: concurrentMode ?? this.concurrentMode,
      takeoverPolicy: takeoverPolicy ?? this.takeoverPolicy,
      takeoverTimeoutS: takeoverTimeoutS ?? this.takeoverTimeoutS,
      permissionTemplate: permissionTemplate ?? this.permissionTemplate,
      permissionBits: permissionBits ?? this.permissionBits,
      localNotice: localNotice ?? this.localNotice,
      auditRetentionDays: auditRetentionDays ?? this.auditRetentionDays,
      localPairPolicy: localPairPolicy ?? this.localPairPolicy,
      sameAccountPolicy: sameAccountPolicy ?? this.sameAccountPolicy,
      passwordlessPolicy: passwordlessPolicy ?? this.passwordlessPolicy,
    );
  }

  /// 序列化为 Rust 端一致的 snake_case JSON。
  Map<String, dynamic> toJson() => {
        'allow_relay': allowRelay,
        'relay_address': relayAddress,
        'connection_policy': connectionPolicyToWire(connectionPolicy),
        'orientation': orientationToWire(orientation),
        'file_bandwidth_ratio': fileBandwidthRatio,
        'video_width': videoWidth,
        'video_height': videoHeight,
        'video_fps': videoFps,
        'video_bitrate_mbps': videoBitrateMbps,
        'auto_reconnect': autoReconnect,
        'remote_access_enabled': remoteAccessEnabled,
        'concurrent_mode': concurrentMode,
        'takeover_policy': takeoverPolicy,
        'takeover_timeout_s': takeoverTimeoutS,
        'permission_template': permissionTemplate,
        'permission_bits': permissionBits,
        'local_notice': localNotice,
        'audit_retention_days': auditRetentionDays,
        'local_pair_policy': localPairPolicy,
        'same_account_policy': sameAccountPolicy,
        'passwordless_policy': passwordlessPolicy,
      };

  /// 从磁盘 JSON 还原；缺字段或非法值一律回落默认，避免旧配置让应用崩在启动路径。
  ///
  /// [json] 必须是 [Map]；未知枚举名回落到默认。
  factory AppSettings.fromJson(Map<String, dynamic> json) {
    const fallback = AppSettings.defaults();
    final ratio = (json['file_bandwidth_ratio'] as num?)?.toDouble();
    return AppSettings(
      allowRelay: json['allow_relay'] as bool? ?? fallback.allowRelay,
      relayAddress: json['relay_address'] as String? ?? fallback.relayAddress,
      connectionPolicy: _parsePolicy(json['connection_policy'] as String?),
      orientation: _parseOrientation(json['orientation'] as String?),
      // 业务约束：比例必须落在 10%~70%，越界视为损坏配置。
      fileBandwidthRatio: (ratio != null && ratio >= 0.1 && ratio <= 0.7)
          ? ratio
          : fallback.fileBandwidthRatio,
      videoWidth: (json['video_width'] as num?)?.toInt() ?? fallback.videoWidth,
      videoHeight:
          (json['video_height'] as num?)?.toInt() ?? fallback.videoHeight,
      videoFps: (json['video_fps'] as num?)?.toInt() ?? fallback.videoFps,
      videoBitrateMbps: (json['video_bitrate_mbps'] as num?)?.toDouble() ??
          fallback.videoBitrateMbps,
      autoReconnect:
          json['auto_reconnect'] as bool? ?? fallback.autoReconnect,
      remoteAccessEnabled:
          json['remote_access_enabled'] as bool? ?? fallback.remoteAccessEnabled,
      concurrentMode:
          json['concurrent_mode'] as String? ?? fallback.concurrentMode,
      takeoverPolicy:
          json['takeover_policy'] as String? ?? fallback.takeoverPolicy,
      takeoverTimeoutS: (json['takeover_timeout_s'] as num?)?.toInt() ??
          fallback.takeoverTimeoutS,
      permissionTemplate:
          json['permission_template'] as String? ?? fallback.permissionTemplate,
      permissionBits:
          (json['permission_bits'] as num?)?.toInt() ?? fallback.permissionBits,
      localNotice: json['local_notice'] as bool? ?? fallback.localNotice,
      auditRetentionDays: (json['audit_retention_days'] as num?)?.toInt() ??
          fallback.auditRetentionDays,
      localPairPolicy:
          json['local_pair_policy'] as String? ?? fallback.localPairPolicy,
      sameAccountPolicy:
          json['same_account_policy'] as String? ?? fallback.sameAccountPolicy,
      passwordlessPolicy: json['passwordless_policy'] as bool? ??
          fallback.passwordlessPolicy,
    );
  }

  /// 解析策略枚举名（兼容 Rust 的 snake_case 与 Dart 旧文件的 camelCase）。
  static ConnectionPolicy _parsePolicy(String? value) {
    return switch (value) {
      'askThenFallback' || 'ask_then_fallback' => ConnectionPolicy.askThenFallback,
      'autoFallback' || 'auto_fallback' => ConnectionPolicy.autoFallback,
      _ => ConnectionPolicy.strictP2p,
    };
  }

  /// 解析方向枚举名（兼容两种写法）。
  static OrientationPolicy _parseOrientation(String? value) {
    return switch (value) {
      'landscape' => OrientationPolicy.landscape,
      'portrait' => OrientationPolicy.portrait,
      _ => OrientationPolicy.followController,
    };
  }
}

/// 策略枚举转 Rust 端字符串。
String connectionPolicyToWire(ConnectionPolicy policy) {
  return switch (policy) {
    ConnectionPolicy.strictP2p => 'strict_p2p',
    ConnectionPolicy.askThenFallback => 'ask_then_fallback',
    ConnectionPolicy.autoFallback => 'auto_fallback',
  };
}

/// 方向枚举转 Rust 端字符串。
String orientationToWire(OrientationPolicy policy) {
  return switch (policy) {
    OrientationPolicy.followController => 'follow_controller',
    OrientationPolicy.landscape => 'landscape',
    OrientationPolicy.portrait => 'portrait',
  };
}
