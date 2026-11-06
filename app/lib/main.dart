// ============================================================
// 文件用途：应用入口（组装 model → controller → view）
//
// 层级：app 启动层
// 被谁调用：Flutter engine 的 dart entrypoint
// 边界：只负责依赖注入与初始化顺序；不承载业务逻辑
// ============================================================

import 'package:flutter/material.dart';

import 'controller/home_controller.dart';
import 'controller/settings_controller.dart';
import 'core/utils/logger.dart';
import 'model/gateway/rust_gateway.dart';
import 'model/settings/settings_repository.dart';
import 'view/pages/home_page.dart';
import 'view/theme/app_theme.dart';

/// 应用启动：先装载 Rust 动态库，再组装 MVC 并运行。
Future<void> main() async {
  // 必须在任何异步 IO 前初始化绑定（FRB 的默认加载器依赖平台通道）。
  WidgetsFlutterBinding.ensureInitialized();

  // =====** Model 组装 =====**
  final gateway = RustGateway();
  final settingsRepository = SettingsRepository();

  // =====** 初始化 Rust 桥接（失败不阻止启动，首页会展示错误） =====**
  String? bridgeInitError;
  try {
    await gateway.initialize();
  } catch (error, stackTrace) {
    bridgeInitError = '$error';
    RdcLogger.log(
      'main',
      'Rust 桥接初始化失败',
      level: LogLevel.error,
      error: error,
      stackTrace: stackTrace,
    );
  }

  // =====** Controller 组装 =====**
  final homeController = HomeController(gateway: gateway);
  final settingsController = SettingsController(repository: settingsRepository);
  await settingsController.load();
  if (bridgeInitError != null) {
    homeController.errorMessage.value = 'Rust 核心加载失败：$bridgeInitError';
  } else {
    await homeController.loadCoreInfo();
  }

  runApp(RdcApp(homeController: homeController, settingsController: settingsController));
}

/// 应用根 Widget。
class RdcApp extends StatelessWidget {
  /// 创建应用根。
  ///
  /// [homeController] 传给首页；[settingsController] 传给设置页。
  const RdcApp({
    super.key,
    required this.homeController,
    required this.settingsController,
  });

  /// 首页控制器。
  final HomeController homeController;

  /// 设置控制器。
  final SettingsController settingsController;

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'RDC',
      debugShowCheckedModeBanner: false,
      theme: AppTheme.dark(),
      home: HomePage(
        controller: homeController,
        settingsController: settingsController,
      ),
    );
  }
}
