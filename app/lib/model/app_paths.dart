// ============================================================
// 文件用途：配置目录解析（与 Rust `rdc_core::paths` 保持同一规则）
//
// 层级：model（纯路径计算，不做读写）
// 被谁调用：model/settings/settings_repository.dart、
//          model/account/account_repository.dart
//
// 规则（Windows）：
// 1. 环境变量 `RDC_CONFIG_DIR` 优先；
// 2. 其次 `<exe 同目录>\config`（安装后即 `C:\Program Files\RDC\config`）；
// 3. 不可写时回退 `%APPDATA%\RDC`；
// 4. 失败兜底系统临时目录。
// 为什么 Dart 也要实现一份：桥接失败时设置/账号仍需可读，规则必须与 Rust 完全一致。
// ============================================================

import 'dart:io';

/// 配置文件所在目录的解析器。
class AppPaths {
  /// 私有构造：只暴露静态方法。
  AppPaths._();

  /// 返回当前生效的配置目录。
  ///
  /// 参数 [baseDirectory] 仅测试注入；[executablePath] 仅测试注入。
  /// 返回：已尝试创建且可写的目录；无法创建时返回旧目录或临时目录。
  static String configDirectory({
    String? baseDirectory,
    String? executablePath,
  }) {
    if (baseDirectory != null) return baseDirectory;

    final override = Platform.environment['RDC_CONFIG_DIR'];
    if (override != null && override.trim().isNotEmpty) {
      return override;
    }

    if (Platform.isWindows) {
      final exe = executablePath ?? Platform.resolvedExecutable;
      final exeDir = File(exe).parent.path;
      if (exeDir.isNotEmpty) {
        final candidate = '$exeDir${Platform.pathSeparator}config';
        if (_ensureWritable(candidate)) return candidate;
      }
      final appData = Platform.environment['APPDATA'];
      if (appData != null && appData.isNotEmpty) {
        final legacy = '$appData${Platform.pathSeparator}RDC';
        if (_ensureWritable(legacy)) return legacy;
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

    // 移动端与极端环境：只保证可写，不保证跨重启持久。
    return '${Directory.systemTemp.path}${Platform.pathSeparator}rdc';
  }

  /// 确保目录存在且可写（用一次性探针文件验证真实权限）。
  ///
  /// [path] 为目标目录。返回 `true` 表示已创建且可写。
  static bool _ensureWritable(String path) {
    try {
      final dir = Directory(path);
      if (!dir.existsSync()) {
        dir.createSync(recursive: true);
      }
      final probe = File(
        '$path${Platform.pathSeparator}.rdc-write-test-$pid',
      );
      probe.writeAsStringSync('ok', flush: true);
      probe.deleteSync();
      return true;
    } catch (_) {
      return false;
    }
  }
}
