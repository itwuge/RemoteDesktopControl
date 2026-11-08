// ============================================================
// 文件用途：设置持久化仓库（优先走 Rust 原子写，Dart 文件写作为回退）
//
// 层级：model（数据持久化边界）
// 被谁调用：controller/settings_controller.dart、controller/permission_controller.dart
//
// 说明：S5-2 起设置读写经 FRB 收口到 Rust（rdc-core `s5::settings`，临时文件 + rename）；
//       Rust 不可用时仍用 dart:io 写同一路径，保证应用不会因桥接故障丢失设置。
// ============================================================

import 'dart:convert';
import 'dart:io';

import '../app_paths.dart';
import '../gateway/rust_gateway.dart';
import '../gateway/rust_types.dart';
import '../settings/app_settings_model.dart';

/// 设置文件读写。
class SettingsRepository {
  /// 创建仓库；[baseDirectory] 仅测试注入，[gateway] 为生产 FFI 通道。
  SettingsRepository({this.baseDirectory, this.gateway});

  /// 测试用目录覆盖。
  final String? baseDirectory;

  /// Rust 设置读写通道；初始化前或测试中为 `null`。
  final RustGateway? gateway;

  /// 设置文件名。
  static const _fileName = 'settings.json';

  /// 返回设置文件绝对路径。
  String get filePath => '${_configDirectory()}${Platform.pathSeparator}$_fileName';

  /// 读取设置。
  ///
  /// 优先调用 Rust（原子读，字段净化）；桥接不可用时回落到 Dart 文件读取。
  /// 文件不存在或损坏时回落到默认值，避免设置损坏阻止应用启动。
  Future<AppSettings> load() async {
    final rust = gateway;
    if (rust != null && rust.isInitialized) {
      try {
        return _fromSnapshot(await rust.loadSettings());
      } catch (_) {
        // 桥接异常不阻塞启动，继续走本地文件。
      }
    }
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
  /// 优先 Rust 原子写；失败则回退 Dart 的「临时文件 + rename」，两者路径一致。
  Future<bool> save(AppSettings settings) async {
    final rust = gateway;
    if (rust != null && rust.isInitialized) {
      try {
        await rust.saveSettings(_toSnapshot(settings, filePath));
        return true;
      } catch (_) {
        // 继续走 Dart 回退。
      }
    }
    try {
      final dir = Directory(_configDirectory());
      await dir.create(recursive: true);
      final file = File(filePath);
      final temp = File('${file.path}.tmp');
      try {
        await temp.writeAsString(
          const JsonEncoder.withIndent('  ').convert(settings.toJson()),
          flush: true,
        );
        await temp.rename(file.path);
        return true;
      } catch (_) {
        // 写入或 rename 失败：删掉本次自己创建的临时文件，不留给下次启动时清理。
        if (await temp.exists()) {
          try {
            await temp.delete();
          } catch (_) {
            // 清理失败不掩盖「保存失败」这个事实。
          }
        }
        return false;
      }
    } catch (_) {
      return false;
    }
  }

  /// Rust 快照转设置模型。
  AppSettings _fromSnapshot(SettingsSnapshot snapshot) {
    return AppSettings(
      allowRelay: snapshot.allowRelay,
      relayAddress: snapshot.relayAddress,
      connectionPolicy: switch (snapshot.connectionPolicy) {
        'ask_then_fallback' => ConnectionPolicy.askThenFallback,
        'auto_fallback' => ConnectionPolicy.autoFallback,
        _ => ConnectionPolicy.strictP2p,
      },
      orientation: switch (snapshot.orientation) {
        'landscape' => OrientationPolicy.landscape,
        'portrait' => OrientationPolicy.portrait,
        _ => OrientationPolicy.followController,
      },
      fileBandwidthRatio: snapshot.fileBandwidthRatio,
      videoWidth: snapshot.videoWidth,
      videoHeight: snapshot.videoHeight,
      videoFps: snapshot.videoFps,
      videoBitrateMbps: snapshot.videoBitrateMbps,
      autoReconnect: snapshot.autoReconnect,
      remoteAccessEnabled: snapshot.remoteAccessEnabled,
      concurrentMode: snapshot.concurrentMode,
      takeoverPolicy: snapshot.takeoverPolicy,
      takeoverTimeoutS: snapshot.takeoverTimeoutS,
      permissionTemplate: snapshot.permissionTemplate,
      permissionBits: snapshot.permissionBits,
      localNotice: snapshot.localNotice,
      auditRetentionDays: snapshot.auditRetentionDays,
      localPairPolicy: snapshot.localPairPolicy,
      sameAccountPolicy: snapshot.sameAccountPolicy,
      passwordlessPolicy: snapshot.passwordlessPolicy,
      firstRunCompleted: snapshot.firstRunCompleted,
      autoUpdateCheck: snapshot.autoUpdateCheck,
    );
  }

  /// 设置模型转 Rust 快照。
  SettingsSnapshot _toSnapshot(AppSettings settings, String path) {
    return SettingsSnapshot(
      allowRelay: settings.allowRelay,
      relayAddress: settings.relayAddress,
      connectionPolicy: connectionPolicyToWire(settings.connectionPolicy),
      orientation: orientationToWire(settings.orientation),
      fileBandwidthRatio: settings.fileBandwidthRatio,
      videoWidth: settings.videoWidth,
      videoHeight: settings.videoHeight,
      videoFps: settings.videoFps,
      videoBitrateMbps: settings.videoBitrateMbps,
      autoReconnect: settings.autoReconnect,
      remoteAccessEnabled: settings.remoteAccessEnabled,
      concurrentMode: settings.concurrentMode,
      takeoverPolicy: settings.takeoverPolicy,
      takeoverTimeoutS: settings.takeoverTimeoutS,
      permissionTemplate: settings.permissionTemplate,
      permissionBits: settings.permissionBits,
      localNotice: settings.localNotice,
      auditRetentionDays: settings.auditRetentionDays,
      localPairPolicy: settings.localPairPolicy,
      sameAccountPolicy: settings.sameAccountPolicy,
      passwordlessPolicy: settings.passwordlessPolicy,
      firstRunCompleted: settings.firstRunCompleted,
      autoUpdateCheck: settings.autoUpdateCheck,
      filePath: path,
    );
  }

  /// 计算平台配置目录。
  ///
  /// 规则与 Rust `rdc_core::paths` 一致：Windows 用 `<安装目录>\config`，
  /// 不可写时回退 `%APPDATA%\RDC`；macOS/Linux 沿用原位置。
  String _configDirectory() {
    return AppPaths.configDirectory(baseDirectory: baseDirectory);
  }
}
