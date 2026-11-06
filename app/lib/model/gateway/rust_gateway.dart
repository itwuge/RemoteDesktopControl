// ============================================================
// 文件用途：Rust FFI 网关（全应用唯一允许 import 生成代码的地方）
//
// 层级：model（数据来源层，负责把 Rust 能力转成 Dart 值对象）
// 被谁调用：controller/home_controller.dart、controller/settings_controller.dart
// 边界：不得包含任何 UI 逻辑；不得把生成代码的类型泄漏到 view 层
// ============================================================

import 'dart:io';

import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart';

import '../../core/utils/logger.dart';
import '../../src/rust/api.dart' as rust_api;
import '../../src/rust/frb_generated.dart';
import 'rust_types.dart';

/// Rust 核心的唯一调用入口。
///
/// 线程模型：所有方法都是异步的；生成代码内部把调用投递到 Rust 的线程池，
/// 不会阻塞 Flutter UI 线程（docs/01 §4.1）。
class RustGateway {
  /// 创建网关实例。
  ///
  /// [libraryOverridePath] 仅用于测试或非常规安装布局；生产环境保持 `null`，
  /// 由 [_libraryCandidates] 自动探测。
  RustGateway({this.libraryOverridePath});

  /// 测试/特殊布局下的显式动态库路径。
  final String? libraryOverridePath;

  /// 是否已完成初始化。
  bool _initialized = false;

  /// 返回是否已完成初始化。
  bool get isInitialized => _initialized;

  // =========** 初始化 =========

  /// 初始化 flutter_rust_bridge 并装载 Rust 动态库。
  ///
  /// 失败路径：先按候选路径逐个尝试 `DynamicLibrary.open`；全部不存在时
  /// 退回 FRB 默认加载器（适合 Android/iOS 由系统按包内路径装载）。
  ///
  /// 抛出：动态库缺失或 ABI 不匹配时抛出 [RustLibException]，由 main.dart 捕获并展示。
  Future<void> initialize() async {
    if (_initialized) return;

    final overridePath = libraryOverridePath;
    if (overridePath != null) {
      await RustLib.init(externalLibrary: ExternalLibrary.open(overridePath));
      _initialized = true;
      return;
    }

    // 为什么先探测文件：打包后的桌面应用工作目录不固定，FRB 生成的相对路径
    // （../crates/rdc-bridge/target/release/）只在开发期有效，生产必须按安装布局查找。
    for (final path in _libraryCandidates()) {
      if (File(path).existsSync()) {
        await RustLib.init(externalLibrary: ExternalLibrary.open(path));
        _initialized = true;
        RdcLogger.log('RustGateway', '动态库已装载: $path');
        return;
      }
    }

    // Android/iOS：动态库随 APK/IPA 打包，系统按库名检索，无需绝对路径。
    await RustLib.init();
    _initialized = true;
    RdcLogger.log('RustGateway', '动态库由默认加载器装载');
  }

  /// 按平台列出动态库候选路径。
  ///
  /// 顺序：可执行文件同目录 → macOS Frameworks → 开发期 cargo target 目录。
  List<String> _libraryCandidates() {
    final exeDir = File(Platform.resolvedExecutable).parent.path;
    final sep = Platform.pathSeparator;
    final fileName = _libraryFileName();

    final candidates = <String>[
      if (exeDir.isNotEmpty) '$exeDir$sep$fileName',
    ];

    if (Platform.isMacOS) {
      // macOS 应用包规范：动态库放 Contents/Frameworks。
      candidates.add('$exeDir$sep..${sep}Frameworks$sep$fileName');
    }

    // 开发期回退：从 app 目录启动时 cargo 产物位置（仓库相对布局）。
    candidates.add(
      '$exeDir$sep..${sep}crates${sep}rdc-bridge${sep}target${sep}release$sep$fileName',
    );

    return candidates;
  }

  /// 返回当前平台的动态库文件名。
  String _libraryFileName() {
    if (Platform.isWindows) return 'rdc_bridge.dll';
    if (Platform.isMacOS) return 'librdc_bridge.dylib';
    return 'librdc_bridge.so';
  }

  // =========** 命令调用 =========

  /// 读取 Rust 核心版本与平台信息。
  ///
  /// 返回：[CoreInfo]，桥接未初始化时抛出 [StateError]。
  Future<CoreInfo> readCoreInfo() async {
    _ensureInitialized();
    // 三个调用互不依赖，并行执行以缩短首页加载时间。
    final results = await Future.wait([
      rust_api.coreVersion(),
      rust_api.platformName(),
      rust_api.bridgeStatus(),
    ]);
    return CoreInfo(
      coreVersion: results[0],
      platform: results[1],
      bridgeStatus: results[2],
    );
  }

  /// 读取 5 行环境自检结果。
  ///
  /// 返回：`[OK] 名称: 详情` 文本列表，顺序与 Rust 端一致。
  Future<List<String>> runSelfCheck() async {
    _ensureInitialized();
    return rust_api.doctorReport();
  }

  /// 断言桥接已初始化。
  ///
  /// 抛出：[StateError] 当调用方在 init 之前发起命令（属编程错误，应在 main 中避免）。
  void _ensureInitialized() {
    if (!_initialized) {
      throw StateError('RustGateway 尚未初始化');
    }
  }
}
