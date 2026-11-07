// ============================================================
// 文件用途：远程控制页控制器（会话角色/权限查询 + 全屏与工具条状态）
//
// 层级：controller
// 被谁调用：view/pages/session_page.dart
// 边界：视频纹理与输入注入尚未接线；页面如实标注「待接线」，不使用假画面
// ============================================================

import 'package:flutter/foundation.dart';

import '../core/utils/logger.dart';
import '../model/gateway/rust_gateway.dart';

/// 远程控制状态与行为。
class SessionController {
  /// 创建控制器。
  ///
  /// [gateway] 由 main.dart 注入。
  SessionController({required this.gateway});

  /// FFI 网关。
  final RustGateway gateway;

  /// 当前角色（controller / viewer）。
  final ValueNotifier<String> role = ValueNotifier('controller');

  /// 当前角色是否可注入输入。
  final ValueNotifier<bool> canInjectInput = ValueNotifier(false);

  /// 当前角色是否可看墙。
  final ValueNotifier<bool> canViewWall = ValueNotifier(false);

  /// 画面区是否全屏。
  final ValueNotifier<bool> fullscreen = ValueNotifier(false);

  /// 错误信息。
  final ValueNotifier<String?> errorMessage = ValueNotifier(null);

  /// 查询角色权限（真实调用 Rust 授权模块）。
  ///
  /// 返回：无返回值；结果写入三个角色通知器。
  Future<void> loadRole() async {
    try {
      final info = await gateway.roleInfo(role.value);
      role.value = info.role;
      canInjectInput.value = info.canInjectInput;
      canViewWall.value = info.canViewWall;
      errorMessage.value = null;
    } catch (error, stackTrace) {
      canInjectInput.value = false;
      canViewWall.value = false;
      errorMessage.value = '角色查询失败：$error';
      RdcLogger.log(
        'SessionController',
        '角色查询失败',
        level: LogLevel.error,
        error: error,
        stackTrace: stackTrace,
      );
    }
  }

  /// 切换全屏。
  void toggleFullscreen() {
    fullscreen.value = !fullscreen.value;
  }

  /// 释放监听器。
  void dispose() {
    role.dispose();
    canInjectInput.dispose();
    canViewWall.dispose();
    fullscreen.dispose();
    errorMessage.dispose();
  }
}
