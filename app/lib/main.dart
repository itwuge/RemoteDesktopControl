// ============================================================
// 文件用途：应用入口（组装 model → controller → view）
//
// 层级：app 启动层
// 被谁调用：Flutter engine 的 dart entrypoint
// 边界：只负责依赖注入与初始化顺序；不承载业务逻辑
// ============================================================

import 'dart:io';

import 'package:flutter/material.dart';

import 'controller/account_controller.dart';
import 'controller/device_controller.dart';
import 'controller/permission_controller.dart';
import 'controller/session_controller.dart';
import 'controller/settings_controller.dart';
import 'controller/shell_controller.dart';
import 'controller/wall_controller.dart';
import 'core/utils/logger.dart';
import 'model/account/account_repository.dart';
import 'model/gateway/rust_gateway.dart';
import 'model/settings/settings_repository.dart';
import 'view/pages/permission_page.dart';
import 'view/pages/shell_page.dart';
import 'view/theme/app_theme.dart';

/// 应用启动：先装载 Rust 动态库，再组装 MVC 并运行。
Future<void> main() async {
  // 必须在任何异步 IO 前初始化绑定（FRB 的默认加载器依赖平台通道）。
  WidgetsFlutterBinding.ensureInitialized();

  // =====** Model 组装 =====**
  final gateway = RustGateway();
  final settingsRepository = SettingsRepository(gateway: gateway);

  // =====** 初始化 Rust 桥接（失败不阻止启动，状态区会展示错误） =====**
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
  final shellController = ShellController(gateway: gateway);
  final deviceController = DeviceController(gateway: gateway);
  final wallController = WallController(gateway: gateway);
  final sessionController = SessionController(gateway: gateway);
  final settingsController = SettingsController(repository: settingsRepository);
  final permissionController = PermissionController(
    repository: settingsRepository,
    gateway: gateway,
  );

  // 账号页：本地登录态持久化 + 账号服务可用性探测（服务未部署只显示「未连接」）。
  final accountController = AccountController(
    repository: AccountRepository(),
  );

  // 首屏所需的最小数据：顶部状态与设备列表；失败由各控制器自己记录错误。
  await shellController.loadStatus();
  if (bridgeInitError != null) {
    shellController.bridgeHealthy.value = false;
    shellController.errorMessage.value = 'Rust 核心加载失败：$bridgeInitError';
  }

  // 截图/演示钩子：RDC_START_TAB 指定初始功能区，RDC_START_PAGE=permission 直接进权限页。
  // 未设置时行为不变；这类入口只在本地验证时使用，不参与业务逻辑。
  final startTab = _startTabFromEnv();
  if (startTab != null) shellController.selectTab(startTab);

  runApp(RdcApp(
    shellController: shellController,
    deviceController: deviceController,
    wallController: wallController,
    sessionController: sessionController,
    settingsController: settingsController,
    permissionController: permissionController,
    accountController: accountController,
    showPermissionPage: Platform.environment['RDC_START_PAGE'] == 'permission',
  ));
}

/// 从环境变量读取初始功能区。
///
/// 返回：`control`/`devices`/`wall`/`settings` 对应的索引；未设置或非法返回 `null`。
int? _startTabFromEnv() {
  return switch (Platform.environment['RDC_START_TAB']) {
    'control' => ShellTab.control,
    'devices' => ShellTab.devices,
    'wall' => ShellTab.wall,
    'settings' => ShellTab.settings,
    _ => null,
  };
}

/// 应用根 Widget。
class RdcApp extends StatelessWidget {
  /// 创建应用根。
  ///
  /// 六个控制器由 [main] 构造并注入；本 Widget 只负责主题与首页装配。
  const RdcApp({
    super.key,
    required this.shellController,
    required this.deviceController,
    required this.wallController,
    required this.sessionController,
    required this.settingsController,
    required this.permissionController,
    required this.accountController,
    this.showPermissionPage = false,
  });

  /// 主界面壳控制器。
  final ShellController shellController;

  /// 设备列表控制器。
  final DeviceController deviceController;

  /// 屏幕墙控制器。
  final WallController wallController;

  /// 远程控制控制器。
  final SessionController sessionController;

  /// 设置控制器。
  final SettingsController settingsController;

  /// 远程访问权限控制器。
  final PermissionController permissionController;

  /// 账号控制器（本地登录态 + 账号服务探测）。
  final AccountController accountController;

  /// 是否直接把权限页作为首屏（仅本地截图验证时由环境变量打开）。
  final bool showPermissionPage;

  @override
  Widget build(BuildContext context) {
    return MaterialApp(
      title: 'RDC',
      debugShowCheckedModeBanner: false,
      theme: AppTheme.dark(),
      home: showPermissionPage
          ? PermissionPage(controller: permissionController)
          : ShellPage(
              shell: shellController,
              devices: deviceController,
              wall: wallController,
              session: sessionController,
              settings: settingsController,
              permission: permissionController,
              account: accountController,
            ),
    );
  }
}
