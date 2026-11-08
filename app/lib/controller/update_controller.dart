// ============================================================
// 文件用途：软件更新控制器
//
// 层级：controller
// 被谁调用：view/pages/settings_page.dart、main.dart（自动检查）
// 边界：只编排“检查 → 下载 → 重启安装”的状态；网络与验签在 Rust 侧。
// ============================================================

import 'dart:io';

import 'package:flutter/foundation.dart';

import '../core/utils/logger.dart';
import '../model/update/update_service.dart';

/// 更新流程阶段。
enum UpdatePhase {
  /// 空闲（尚未检查）。
  idle,

  /// 正在检查。
  checking,

  /// 已是最新。
  upToDate,

  /// 发现新版本，等待用户确认下载。
  available,

  /// 正在下载并校验。
  downloading,

  /// 已校验完成，等待重启安装。
  ready,

  /// 正在切换到安装进程（应用即将退出）。
  applying,

  /// 失败。
  failed,
}

/// 更新状态快照。
class UpdateState {
  /// 创建状态。
  const UpdateState({
    this.phase = UpdatePhase.idle,
    this.currentVersion = '-',
    this.latestVersion = '',
    this.notes = '',
    this.message = '',
    this.code = '',
    this.check,
    this.staged,
  });

  /// 当前阶段。
  final UpdatePhase phase;

  /// 本端版本。
  final String currentVersion;

  /// 远端版本。
  final String latestVersion;

  /// 更新说明。
  final String notes;

  /// 展示给用户的中文说明。
  final String message;

  /// 错误码（失败时）。
  final String code;

  /// 最近一次检查结果。
  final UpdateCheckResult? check;

  /// 已下载的暂存更新。
  final StagedUpdateInfo? staged;

  /// 是否正在执行耗时操作（按钮应禁用）。
  bool get isBusy =>
      phase == UpdatePhase.checking ||
      phase == UpdatePhase.downloading ||
      phase == UpdatePhase.applying;

  /// 复制并覆盖部分字段。
  UpdateState copyWith({
    UpdatePhase? phase,
    String? currentVersion,
    String? latestVersion,
    String? notes,
    String? message,
    String? code,
    UpdateCheckResult? check,
    StagedUpdateInfo? staged,
  }) {
    return UpdateState(
      phase: phase ?? this.phase,
      currentVersion: currentVersion ?? this.currentVersion,
      latestVersion: latestVersion ?? this.latestVersion,
      notes: notes ?? this.notes,
      message: message ?? this.message,
      code: code ?? this.code,
      check: check ?? this.check,
      staged: staged ?? this.staged,
    );
  }
}

/// 软件更新控制器。
class UpdateController {
  /// 创建控制器。
  ///
  /// [service] 更新服务；[publicKeyConfigured] 表示本构建是否内置发布公钥。
  UpdateController({
    required this.service,
    required this.publicKeyConfigured,
  }) {
    state.value = UpdateState(currentVersion: service.currentVersion);
  }

  /// 更新服务。
  final UpdateService service;

  /// 是否已内置发布公钥（未内置时更新必然被拒绝，界面需提前提示）。
  final bool publicKeyConfigured;

  /// 更新状态。
  final ValueNotifier<UpdateState> state = ValueNotifier(const UpdateState());

  /// 检查更新。
  ///
  /// [silent] 为 `true` 时用于启动期自动检查，不额外提示“已是最新”。
  Future<void> check({bool silent = false}) async {
    if (state.value.isBusy) return;
    state.value = state.value.copyWith(
      phase: UpdatePhase.checking,
      message: '正在检查更新…',
      code: '',
    );
    try {
      // 未配置发布公钥时仍允许检查，但结果会明确提示无法自动安装。
      final result = await service.check();
      if (result.available) {
        state.value = state.value.copyWith(
          phase: UpdatePhase.available,
          currentVersion: result.currentVersion,
          latestVersion: result.latestVersion,
          notes: result.notes,
          message: publicKeyConfigured
              ? '发现新版本 ${result.latestVersion}'
              : '发现新版本 ${result.latestVersion}（本构建缺少发布公钥，无法验证安装包）',
          code: '',
          check: result,
        );
      } else {
        state.value = state.value.copyWith(
          phase: UpdatePhase.upToDate,
          currentVersion: result.currentVersion,
          latestVersion: result.latestVersion,
          check: result,
          code: '',
          message: silent ? '' : '已是最新版本（${result.currentVersion}）',
        );
      }
    } on UpdateException catch (error) {
      state.value = state.value.copyWith(
        phase: UpdatePhase.failed,
        code: error.code,
        message: error.message,
      );
      RdcLogger.log(
        'UpdateController',
        '检查更新失败 ${error.code}：${error.message}',
        level: LogLevel.warn,
      );
    } catch (error, stackTrace) {
      state.value = state.value.copyWith(
        phase: UpdatePhase.failed,
        code: 'RDC-UPD-INTERNAL-001',
        message: '检查更新出现未预期错误：$error',
      );
      RdcLogger.log(
        'UpdateController',
        '检查更新异常',
        level: LogLevel.error,
        error: error,
        stackTrace: stackTrace,
      );
    }
  }

  /// 下载并校验新版本，成功后进入“等待重启安装”。
  Future<void> download() async {
    final check = state.value.check;
    if (check == null || !check.available || state.value.isBusy) return;
    state.value = state.value.copyWith(
      phase: UpdatePhase.downloading,
      message: '正在下载并校验 ${check.latestVersion}…',
      code: '',
    );
    try {
      final staged = await service.download(check.latestVersion);
      state.value = state.value.copyWith(
        phase: UpdatePhase.ready,
        staged: staged,
        message: '${staged.version} 已下载并通过校验，重启应用即可完成安装',
      );
    } on UpdateException catch (error) {
      state.value = state.value.copyWith(
        phase: UpdatePhase.failed,
        code: error.code,
        message: error.message,
      );
      RdcLogger.log(
        'UpdateController',
        '下载更新失败 ${error.code}：${error.message}',
        level: LogLevel.warn,
      );
    }
  }

  /// 启动后台更新进程并退出当前应用。
  ///
  /// 必须先退出应用再安装：Windows 不允许覆盖正在运行的可执行文件。
  /// 更新进程会等待本进程退出后执行安装，失败时自动回滚。
  Future<void> restartAndInstall() async {
    final staged = state.value.staged;
    if (staged == null || state.value.phase != UpdatePhase.ready) return;
    state.value = state.value.copyWith(
      phase: UpdatePhase.applying,
      message: '正在切换到安装进程…',
    );
    final exeDir = File(Platform.resolvedExecutable).parent.path;
    try {
      await Process.start(
        service.updaterPath,
        [
          'apply',
          '--install-dir',
          exeDir,
          '--installer',
          staged.installerPath,
          '--version',
          staged.version,
          '--sha256',
          staged.sha256,
          '--current',
          service.currentVersion,
          '--wait-pid',
          '$pid',
        ],
        mode: ProcessStartMode.detached,
      );
      // 给更新进程一点时间进入等待状态，再退出，避免“应用已退、更新器未起”。
      await Future<void>.delayed(const Duration(milliseconds: 600));
      exit(0);
    } catch (error, stackTrace) {
      state.value = state.value.copyWith(
        phase: UpdatePhase.failed,
        code: 'RDC-UPD-APPLY-002',
        message: '无法启动安装进程：$error（可稍后重试或手动运行安装包）',
      );
      RdcLogger.log(
        'UpdateController',
        '启动安装进程失败',
        level: LogLevel.error,
        error: error,
        stackTrace: stackTrace,
      );
    }
  }

  /// 释放状态监听器。
  void dispose() {
    state.dispose();
  }
}
