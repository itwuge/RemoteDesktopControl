// ============================================================
// 文件用途：设备列表页控制器（搜索/筛选/分页/增删改）
//
// 层级：controller
// 被谁调用：view/pages/device_page.dart
// 边界：设备数据全部来自 Rust 设备列表文件；本控制器不做假数据
// ============================================================

import 'package:flutter/foundation.dart';

import '../core/utils/logger.dart';
import '../model/gateway/rust_gateway.dart';
import '../model/gateway/rust_types.dart';

/// 设备行（view 可直接消费的原始值；不泄漏 model 类型）。
class DeviceRow {
  /// 创建行。
  ///
  /// [id] 设备 ID；[name] 名称；[lastOnline] 最后在线；[trusted] 信任；[sameAccount] 同账号。
  const DeviceRow({
    required this.id,
    required this.name,
    required this.lastOnline,
    required this.trusted,
    required this.sameAccount,
  });

  /// 设备 ID。
  final String id;

  /// 显示名。
  final String name;

  /// 最后在线。
  final DateTime? lastOnline;

  /// 信任。
  final bool trusted;

  /// 同账号。
  final bool sameAccount;
}

/// 设备列表状态与行为。
class DeviceController {
  /// 创建控制器。
  ///
  /// [gateway] 由 main.dart 注入；不可用时列表为空并展示错误。
  DeviceController({required this.gateway});

  /// FFI 网关。
  final RustGateway gateway;

  /// 每页条目数。
  static const int pageSize = 6;

  /// 全部设备（已按设备 ID 排序）。
  final ValueNotifier<List<DeviceRow>> allDevices = ValueNotifier(const []);

  /// 当前页可见设备。
  final ValueNotifier<List<DeviceRow>> visibleDevices = ValueNotifier(const []);

  /// 搜索关键字。
  final ValueNotifier<String> query = ValueNotifier('');

  /// 筛选索引：0 全部，1 仅信任。
  final ValueNotifier<int> filterIndex = ValueNotifier(0);

  /// 当前页码（从 1 开始）。
  final ValueNotifier<int> page = ValueNotifier(1);

  /// 总页数（至少 1，便于空列表也显示分页容器）。
  final ValueNotifier<int> pageCount = ValueNotifier(1);

  /// 是否加载中。
  final ValueNotifier<bool> loading = ValueNotifier(false);

  /// 是否执行中（增删改）。
  final ValueNotifier<bool> busy = ValueNotifier(false);

  /// 错误信息。
  final ValueNotifier<String?> errorMessage = ValueNotifier(null);

  /// 设备文件真实路径。
  final ValueNotifier<String> filePath = ValueNotifier('-');

  /// 从 Rust 读取设备列表。
  ///
  /// 返回：无返回值；结果写入 [allDevices] 并刷新当前页。
  Future<void> load() async {
    if (loading.value) return;
    loading.value = true;
    try {
      final snapshot = await gateway.loadDevices();
      allDevices.value = _toRows(snapshot.entries);
      filePath.value = snapshot.filePath;
      errorMessage.value = null;
      _recompute();
    } catch (error, stackTrace) {
      errorMessage.value = '读取设备列表失败：$error';
      RdcLogger.log(
        'DeviceController',
        '读取设备列表失败',
        level: LogLevel.error,
        error: error,
        stackTrace: stackTrace,
      );
    } finally {
      loading.value = false;
    }
  }

  /// 更新搜索关键字。
  void setQuery(String value) {
    query.value = value;
    page.value = 1;
    _recompute();
  }

  /// 切换筛选（0 全部 / 1 仅信任）。
  void setFilter(int index) {
    filterIndex.value = index;
    page.value = 1;
    _recompute();
  }

  /// 跳到指定页。
  void goToPage(int value) {
    final clamped = value.clamp(1, pageCount.value);
    if (clamped == page.value) return;
    page.value = clamped;
    _recompute();
  }

  /// 新增设备。
  ///
  /// 返回：成功返回 `null`，失败返回可展示的文案。
  Future<String?> addDevice({
    required String deviceId,
    required String name,
    required bool trusted,
  }) async {
    if (deviceId.trim().isEmpty || name.trim().isEmpty) {
      return '设备 ID 与名称不能为空';
    }
    return _mutate(() => gateway.addDevice(
          deviceId: deviceId.trim(),
          name: name.trim(),
          trusted: trusted,
        ));
  }

  /// 修改设备。
  ///
  /// 返回：成功返回 `null`，失败返回可展示的文案。
  Future<String?> updateDevice({
    required String deviceId,
    required String name,
    required bool trusted,
    required bool sameAccount,
  }) async {
    if (name.trim().isEmpty) return '设备名称不能为空';
    return _mutate(() => gateway.updateDevice(
          deviceId: deviceId,
          name: name.trim(),
          trusted: trusted,
          sameAccount: sameAccount,
        ));
  }

  /// 删除设备。
  ///
  /// 返回：成功返回 `null`，失败返回可展示的文案。
  Future<String?> removeDevice(String deviceId) async {
    return _mutate(() => gateway.removeDevice(deviceId));
  }

  /// 执行一次写操作并刷新列表。
  Future<String?> _mutate(Future<DeviceListSnapshot> Function() action) async {
    if (busy.value) return '操作进行中';
    busy.value = true;
    try {
      final snapshot = await action();
      allDevices.value = _toRows(snapshot.entries);
      filePath.value = snapshot.filePath;
      errorMessage.value = null;
      _recompute();
      return null;
    } catch (error, stackTrace) {
      RdcLogger.log(
        'DeviceController',
        '设备写操作失败',
        level: LogLevel.error,
        error: error,
        stackTrace: stackTrace,
      );
      return '操作失败：$error';
    } finally {
      busy.value = false;
    }
  }

  /// 把网关条目转为 view 可直接消费的行。
  List<DeviceRow> _toRows(List<DeviceEntry> entries) {
    return entries
        .map((entry) => DeviceRow(
              id: entry.deviceId,
              name: entry.name,
              lastOnline: entry.lastOnline,
              trusted: entry.trusted,
              sameAccount: entry.sameAccount,
            ))
        .toList(growable: false);
  }

  /// 按搜索词与筛选条件重算当前页。
  void _recompute() {
    final keyword = query.value.trim().toLowerCase();
    final onlyTrusted = filterIndex.value == 1;
    final filtered = allDevices.value.where((device) {
      if (onlyTrusted && !device.trusted) return false;
      if (keyword.isEmpty) return true;
      return device.name.toLowerCase().contains(keyword) ||
          device.id.toLowerCase().contains(keyword);
    }).toList(growable: false);

    final pages = (filtered.length + pageSize - 1) ~/ pageSize;
    pageCount.value = pages < 1 ? 1 : pages;
    if (page.value > pageCount.value) page.value = pageCount.value;
    final start = (page.value - 1) * pageSize;
    final end = (start + pageSize).clamp(0, filtered.length);
    visibleDevices.value = start >= filtered.length
        ? const []
        : filtered.sublist(start, end);
  }

  /// 释放监听器。
  void dispose() {
    allDevices.dispose();
    visibleDevices.dispose();
    query.dispose();
    filterIndex.dispose();
    page.dispose();
    pageCount.dispose();
    loading.dispose();
    busy.dispose();
    errorMessage.dispose();
    filePath.dispose();
  }
}
