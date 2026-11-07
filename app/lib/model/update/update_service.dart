// ============================================================
// 文件用途：软件更新服务（调用随包安装的 rdc-updater.exe）
//
// 层级：model（进程调用与 JSON 解析，不含 UI 状态）
// 被谁调用：controller/update_controller.dart
// 边界：所有网络、验签、哈希、升级、回滚都在 Rust 侧完成；
//       本文件只负责拼接参数、解析输出、把错误码翻译成可读对象。
// ============================================================

import 'dart:async';
import 'dart:convert';
import 'dart:io';

/// 更新操作失败（含固定错误码）。
class UpdateException implements Exception {
  /// 创建异常。
  ///
  /// [code] 为 Rust 侧固定错误码（如 `RDC-UPD-NET-001`）；[message] 为中文说明。
  const UpdateException(this.code, this.message);

  /// 固定错误码。
  final String code;

  /// 中文说明。
  final String message;

  @override
  String toString() => '[$code] $message';
}

/// 检查更新结果。
class UpdateCheckResult {
  /// 创建结果。
  const UpdateCheckResult({
    required this.available,
    required this.currentVersion,
    required this.latestVersion,
    required this.notes,
    required this.installerName,
    required this.installerSha256,
    required this.installerSize,
    required this.downloadUrl,
    required this.minSupportedVersion,
  });

  /// 是否有新版本。
  final bool available;

  /// 本端版本。
  final String currentVersion;

  /// 远端版本。
  final String latestVersion;

  /// 更新说明。
  final String notes;

  /// 安装包文件名。
  final String installerName;

  /// 安装包哈希。
  final String installerSha256;

  /// 安装包大小（字节）。
  final int installerSize;

  /// 下载地址。
  final String downloadUrl;

  /// 最低可升级版本。
  final String minSupportedVersion;

  /// 从 JSON 解析。
  ///
  /// [json] 为 Rust `UpdateCheck` 的序列化结果。返回解析后的对象。
  factory UpdateCheckResult.fromJson(Map<String, dynamic> json) {
    return UpdateCheckResult(
      available: json['available'] as bool? ?? false,
      currentVersion: json['current_version'] as String? ?? '-',
      latestVersion: json['latest_version'] as String? ?? '-',
      notes: json['notes'] as String? ?? '',
      installerName: json['installer_name'] as String? ?? '',
      installerSha256: json['installer_sha256'] as String? ?? '',
      installerSize: (json['installer_size'] as num?)?.toInt() ?? 0,
      downloadUrl: json['download_url'] as String? ?? '',
      minSupportedVersion: json['min_supported_version'] as String? ?? '',
    );
  }
}

/// 已下载并校验的更新。
class StagedUpdateInfo {
  /// 创建结果。
  const StagedUpdateInfo({
    required this.version,
    required this.installerPath,
    required this.sha256,
    required this.size,
    required this.stagingDir,
  });

  /// 目标版本。
  final String version;

  /// 安装包路径。
  final String installerPath;

  /// 校验通过的哈希。
  final String sha256;

  /// 字节数。
  final int size;

  /// 暂存目录。
  final String stagingDir;

  /// 从 JSON 解析。
  ///
  /// [json] 为 Rust `StagedUpdate` 的序列化结果。
  factory StagedUpdateInfo.fromJson(Map<String, dynamic> json) {
    return StagedUpdateInfo(
      version: json['version'] as String? ?? '',
      installerPath: json['installer_path'] as String? ?? '',
      sha256: json['sha256'] as String? ?? '',
      size: (json['size'] as num?)?.toInt() ?? 0,
      stagingDir: json['staging_dir'] as String? ?? '',
    );
  }
}

/// 升级结果。
class ApplyOutcomeInfo {
  /// 创建结果。
  const ApplyOutcomeInfo({
    required this.applied,
    required this.rolledBack,
    required this.code,
    required this.message,
    required this.backupDir,
    required this.oldVersion,
    required this.newVersion,
    required this.installerExitCode,
  });

  /// 是否已升级成功。
  final bool applied;

  /// 是否已回滚。
  final bool rolledBack;

  /// 结果码。
  final String code;

  /// 中文说明。
  final String message;

  /// 备份目录。
  final String backupDir;

  /// 旧版本。
  final String oldVersion;

  /// 目标版本。
  final String newVersion;

  /// 安装器退出码。
  final int installerExitCode;

  /// 从 JSON 解析。
  ///
  /// [json] 为 Rust `ApplyOutcome` 的序列化结果。
  factory ApplyOutcomeInfo.fromJson(Map<String, dynamic> json) {
    return ApplyOutcomeInfo(
      applied: json['applied'] as bool? ?? false,
      rolledBack: json['rolled_back'] as bool? ?? false,
      code: json['code'] as String? ?? '',
      message: json['message'] as String? ?? '',
      backupDir: json['backup_dir'] as String? ?? '',
      oldVersion: json['old_version'] as String? ?? '',
      newVersion: json['new_version'] as String? ?? '',
      installerExitCode: (json['installer_exit_code'] as num?)?.toInt() ?? -1,
    );
  }
}

/// 软件更新服务。
class UpdateService {
  /// 创建服务。
  ///
  /// [updaterPath] 可显式指定 `rdc-updater.exe`（测试用）；默认取 exe 同目录。
  /// [apiUrl] 为 Release API 地址；[currentVersion] 为本端版本。
  UpdateService({
    String? updaterPath,
    required this.apiUrl,
    required this.currentVersion,
  }) : _explicitUpdaterPath = updaterPath;

  /// Release API 地址。
  final String apiUrl;

  /// 本端版本。
  final String currentVersion;

  /// 显式指定的更新器路径。
  final String? _explicitUpdaterPath;

  /// 单次命令超时：下载 200 MiB 在慢网下也可能超过 2 分钟。
  static const Duration _timeout = Duration(minutes: 5);

  /// 解析更新器路径。
  ///
  /// 返回：`RDC_UPDATER_PATH` 环境变量 > 显式构造参数 > exe 同目录 `rdc-updater.exe`。
  String get updaterPath {
    final fromEnv = Platform.environment['RDC_UPDATER_PATH'];
    if (fromEnv != null && fromEnv.trim().isNotEmpty) return fromEnv;
    if (_explicitUpdaterPath != null && _explicitUpdaterPath.isNotEmpty) {
      return _explicitUpdaterPath;
    }
    final exeDir = File(Platform.resolvedExecutable).parent.path;
    final separator = Platform.pathSeparator;
    return '$exeDir${separator}rdc-updater.exe';
  }

  /// 检查更新。
  ///
  /// 返回：Rust 侧验签后的检查结果。
  /// 抛出：[UpdateException] 网络/验签/配置错误。
  Future<UpdateCheckResult> check() async {
    final data = await _run([
      'check',
      '--api',
      apiUrl,
      '--current',
      currentVersion,
    ]);
    return UpdateCheckResult.fromJson(data);
  }

  /// 下载并校验指定版本。
  ///
  /// [version] 必须与已验签清单一致。返回暂存结果。
  Future<StagedUpdateInfo> download(String version) async {
    final data = await _run([
      'download',
      '--api',
      apiUrl,
      '--version',
      version,
    ]);
    return StagedUpdateInfo.fromJson(data);
  }

  /// 执行升级（含备份与失败回滚）。
  ///
  /// [staged] 为已校验的暂存更新；返回升级结论。
  Future<ApplyOutcomeInfo> apply(StagedUpdateInfo staged) async {
    final exeDir = File(Platform.resolvedExecutable).parent.path;
    final data = await _run([
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
      currentVersion,
    ]);
    return ApplyOutcomeInfo.fromJson(data);
  }

  /// 运行更新器并解析单行 JSON。
  ///
  /// [arguments] 为子命令参数；返回 `data` 字段（Map）。
  /// 抛出：[UpdateException] 更新器缺失、超时、输出非法或 `ok=false`。
  Future<Map<String, dynamic>> _run(List<String> arguments) async {
    final executable = updaterPath;
    if (!File(executable).existsSync()) {
      throw UpdateException(
        'RDC-UPD-ENV-001',
        '更新组件缺失：$executable（安装包需包含 rdc-updater.exe）',
      );
    }
    ProcessResult result;
    try {
      result = await Process.run(
        executable,
        arguments,
        stdoutEncoding: utf8,
        stderrEncoding: utf8,
      ).timeout(_timeout);
    } on TimeoutException {
      throw const UpdateException('RDC-UPD-NET-003', '更新命令超时（5 分钟）');
    } on ProcessException catch (error) {
      throw UpdateException(
        'RDC-UPD-ENV-001',
        '无法启动更新组件：${error.message}',
      );
    }

    final stdoutText = (result.stdout as String).trim();
    final line = stdoutText.isEmpty
        ? ''
        : stdoutText.split(RegExp(r'\r?\n')).last.trim();
    Map<String, dynamic>? json;
    if (line.isNotEmpty) {
      try {
        final decoded = jsonDecode(line);
        if (decoded is Map<String, dynamic>) json = decoded;
      } catch (_) {
        // 落到下面统一报“输出非法”，同时保留 stderr 供日志。
      }
    }
    if (json == null) {
      throw UpdateException(
        'RDC-UPD-JSON-001',
        '更新组件输出无法解析（退出码 ${result.exitCode}）：'
        '${(result.stderr as String).trim()}',
      );
    }
    if (json['ok'] == false) {
      throw UpdateException(
        json['code'] as String? ?? 'RDC-UPD-ENV-001',
        json['message'] as String? ?? '更新失败',
      );
    }
    final data = json['data'];
    if (data is Map<String, dynamic>) return data;
    return json;
  }
}
