// ============================================================
// 文件用途：账号页控制器（本地登录态 + 账号服务探测）
//
// 层级：controller
// 被谁调用：view/pages/settings_page.dart
// 边界：只做状态编排与本地持久化；不发起真实登录请求（账号服务未接入），
//      服务可用性用一次 TCP 探测表达，未部署时状态为「未连接」而不是抛异常。
// ============================================================

import 'dart:io';

import 'package:flutter/foundation.dart';

import '../core/utils/logger.dart';
import '../model/account/account_model.dart';
import '../model/account/account_repository.dart';

/// 账号服务可用性。
enum AccountServiceState {
  /// 尚未探测。
  unknown,

  /// 探测成功（端口可连）。
  online,

  /// 未部署 / 连不上。
  offline,
}

/// 账号页状态与行为。
class AccountController {
  /// 创建控制器。
  ///
  /// [repository] 由 main.dart 注入；[endpoint] 仅测试注入。
  AccountController({required this.repository, String? endpoint})
      : _endpoint = endpoint ??
            Platform.environment['RDC_ACCOUNT_ENDPOINT'] ??
            '127.0.0.1:8090';

  /// 本地账号持久化。
  final AccountRepository repository;

  /// 账号服务端点（`host:port`），默认 `RDC_ACCOUNT_ENDPOINT` 或本地 8090。
  final String _endpoint;

  /// 当前登录态与设备发现开关。
  final ValueNotifier<AccountState> state = ValueNotifier(AccountState.empty);

  /// 账号服务可用性。
  final ValueNotifier<AccountServiceState> service =
      ValueNotifier(AccountServiceState.unknown);

  /// 从磁盘加载本地账号状态（刷新后保持）。
  Future<void> load() async {
    try {
      state.value = await repository.load();
    } catch (error, stackTrace) {
      RdcLogger.log(
        'AccountController',
        '加载本地账号状态失败',
        level: LogLevel.error,
        error: error,
        stackTrace: stackTrace,
      );
    }
  }

  /// 探测账号服务。失败只把状态置为「未连接」，不抛异常。
  Future<void> refreshService() async {
    service.value = AccountServiceState.unknown;
    final parts = _endpoint.split(':');
    final host = parts.isNotEmpty && parts.first.isNotEmpty ? parts.first : '127.0.0.1';
    final port = (parts.length > 1 ? int.tryParse(parts[1]) : null) ?? 8090;
    try {
      final socket = await Socket.connect(
        host,
        port,
        timeout: const Duration(milliseconds: 800),
      );
      socket.destroy();
      service.value = AccountServiceState.online;
    } catch (error, stackTrace) {
      service.value = AccountServiceState.offline;
      RdcLogger.log(
        'AccountController',
        '账号服务未连接（$_endpoint）',
        level: LogLevel.warn,
        error: error,
        stackTrace: stackTrace,
      );
    }
  }

  /// 本地登录：记录账号并探测服务。
  ///
  /// 账号服务未部署时登录态仍成立（只是不具备云端能力），界面需如实标注。
  Future<void> signIn(String email) async {
    final trimmed = email.trim();
    if (trimmed.isEmpty) return;
    final name = trimmed.split('@').first;
    state.value = state.value.copyWith(email: trimmed, displayName: name);
    await _persist();
    await refreshService();
  }

  /// 退出登录：清空账号与设备发现开关。
  Future<void> signOut() async {
    state.value = state.value.copyWith(clearIdentity: true);
    await _persist();
  }

  /// 切换「同账号设备发现」；未登录时不生效。
  Future<void> setDiscoveryEnabled(bool value) async {
    if (!state.value.signedIn) return;
    state.value = state.value.copyWith(discoveryEnabled: value);
    await _persist();
  }

  /// 落盘并记录失败日志（不阻塞 UI）。
  Future<void> _persist() async {
    final ok = await repository.save(state.value);
    if (!ok) {
      RdcLogger.log(
        'AccountController',
        '本地账号状态保存失败',
        level: LogLevel.warn,
      );
    }
  }
}
