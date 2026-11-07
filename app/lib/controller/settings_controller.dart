// ============================================================
// 文件用途：设置页控制器
//
// 层级：controller
// 被谁调用：view/pages/settings_page.dart
// 边界：只做状态编排与持久化调用；确认弹窗等交互留在 view 层
// ============================================================

import 'package:flutter/foundation.dart';

import '../core/l10n/strings.dart';
import '../core/utils/logger.dart';
import '../model/settings/app_settings_model.dart';
import '../model/settings/settings_repository.dart';

/// 设置页状态与行为。
class SettingsController {
  /// 创建控制器。
  ///
  /// [repository] 由 main.dart 注入；本控制器不直接构造文件路径。
  SettingsController({required this.repository});

  /// 设置仓库（model）。
  final SettingsRepository repository;

  /// 当前设置（草稿态，保存后才落盘）。
  final ValueNotifier<AppSettings> settings =
      ValueNotifier(const AppSettings.defaults());

  /// 是否正在保存。
  final ValueNotifier<bool> isSaving = ValueNotifier(false);

  /// 保存结果提示；`null` 表示无提示。
  final ValueNotifier<String?> message = ValueNotifier(null);

  /// 从磁盘加载设置。
  ///
  /// 返回：无返回值；结果写入 [settings]。
  Future<void> load() async {
    try {
      settings.value = await repository.load();
    } catch (error, stackTrace) {
      RdcLogger.log(
        'SettingsController',
        '加载设置失败',
        level: LogLevel.error,
        error: error,
        stackTrace: stackTrace,
      );
    }
  }

  /// 更新中继总开关。
  ///
  /// [value] 为新的开关状态；只改草稿，不落盘。
  void setAllowRelay(bool value) {
    settings.value = settings.value.copyWith(allowRelay: value);
    // 关闭中继时清空地址：避免下次误开启时连到旧地址，也减少敏感配置残留。
    if (!value) {
      settings.value = settings.value.copyWith(relayAddress: '');
    }
  }

  /// 更新中继地址。
  ///
  /// [value] 为用户输入；不做格式校验（S1 不连接中继），仅做去空格。
  void setRelayAddress(String value) {
    settings.value = settings.value.copyWith(relayAddress: value.trim());
  }

  /// 更新打洞失败策略。
  ///
  /// [value] 为新策略；若选择非严格策略但中继未开启，则不改动策略（由 view 先提示）。
  void setConnectionPolicy(ConnectionPolicy value) {
    settings.value = settings.value.copyWith(connectionPolicy: value);
  }

  /// 当前策略在界面上的索引。
  ///
  /// 为什么暴露索引而不是模型枚举：view 层按 09 §3.3 只允许依赖 controller/core/components，
  /// 用 int 把模型类型挡在控制器内部。
  int get policyIndex => settings.value.connectionPolicy.index;

  /// 按界面索引更新策略。
  ///
  /// [index] 必须落在 `ConnectionPolicy.values` 范围内，越界直接忽略。
  void setConnectionPolicyByIndex(int index) {
    if (index < 0 || index >= ConnectionPolicy.values.length) return;
    setConnectionPolicy(ConnectionPolicy.values[index]);
  }

  /// 当前方向策略在界面上的索引。
  int get orientationIndex => settings.value.orientation.index;

  /// 按界面索引更新方向策略。
  ///
  /// [index] 越界直接忽略，避免下拉框异步回调传入非法值。
  void setOrientationByIndex(int index) {
    if (index < 0 || index >= OrientationPolicy.values.length) return;
    setOrientation(OrientationPolicy.values[index]);
  }

  /// 更新方向策略。
  void setOrientation(OrientationPolicy value) {
    settings.value = settings.value.copyWith(orientation: value);
  }

  /// 更新文件传输带宽上限。
  ///
  /// [value] 为 0.1~0.7 的比例；越界值直接钳制，避免写入非法配置。
  void setFileBandwidthRatio(double value) {
    final clamped = value.clamp(0.1, 0.7);
    settings.value = settings.value.copyWith(fileBandwidthRatio: clamped);
  }

  /// 设置主码流分辨率预设。
  ///
  /// [width] / [height] 为偶数；越界交给 Rust 端净化，这里只挡负数。
  void setVideoSize(int width, int height) {
    if (width <= 0 || height <= 0) return;
    settings.value =
        settings.value.copyWith(videoWidth: width, videoHeight: height);
  }

  /// 设置主码流帧率（15/30/60）。
  void setVideoFps(int fps) {
    if (fps <= 0) return;
    settings.value = settings.value.copyWith(videoFps: fps);
  }

  /// 设置主码流码率（Mbps）。
  void setVideoBitrate(double mbps) {
    if (mbps <= 0) return;
    settings.value = settings.value.copyWith(videoBitrateMbps: mbps);
  }

  /// 设置断线自动重连。
  void setAutoReconnect(bool value) {
    settings.value = settings.value.copyWith(autoReconnect: value);
  }

  /// 保存到磁盘。
  ///
  /// 返回：无返回值；结果写入 [message]，供 view 显示 SnackBar。
  Future<void> save() async {
    if (isSaving.value) return;
    isSaving.value = true;
    message.value = null;
    try {
      final ok = await repository.save(settings.value);
      message.value = ok ? AppStrings.saveSuccess : AppStrings.saveFailed;
      RdcLogger.log('SettingsController', '保存设置: ${ok ? '成功' : '失败'}');
    } finally {
      isSaving.value = false;
    }
  }

  /// 释放监听器。
  void dispose() {
    settings.dispose();
    isSaving.dispose();
    message.dispose();
  }
}
