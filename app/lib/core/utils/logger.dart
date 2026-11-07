// ============================================================
// 文件用途：极简日志门面
//
// 层级：core（全应用唯一打印入口）
// 被谁调用：controller、model/gateway
// 约束：禁止在页面里直接 print；日志只允许输出错误码与路径，不含画面内容/密钥
// ============================================================

import 'dart:developer' as developer;

import 'package:flutter/foundation.dart';

/// 日志级别。
enum LogLevel { debug, info, warn, error }

/// 统一日志输出。
class RdcLogger {
  const RdcLogger._();

  /// 输出一条日志。
  ///
  /// [tag] 为模块名（如 `RustGateway`）；[message] 为描述；
  /// [error] 与 [stackTrace] 仅在异常路径传入，避免正常日志携带堆栈开销。
  static void log(
    String tag,
    String message, {
    LogLevel level = LogLevel.info,
    Object? error,
    StackTrace? stackTrace,
  }) {
    // 为什么用 dart:developer：Flutter 调试器与 DevTools 可直接过滤级别，且不影响 release 行为。
    developer.log(
      message,
      name: 'rdc.$tag',
      level: switch (level) {
        LogLevel.debug => 500,
        LogLevel.info => 800,
        LogLevel.warn => 900,
        LogLevel.error => 1000,
      },
      error: error,
      stackTrace: stackTrace,
    );
    // debug 构建额外写一条 logcat：`developer.log` 不进 logcat，验收取数（fps/延迟）
    // 与现场排障都需要在 `adb logcat` 里看到同一行；release 不开，避免刷屏。
    if (kDebugMode) {
      debugPrint('[$tag] $message');
    }
  }
}
