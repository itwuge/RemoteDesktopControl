// ============================================================
// 文件用途：主界面壳控制器（顶部状态 + 四个功能区切换 + 当前设备）
//
// 层级：controller（唯一连接 model 与 view 的桥）
// 被谁调用：view/pages/shell_page.dart、device_page.dart、wall_page.dart
// 边界：不持有 Widget；对 view 只暴露原始值与回调，不泄漏 model 枚举
// ============================================================

import 'dart:io';

import 'package:flutter/foundation.dart';

import '../core/utils/logger.dart';
import '../model/gateway/rust_gateway.dart';

/// 主界面功能区索引（0 远程控制 / 1 设备列表 / 2 屏幕墙 / 3 设置）。
class ShellTab {
  const ShellTab._();

  /// 远程控制。
  static const int control = 0;

  /// 设备列表。
  static const int devices = 1;

  /// 屏幕墙。
  static const int wall = 2;

  /// 设置。
  static const int settings = 3;

  /// 入口数量。
  static const int count = 4;
}

/// 主界面状态与行为。
class ShellController {
  /// 创建控制器。
  ///
  /// [gateway] 由 main.dart 构造并完成初始化。
  ShellController({required this.gateway});

  /// FFI 网关。
  final RustGateway gateway;

  /// 当前功能区索引。
  final ValueNotifier<int> activeTab = ValueNotifier(ShellTab.control);

  /// 顶部状态：核心版本。
  final ValueNotifier<String> coreVersion = ValueNotifier('-');

  /// 顶部状态：构建平台。
  final ValueNotifier<String> platform = ValueNotifier('-');

  /// 顶部状态：桥接是否正常。
  final ValueNotifier<bool> bridgeHealthy = ValueNotifier(false);

  /// 顶部状态：本机名。
  final ValueNotifier<String> hostName = ValueNotifier('-');

  /// 顶部状态：最近错误。
  final ValueNotifier<String?> errorMessage = ValueNotifier(null);

  /// 当前进入控制的目标设备 ID。
  final ValueNotifier<String?> activeDeviceId = ValueNotifier(null);

  /// 当前进入控制的目标设备名。
  final ValueNotifier<String> activeDeviceName = ValueNotifier('本机回环预览');

  /// 当前进入控制的是否本机预览（没有真实设备时）。
  final ValueNotifier<bool> isLocalPreview = ValueNotifier(true);

  /// 加载顶部状态（版本/平台/桥接/主机名）。
  ///
  /// 返回：无返回值；失败写入 [errorMessage]。
  Future<void> loadStatus() async {
    hostName.value = Platform.localHostname;
    try {
      final info = await gateway.readCoreInfo();
      coreVersion.value = info.coreVersion;
      platform.value = info.platform;
      bridgeHealthy.value = info.isBridgeHealthy;
      errorMessage.value = null;
    } catch (error, stackTrace) {
      bridgeHealthy.value = false;
      errorMessage.value = 'Rust 核心加载失败：$error';
      RdcLogger.log(
        'ShellController',
        '读取核心信息失败',
        level: LogLevel.error,
        error: error,
        stackTrace: stackTrace,
      );
    }
  }

  /// 切换功能区。
  ///
  /// [index] 必须落在 0..3，越界忽略。
  void selectTab(int index) {
    if (index < 0 || index >= ShellTab.count) return;
    activeTab.value = index;
  }

  /// 从设备列表/屏幕墙进入远程控制。
  ///
  /// [deviceId] 为空表示本机预览；[deviceName] 用于标题栏。
  void openControl({String? deviceId, required String deviceName}) {
    activeDeviceId.value = deviceId;
    activeDeviceName.value = deviceName;
    isLocalPreview.value = deviceId == null;
    activeTab.value = ShellTab.control;
  }

  /// 断开并回到设备列表。
  void disconnectToDevices() {
    activeTab.value = ShellTab.devices;
  }

  /// 释放监听器。
  void dispose() {
    activeTab.dispose();
    coreVersion.dispose();
    platform.dispose();
    bridgeHealthy.dispose();
    hostName.dispose();
    errorMessage.dispose();
    activeDeviceId.dispose();
    activeDeviceName.dispose();
    isLocalPreview.dispose();
  }
}
