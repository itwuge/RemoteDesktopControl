// ============================================================
// 文件用途：屏幕墙页控制器（启动子码流预览、轮询最新帧、宫格切换）
//
// 层级：controller
// 被谁调用：view/pages/wall_page.dart
// 边界：预览帧来自 Rust 本机回环管线（真实 848x480/10fps/400kbps 缩略流），
//       不是占位图；跨机会话尚未接线，页面需如实标注
// ============================================================

import 'dart:async';

import 'package:flutter/foundation.dart';

import '../core/utils/logger.dart';
import '../model/gateway/rust_gateway.dart';
import '../model/gateway/rust_types.dart';

/// 屏幕墙状态与行为。
class WallController {
  /// 创建控制器。
  ///
  /// [gateway] 由 main.dart 注入。
  WallController({required this.gateway});

  /// FFI 网关。
  final RustGateway gateway;

  /// 宫格选项：1/2/4/6 路。
  static const List<int> gridOptions = [1, 2, 4, 6];

  /// 最新一帧（RGBA）。
  final ValueNotifier<WallPreviewFrame?> latest =
      ValueNotifier<WallPreviewFrame?>(null);

  /// 服务状态。
  final ValueNotifier<WallStatusInfo?> status =
      ValueNotifier<WallStatusInfo?>(null);

  /// 是否启动中。
  final ValueNotifier<bool> starting = ValueNotifier(false);

  /// 错误信息。
  final ValueNotifier<String?> errorMessage = ValueNotifier(null);

  /// 宫格索引（对应 [gridOptions]）。
  final ValueNotifier<int> gridIndex = ValueNotifier(0);

  /// 最近设备名（用于宫格标签；真实设备列表，可为空）。
  final ValueNotifier<List<String>> deviceNames = ValueNotifier(const []);

  /// 轮询定时器。
  Timer? _timer;

  /// 防止轮询重入。
  bool _polling = false;

  /// 最近一次帧号，用于只在有新帧时通知 UI。
  int _lastFrameId = -1;

  /// 轮询计数，用于降低状态读取频率。
  int _pollTicks = 0;

  /// 是否运行中。
  bool get isRunning => status.value?.running ?? false;

  /// 当前宫格数。
  int get gridCount => gridOptions[gridIndex.value];

  /// 启动预览并加载设备名。
  ///
  /// 返回：无返回值；失败写入 [errorMessage]。
  Future<void> start() async {
    if (starting.value || isRunning) return;
    starting.value = true;
    try {
      status.value = await gateway.startWall();
      errorMessage.value = null;
      _timer?.cancel();
      _timer = Timer.periodic(
        const Duration(milliseconds: 100),
        (_) => _poll(),
      );
    } catch (error, stackTrace) {
      errorMessage.value = '屏幕墙启动失败：$error';
      RdcLogger.log(
        'WallController',
        '屏幕墙启动失败',
        level: LogLevel.error,
        error: error,
        stackTrace: stackTrace,
      );
    } finally {
      starting.value = false;
    }
    await loadDeviceNames();
  }

  /// 停止预览。
  Future<void> stop() async {
    _timer?.cancel();
    _timer = null;
    try {
      status.value = await gateway.stopWall();
    } catch (error) {
      RdcLogger.log('WallController', '屏幕墙停止失败：$error', level: LogLevel.error);
    }
    latest.value = null;
    _lastFrameId = -1;
  }

  /// 读取设备名（只用于宫格标签，不伪造在线状态）。
  Future<void> loadDeviceNames() async {
    try {
      final snapshot = await gateway.loadDevices();
      deviceNames.value =
          snapshot.entries.map((entry) => entry.name).toList(growable: false);
    } catch (_) {
      deviceNames.value = const [];
    }
  }

  /// 切换宫格。
  void setGrid(int index) {
    if (index < 0 || index >= gridOptions.length) return;
    gridIndex.value = index;
  }

  /// 轮询最新帧与状态。
  Future<void> _poll() async {
    if (_polling) return;
    _polling = true;
    _pollTicks += 1;
    try {
      final frame = await gateway.wallLatest();
      if (frame != null && frame.frameId != _lastFrameId) {
        _lastFrameId = frame.frameId;
        latest.value = frame;
      }
      // 状态每 5 次轮询刷新一次（500 ms），减少 FFI 往返。
      if (_pollTicks % 5 == 0 || status.value == null) {
        status.value = await gateway.wallStatus();
      }
      if (status.value?.lastError != null) {
        errorMessage.value = status.value!.lastError;
      }
    } catch (error) {
      errorMessage.value = '预览读取失败：$error';
    } finally {
      _polling = false;
    }
  }

  /// 释放监听器与定时器。
  void dispose() {
    _timer?.cancel();
    _timer = null;
    latest.dispose();
    status.dispose();
    starting.dispose();
    errorMessage.dispose();
    gridIndex.dispose();
    deviceNames.dispose();
  }
}
