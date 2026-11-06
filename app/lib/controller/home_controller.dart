// ============================================================
// 文件用途：首页控制器
//
// 层级：controller（唯一连接 model 与 view 的桥）
// 被谁调用：view/pages/home_page.dart
// 边界：不持有任何 Widget；不直接 print；所有状态通过 ValueNotifier 暴露
// ============================================================

import 'package:flutter/foundation.dart';

import '../core/utils/logger.dart';
import '../model/gateway/rust_gateway.dart';
import '../model/gateway/rust_types.dart';

/// 首页状态与行为。
class HomeController {
  /// 创建控制器。
  ///
  /// [gateway] 由 main.dart 构造并完成初始化；本控制器不负责装载动态库。
  HomeController({required this.gateway});

  /// FFI 网关（model）。
  final RustGateway gateway;

  /// 核心信息（版本/平台/桥接状态）。
  final ValueNotifier<CoreInfo> coreInfo = ValueNotifier(const CoreInfo.empty());

  /// 5 行自检结果；空列表表示尚未运行。
  final ValueNotifier<List<String>> selfCheckLines = ValueNotifier(const []);

  /// 自检是否进行中。
  final ValueNotifier<bool> isChecking = ValueNotifier(false);

  /// 最近一次错误（展示在首页横幅）。
  final ValueNotifier<String?> errorMessage = ValueNotifier(null);

  /// 加载版本与桥接信息。
  ///
  /// 返回：无返回值；结果写入 [coreInfo]，失败写入 [errorMessage]。
  Future<void> loadCoreInfo() async {
    try {
      coreInfo.value = await gateway.readCoreInfo();
      errorMessage.value = null;
    } catch (error, stackTrace) {
      // 失败不抛出：首页需要保持可用并展示可读错误，而不是白屏。
      errorMessage.value = '无法读取 Rust 核心信息：$error';
      RdcLogger.log(
        'HomeController',
        '读取核心信息失败',
        level: LogLevel.error,
        error: error,
        stackTrace: stackTrace,
      );
    }
  }

  /// 运行环境自检。
  ///
  /// 返回：无返回值；结果写入 [selfCheckLines]。
  Future<void> runSelfCheck() async {
    if (isChecking.value) return;
    isChecking.value = true;
    try {
      selfCheckLines.value = await gateway.runSelfCheck();
      errorMessage.value = null;
    } catch (error, stackTrace) {
      errorMessage.value = '自检执行失败：$error';
      RdcLogger.log(
        'HomeController',
        '自检失败',
        level: LogLevel.error,
        error: error,
        stackTrace: stackTrace,
      );
    } finally {
      isChecking.value = false;
    }
  }

  /// 释放监听器。
  void dispose() {
    coreInfo.dispose();
    selfCheckLines.dispose();
    isChecking.dispose();
    errorMessage.dispose();
  }
}
