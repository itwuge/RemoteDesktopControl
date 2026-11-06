// ============================================================
// 文件用途：FFI 返回值对应的 Dart 值对象
//
// 层级：model（跨层数据契约，不含逻辑）
// 被谁调用：model/gateway/rust_gateway.dart、controller/home_controller.dart
// ============================================================

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
