// ============================================================
// 文件用途：应用设置模型与序列化
//
// 层级：model（唯一设置真源；不直接读写文件）
// 被谁调用：model/settings/settings_repository.dart、controller/settings_controller.dart
// 边界：字段默认值必须与 docs/06 一致；新增字段要同步 fromJson/toJson
// ============================================================

/// 打洞失败后的策略（与 Rust `ConnectionPolicy` 一一对应）。
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

/// 应用设置。
class AppSettings {
  /// 创建设置快照。
  const AppSettings({
    required this.allowRelay,
    required this.relayAddress,
    required this.connectionPolicy,
    required this.orientation,
    required this.fileBandwidthRatio,
  });

  /// 默认设置：默认不允许中继、严格 P2P、跟随主控端、文件上限 40%。
  const AppSettings.defaults()
      : allowRelay = false,
        relayAddress = '',
        connectionPolicy = ConnectionPolicy.strictP2p,
        orientation = OrientationPolicy.followController,
        fileBandwidthRatio = 0.4;

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

  /// 复制一份并覆盖部分字段。
  ///
  /// 参数均为可选覆盖项；未传字段保持原值。
  AppSettings copyWith({
    bool? allowRelay,
    String? relayAddress,
    ConnectionPolicy? connectionPolicy,
    OrientationPolicy? orientation,
    double? fileBandwidthRatio,
  }) {
    return AppSettings(
      allowRelay: allowRelay ?? this.allowRelay,
      relayAddress: relayAddress ?? this.relayAddress,
      connectionPolicy: connectionPolicy ?? this.connectionPolicy,
      orientation: orientation ?? this.orientation,
      fileBandwidthRatio: fileBandwidthRatio ?? this.fileBandwidthRatio,
    );
  }

  /// 序列化为可写入磁盘的 JSON 结构。
  Map<String, dynamic> toJson() => {
        'allow_relay': allowRelay,
        'relay_address': relayAddress,
        'connection_policy': connectionPolicy.name,
        'orientation': orientation.name,
        'file_bandwidth_ratio': fileBandwidthRatio,
      };

  /// 从磁盘 JSON 还原；缺字段或非法值一律回落到默认值，避免旧配置让应用崩在启动路径。
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
    );
  }

  /// 解析策略枚举名。
  static ConnectionPolicy _parsePolicy(String? value) {
    return switch (value) {
      'askThenFallback' => ConnectionPolicy.askThenFallback,
      'autoFallback' => ConnectionPolicy.autoFallback,
      _ => ConnectionPolicy.strictP2p,
    };
  }

  /// 解析方向枚举名。
  static OrientationPolicy _parseOrientation(String? value) {
    return switch (value) {
      'landscape' => OrientationPolicy.landscape,
      'portrait' => OrientationPolicy.portrait,
      _ => OrientationPolicy.followController,
    };
  }
}
