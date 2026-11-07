// ============================================================
// 文件用途：远程控制页控制器（角色权限 + 真实画面管线 + 全屏与工具条状态）
//
// 层级：controller
// 被谁调用：view/pages/session_page.dart
// 边界：画面走外部纹理（Texture），不逐帧转 Image；控制器只透传纹理 ID 与状态
// ============================================================

import 'dart:async';

import 'package:flutter/foundation.dart';

import '../core/utils/logger.dart';
import '../model/gateway/rust_gateway.dart';
import '../model/gateway/rust_types.dart';

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

  /// Flutter 外部纹理 ID；`null` 表示尚未接通。
  final ValueNotifier<int?> textureId = ValueNotifier(null);

  /// 控制页实测指标（fps / 延迟 / 丢帧）。
  final ValueNotifier<ControlPreviewInfo?> preview = ValueNotifier(null);

  /// 画面启动失败原因。
  final ValueNotifier<String?> previewError = ValueNotifier(null);

  /// 状态轮询定时器。
  Timer? _statusTimer;

  /// 是否正在启动（防止重复进入）。
  bool _starting = false;

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

  /// 启动真实画面管线：Rust 采集被控端桌面 → 编码 → 解码 → 共享纹理，
  /// 再把共享句柄注册为 Flutter 外部纹理。
  ///
  /// 返回：无返回值；成功后 [textureId] 非空，指标每秒刷新。
  Future<void> startPreview() async {
    if (_starting || textureId.value != null) return;
    _starting = true;
    previewError.value = null;
    try {
      final info = await gateway.startControlPreview();
      final id = await gateway.registerControlTexture(info);
      preview.value = info;
      if (id == null) {
        previewError.value = '外部纹理注册失败（非 Windows 或 runner 未挂载通道）';
        await gateway.stopControlPreview();
      } else {
        textureId.value = id;
        _statusTimer?.cancel();
        _statusTimer = Timer.periodic(
          const Duration(seconds: 1),
          (_) => _refreshStatus(),
        );
      }
    } catch (error, stackTrace) {
      previewError.value = '$error';
      RdcLogger.log(
        'SessionController',
        '真实画面启动失败',
        level: LogLevel.error,
        error: error,
        stackTrace: stackTrace,
      );
    } finally {
      _starting = false;
    }
  }

  /// 停止画面管线：先注销纹理，再停止 Rust（避免句柄提前释放）。
  Future<void> stopPreview() async {
    _statusTimer?.cancel();
    _statusTimer = null;
    try {
      await gateway.unregisterControlTexture();
    } catch (error) {
      RdcLogger.log('SessionController', '注销纹理失败: $error');
    }
    try {
      await gateway.stopControlPreview();
    } catch (error) {
      RdcLogger.log('SessionController', '停止画面管线失败: $error');
    }
    textureId.value = null;
    preview.value = null;
  }

  /// 刷新一次指标。
  Future<void> _refreshStatus() async {
    try {
      preview.value = await gateway.controlPreviewStatus();
    } catch (error) {
      RdcLogger.log('SessionController', '读取画面状态失败: $error');
    }
  }

  /// 切换全屏。
  void toggleFullscreen() {
    fullscreen.value = !fullscreen.value;
  }

  /// 释放监听器与管线。
  void dispose() {
    _statusTimer?.cancel();
    _statusTimer = null;
    // 只做资源回收，不再写通知器（通知器随后即被销毁）。
    unawaited(_teardown());
    role.dispose();
    canInjectInput.dispose();
    canViewWall.dispose();
    fullscreen.dispose();
    errorMessage.dispose();
    textureId.dispose();
    preview.dispose();
    previewError.dispose();
  }

  /// 无通知器写入的管线回收。
  Future<void> _teardown() async {
    try {
      await gateway.unregisterControlTexture();
    } catch (_) {
      // 窗口关闭路径，忽略失败。
    }
    try {
      await gateway.stopControlPreview();
    } catch (_) {
      // 窗口关闭路径，忽略失败。
    }
  }
}
