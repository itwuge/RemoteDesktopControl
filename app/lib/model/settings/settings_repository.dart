// ============================================================
// 文件用途：设置持久化仓库（纯 Dart 文件读写，不依赖插件）
//
// 层级：model（数据持久化边界）
// 被谁调用：controller/settings_controller.dart
//
// 说明：S1 使用 dart:io 直接写入平台配置目录，避免引入 path_provider 插件
//       （本机未开启 Windows 开发者模式，插件工程无法生成符号链接）。
//       移动端目录在 S2 接入正式存储方案，见 docs/决策待办.md。
// ============================================================

import 'dart:convert';
import 'dart:io';

import '../settings/app_settings_model.dart';

/// 设置文件读写。
class SettingsRepository {
  /// 创建仓库；[baseDirectory] 仅测试注入，生产为 `null`。
  SettingsRepository({this.baseDirectory});

  /// 测试用目录覆盖。
  final String? baseDirectory;

  /// 设置文件名。
  static const _fileName = 'settings.json';

  /// 返回设置文件绝对路径。
  String get filePath => '${_configDirectory()}${Platform.pathSeparator}$_fileName';

  /// 读取设置；文件不存在或损坏时回落到默认值。
  ///
  /// 为什么吞掉解析异常：设置损坏不应阻止应用启动，用户可在设置页重新保存覆盖。
  Future<AppSettings> load() async {
    try {
      final file = File(filePath);
      if (!await file.exists()) return const AppSettings.defaults();
      final raw = await file.readAsString();
      final decoded = jsonDecode(raw);
      if (decoded is! Map<String, dynamic>) return const AppSettings.defaults();
      return AppSettings.fromJson(decoded);
    } catch (_) {
      return const AppSettings.defaults();
    }
  }

  /// 保存设置。
  ///
  /// 返回：`true` 表示写入成功；失败时调用方负责提示用户。
  /// 实现：先写临时文件再原子替换，避免断电/崩溃留下半截 JSON。
  Future<bool> save(AppSettings settings) async {
    try {
      final dir = Directory(_configDirectory());
      await dir.create(recursive: true);
      final file = File(filePath);
      final temp = File('${file.path}.tmp');
      await temp.writeAsString(
        const JsonEncoder.withIndent('  ').convert(settings.toJson()),
        flush: true,
      );
      await temp.rename(file.path);
      return true;
    } catch (_) {
      return false;
    }
  }

  /// 计算平台配置目录。
  ///
  /// Windows 用 `%APPDATA%\RDC`；macOS 用 `~/Library/Application Support/RDC`；
  /// Linux 用 `$XDG_CONFIG_HOME/rdc`；移动端（无 HOME）临时用系统临时目录，
  /// S2 接入 path_provider 后替换。
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
    } else if (Platform.isLinux) {
      final xdg = Platform.environment['XDG_CONFIG_HOME'];
      final home = Platform.environment['HOME'];
      if (xdg != null && xdg.isNotEmpty) {
        return '$xdg/rdc';
      }
      if (home != null && home.isNotEmpty) {
        return '$home/.config/rdc';
      }
    }

    // 移动端没有可用的标准环境变量；S1 只保证可写，不保证跨重启持久。
    return '${Directory.systemTemp.path}${Platform.pathSeparator}rdc';
  }
}
