// ============================================================
// 文件用途：远程访问权限页控制器（13 号文档 7 个区块的状态与持久化）
//
// 层级：controller
// 被谁调用：view/pages/permission_page.dart
// 边界：权限策略写入 Rust 设置文件；信任设备列表来自 Rust 设备列表；
//       授权访问者列表尚无内核接口，页面显示空状态并标注「待接线」
// ============================================================

import 'package:flutter/foundation.dart';

import '../core/l10n/strings.dart';
import '../core/utils/logger.dart';
import '../model/gateway/rust_gateway.dart';
import '../model/gateway/rust_types.dart';
import '../model/settings/app_settings_model.dart';
import '../model/settings/settings_repository.dart';

/// 信任设备行（view 可直接消费）。
class TrustedDeviceRow {
  /// 创建行。
  const TrustedDeviceRow({
    required this.id,
    required this.name,
    required this.sameAccount,
  });

  /// 设备 ID。
  final String id;

  /// 显示名。
  final String name;

  /// 同账号字段。
  final bool sameAccount;
}

/// 远程访问权限状态与行为。
class PermissionController {
  /// 创建控制器。
  ///
  /// [repository] 设置持久化；[gateway] 读信任设备。
  PermissionController({required this.repository, required this.gateway});

  /// 设置仓库。
  final SettingsRepository repository;

  /// FFI 网关。
  final RustGateway gateway;

  /// 当前设置草稿（含权限字段）。
  final ValueNotifier<AppSettings?> settings =
      ValueNotifier<AppSettings?>(null);

  /// 信任设备（真实设备列表过滤）。
  final ValueNotifier<List<TrustedDeviceRow>> trustedDevices =
      ValueNotifier(const []);

  /// 权限内核快照（授权、本机保护区、信任设备、会话、审计）。
  final ValueNotifier<PermissionSnapshot?> kernel =
      ValueNotifier<PermissionSnapshot?>(null);

  /// 内核数据加载中。
  final ValueNotifier<bool> loadingKernel = ValueNotifier(false);

  /// 保存中。
  final ValueNotifier<bool> saving = ValueNotifier(false);

  /// 错误信息。
  final ValueNotifier<String?> errorMessage = ValueNotifier(null);

  /// 权限位名称（按 docs/07 §4 位序，bit0..bit8），顺序固定供 UI 生成。
  static const List<String> permissionBitLabels = [
    '查看屏幕',
    '控制鼠标键盘',
    '文本剪贴板',
    '图片剪贴板',
    '传输文件',
    '音频回传',
    '重启/关机',
    '切换显示器',
    '断线自动重连',
  ];

  /// 当前权限模板对应的位图（readonly 仅查看，standard 到剪贴板，full 全开安全位）。
  ///
  /// `full` 不含 bit6/bit7（电源、切换显示器需逐次本机确认）。
  static const Map<String, int> templateBits = {
    'readonly': 0x01,
    'standard': 0x07,
    'full': 0x13F,
    'custom': 0,
  };

  /// 加载设置与信任设备。
  Future<void> load() async {
    try {
      settings.value = await repository.load();
      errorMessage.value = null;
    } catch (error, stackTrace) {
      errorMessage.value = '读取权限设置失败：$error';
      RdcLogger.log(
        'PermissionController',
        '读取权限设置失败',
        level: LogLevel.error,
        error: error,
        stackTrace: stackTrace,
      );
    }
    await reloadTrustedDevices();
    await loadKernel();
  }

  /// 读取权限内核快照（授权、本机保护区、信任设备、审计）。
  Future<void> loadKernel() async {
    loadingKernel.value = true;
    try {
      kernel.value = await gateway.loadPermissionSnapshot();
    } catch (error, stackTrace) {
      errorMessage.value = '${AppStrings.kernelError}：$error';
      RdcLogger.log(
        'PermissionController',
        '读取权限内核失败',
        level: LogLevel.error,
        error: error,
        stackTrace: stackTrace,
      );
    } finally {
      loadingKernel.value = false;
    }
  }

  /// 为已配对设备签发授权。
  Future<String?> createGrantForDevice({
    required String deviceId,
    required String template,
    required int validityDays,
  }) {
    return _applyKernel(() => gateway.createGrantForDevice(
          deviceId: deviceId,
          template: template,
          validityDays: validityDays,
        ));
  }

  /// 换模板重签（权限提升必须换新令牌）。
  Future<String?> reissueGrant({
    required String grantId,
    required String template,
    required int validityDays,
  }) {
    return _applyKernel(() => gateway.reissueGrant(
          grantId: grantId,
          template: template,
          validityDays: validityDays,
        ));
  }

  /// 吊销授权（最终一致）。
  Future<String?> revokeGrant(String grantId) {
    return _applyKernel(() => gateway.revokeGrant(grantId));
  }

  /// 移除授权记录。
  Future<String?> removeGrant(String grantId) {
    return _applyKernel(() => gateway.removeGrant(grantId));
  }

  /// 立即停止远程访问：停输入 → 断流 → 审计，恢复需二次确认。
  Future<RemoteStopReport?> stopRemoteAccess() async {
    saving.value = true;
    try {
      final report = await gateway.stopRemoteAccess();
      kernel.value = report.snapshot;
      settings.value = await repository.load();
      errorMessage.value = null;
      return report;
    } catch (error) {
      errorMessage.value = '停止远程访问失败：$error';
      return null;
    } finally {
      saving.value = false;
    }
  }

  /// 恢复远程访问（调用方必须先完成本机二次确认）。
  Future<String?> resumeRemoteAccess() async {
    saving.value = true;
    try {
      kernel.value = await gateway.resumeRemoteAccess(confirm: true);
      settings.value = await repository.load();
      errorMessage.value = null;
      return null;
    } catch (error) {
      errorMessage.value = '恢复远程访问失败：$error';
      return error.toString();
    } finally {
      saving.value = false;
    }
  }

  /// 统一执行内核写操作并回填快照。
  Future<String?> _applyKernel(
    Future<PermissionSnapshot> Function() action,
  ) async {
    saving.value = true;
    try {
      kernel.value = await action();
      errorMessage.value = null;
      return null;
    } catch (error) {
      final message = '操作失败：$error';
      errorMessage.value = message;
      return message;
    } finally {
      saving.value = false;
    }
  }

  /// 重新读取信任设备。
  Future<void> reloadTrustedDevices() async {
    try {
      final snapshot = await gateway.loadDevices();
      trustedDevices.value = snapshot.entries
          .where((device) => device.trusted)
          .map((device) => TrustedDeviceRow(
                id: device.deviceId,
                name: device.name,
                sameAccount: device.sameAccount,
              ))
          .toList(growable: false);
    } catch (_) {
      trustedDevices.value = const [];
    }
  }

  /// 取消信任某台设备（写回真实设备列表）。
  ///
  /// 返回：成功返回 `null`，失败返回可展示文案。
  Future<String?> untrust(
    String deviceId, {
    String? name,
    bool sameAccount = false,
  }) async {
    final device = TrustedDeviceRow(
      id: deviceId,
      name: name ?? deviceId,
      sameAccount: sameAccount,
    );
    try {
      await gateway.updateDevice(
        deviceId: device.id,
        name: device.name,
        trusted: false,
        sameAccount: device.sameAccount,
      );
      await reloadTrustedDevices();
      await loadKernel();
      return null;
    } catch (error) {
      return '取消信任失败：$error';
    }
  }

  /// 切换远程访问总开关（默认关）。
  Future<void> setRemoteAccess(bool value) async {
    await _commit((draft) => draft.copyWith(remoteAccessEnabled: value));
  }

  /// 设置并发模式：`exclusive` / `one_control_many_watch`。
  Future<void> setConcurrentMode(String value) async {
    await _commit((draft) => draft.copyWith(concurrentMode: value));
  }

  /// 设置接管策略：`auto` / `confirm` / `forbid`。
  Future<void> setTakeoverPolicy(String value) async {
    await _commit((draft) => draft.copyWith(takeoverPolicy: value));
  }

  /// 设置本机确认超时（秒）。
  Future<void> setTakeoverTimeout(int seconds) async {
    await _commit((draft) => draft.copyWith(takeoverTimeoutS: seconds));
  }

  /// 选择权限模板：`readonly` / `standard` / `full` / `custom`。
  Future<void> setTemplate(String value) async {
    final bits = templateBits[value] ?? 0;
    await _commit((draft) => draft.copyWith(
          permissionTemplate: value,
          permissionBits: value == 'custom' ? draft.permissionBits : bits,
        ));
  }

  /// 切换单个权限位；改动后模板自动变为 `custom`。
  Future<void> togglePermissionBit(int bit) async {
    final current = settings.value;
    if (current == null) return;
    // bit0（查看）是底线，不可取消。
    if (bit == 0) return;
    final mask = 1 << bit;
    final next = current.permissionBits ^ mask;
    await _commit((draft) => draft.copyWith(
          permissionBits: next,
          permissionTemplate: 'custom',
        ));
  }

  /// 切换连接时本机常驻提示。
  Future<void> setLocalNotice(bool value) async {
    await _commit((draft) => draft.copyWith(localNotice: value));
  }

  /// 设置审计保留天数。
  Future<void> setAuditRetention(int days) async {
    await _commit((draft) => draft.copyWith(auditRetentionDays: days));
  }

  /// 设置本地配对策略：`ask` / `auto`。
  Future<void> setLocalPairPolicy(String value) async {
    await _commit((draft) => draft.copyWith(localPairPolicy: value));
  }

  /// 设置同账号策略：`ask` / `auto`。
  Future<void> setSameAccountPolicy(String value) async {
    await _commit((draft) => draft.copyWith(sameAccountPolicy: value));
  }

  /// 切换免密策略。
  Future<void> setPasswordless(bool value) async {
    await _commit((draft) => draft.copyWith(passwordlessPolicy: value));
  }

  /// 保存一份修改后的设置。
  Future<void> _commit(AppSettings Function(AppSettings draft) change) async {
    final current = settings.value;
    if (current == null) return;
    final draft = change(current);
    settings.value = draft;
    saving.value = true;
    try {
      final ok = await repository.save(draft);
      if (!ok) {
        errorMessage.value = '保存失败：设置文件不可写';
        return;
      }
      errorMessage.value = null;
      // Rust 会净化字段，重新读取以保证 UI 与磁盘一致。
      settings.value = await repository.load();
    } catch (error) {
      errorMessage.value = '保存失败：$error';
    } finally {
      saving.value = false;
    }
  }

  /// 释放监听器。
  void dispose() {
    settings.dispose();
    trustedDevices.dispose();
    kernel.dispose();
    loadingKernel.dispose();
    saving.dispose();
    errorMessage.dispose();
  }
}
