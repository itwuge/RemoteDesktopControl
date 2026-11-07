// ============================================================
// 文件用途：账号本地状态持久化（`account.json`）
//
// 层级：model（数据持久化边界）
// 被谁调用：controller/account_controller.dart
//
// 说明：只在本地记录登录态与「同账号设备发现」开关，不做任何网络请求；
//      账号服务是否可用由 AccountController 单独探测，读取失败时回退空状态，
//      保证账号服务缺失不会阻止应用启动或影响其他功能。
// ============================================================

import 'dart:convert';
import 'dart:io';

import 'account_model.dart';

/// 账号本地状态读写。
class AccountRepository {
  /// 创建仓库；[baseDirectory] 仅测试注入。
  AccountRepository({this.baseDirectory});

  /// 测试用目录覆盖。
  final String? baseDirectory;

  /// 文件名。
  static const _fileName = 'account.json';

  /// 返回账号文件绝对路径。
  String get filePath => '${_configDirectory()}${Platform.pathSeparator}$_fileName';

  /// 读取账号状态；文件不存在或损坏时返回空状态。
  Future<AccountState> load() async {
    try {
      final file = File(filePath);
      if (!await file.exists()) return AccountState.empty;
      final raw = await file.readAsString();
      final decoded = jsonDecode(raw);
      if (decoded is! Map<String, dynamic>) return AccountState.empty;
      return AccountState.fromJson(decoded);
    } catch (_) {
      return AccountState.empty;
    }
  }

  /// 保存账号状态（临时文件 + rename，避免半截文件）。
  Future<bool> save(AccountState state) async {
    try {
      final dir = Directory(_configDirectory());
      await dir.create(recursive: true);
      final file = File(filePath);
      final temp = File('${file.path}.tmp');
      await temp.writeAsString(
        const JsonEncoder.withIndent('  ').convert(state.toJson()),
        flush: true,
      );
      await temp.rename(file.path);
      return true;
    } catch (_) {
      return false;
    }
  }

  /// 计算平台配置目录（与设置文件同目录）。
  String _configDirectory() {
    final override = baseDirectory;
    if (override != null) return override;

    if (Platform.isWindows) {
      final appData = Platform.environment['APPDATA'];
      if (appData != null && appData.isNotEmpty) {
        return '$appData${Platform.pathSeparator}RDC';
      }
    } else if (Platform.isMacOS) {
      final home = Platform.environment['HOME'];
      if (home != null && home.isNotEmpty) {
        return '$home/Library/Application Support/RDC';
      }
    } else {
      final xdg = Platform.environment['XDG_CONFIG_HOME'];
      if (xdg != null && xdg.isNotEmpty) {
        return '$xdg${Platform.pathSeparator}rdc';
      }
    }
    final temp = Directory.systemTemp.path;
    return '$temp${Platform.pathSeparator}rdc';
  }
}
