// ============================================================
// 文件用途：应用入口（组装 model → controller → view）
//
// 层级：app 启动层
// 被谁调用：Flutter engine 的 dart entrypoint
// 边界：只负责依赖注入与初始化顺序；不承载业务逻辑
// ============================================================

import 'dart:async';
import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter/semantics.dart';

import 'controller/account_controller.dart';
import 'controller/device_controller.dart';
import 'controller/permission_controller.dart';
import 'controller/session_controller.dart';
import 'controller/settings_controller.dart';
import 'controller/shell_controller.dart';
import 'controller/update_controller.dart';
import 'controller/wall_controller.dart';
import 'core/l10n/strings.dart';
import 'core/utils/logger.dart';
import 'model/account/account_repository.dart';
import 'model/gateway/rust_gateway.dart';
import 'model/settings/settings_repository.dart';
import 'model/update/update_service.dart';
import 'view/pages/first_run_page.dart';
import 'view/pages/permission_page.dart';
import 'view/pages/shell_page.dart';
import 'view/theme/app_theme.dart';

/// 全局语义句柄。
///
/// 主动保持引用不释放：一旦句柄被回收，Flutter 会关闭语义树，
/// Windows UIA / 读屏将再次只能看到 `FLUTTERVIEW` 一个节点。
/// 这是无障碍需求，也是后续自动化测试读取控件树的前提（清单「必须做的技术活」）。
SemanticsHandle? semanticsHandle;

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

  // =====** 配置迁移：旧 `%APPDATA%\RDC` → `<安装目录>\config`（幂等） =====**
  // 必须早于任何设置读取：否则首启动会读到默认值，看起来像“配置丢了”。
  if (bridgeInitError == null) {
    try {
      final migration = await gateway.migrateLegacyConfig();
      RdcLogger.log(
        'main',
        '配置迁移 ${migration.status} ${migration.code}：${migration.message}',
        level: migration.isSuccess ? LogLevel.info : LogLevel.warn,
      );
    } catch (error, stackTrace) {
      // 迁移失败不阻断启动；错误码由 Rust 写入 migration.json。
      RdcLogger.log(
        'main',
        '配置迁移调用失败',
        level: LogLevel.warn,
        error: error,
        stackTrace: stackTrace,
      );
    }
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

  // 首启引导与首页共用同一份设置：先读盘，避免首启页与设置页状态不一致。
  await settingsController.load();

  // =====** 软件更新：版本/更新源均以 Rust 为单一来源 =====**
  // 桥接失败时更新源为空，自动检查会被跳过（不做无意义的联网请求）。
  var appVersion = '0.0.0';
  var updateApi = '';
  var updateKeyConfigured = false;
  if (bridgeInitError == null) {
    try {
      appVersion = await gateway.appVersion();
      updateApi = await gateway.updateDefaultApiUrl();
      updateKeyConfigured = await gateway.updatePublicKeyConfigured();
    } catch (error, stackTrace) {
      RdcLogger.log(
        'main',
        '读取更新配置失败',
        level: LogLevel.warn,
        error: error,
        stackTrace: stackTrace,
      );
    }
  }
  final updateController = UpdateController(
    service: UpdateService(apiUrl: updateApi, currentVersion: appVersion),
    publicKeyConfigured: updateKeyConfigured,
  );

  // 自动检查默认开启；延迟 10 秒，避免与首屏加载/用户操作抢资源。
  // 关闭开关后这里不会创建任何定时器，也就不会发起网络请求。
  if (settingsController.settings.value.autoUpdateCheck && updateApi.isNotEmpty) {
    Timer(const Duration(seconds: 10), () {
      updateController.check(silent: true);
    });
  }

  // 首屏所需的最小数据：顶部状态与设备列表；失败由各控制器自己记录错误。
  await shellController.loadStatus();
  if (bridgeInitError != null) {
    shellController.bridgeHealthy.value = false;
    shellController.errorMessage.value =
        '${AppStrings.bridgeInitFailed}：$bridgeInitError';
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
    updateController: updateController,
    showPermissionPage: Platform.environment['RDC_START_PAGE'] == 'permission',
  ));

  // 首帧之后再通知平台“语义树开启”：此时平台视图已创建，
  // Windows embedder 才有对象可挂语义节点；在 runApp 之前调用会被丢掉。
  WidgetsBinding.instance.addPostFrameCallback((_) {
    semanticsHandle ??= SemanticsBinding.instance.ensureSemantics();
  });
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
    required this.updateController,
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

  /// 软件更新控制器。
  final UpdateController updateController;

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
          : _StartupGate(
              settings: settingsController,
              shell: ShellPage(
                shell: shellController,
                devices: deviceController,
                wall: wallController,
                session: sessionController,
                settings: settingsController,
                permission: permissionController,
                account: accountController,
                update: updateController,
              ),
            ),
    );
  }
}

/// 启动门：首启未完成时先展示「允许被连 / 不允许」，完成后进入主界面。
///
/// 首次进入读 [SettingsController.settings]；旧配置缺 `first_run_completed`
/// 时按 `false` 处理，因此老用户也会看到一次引导（安全默认仍为不允许）。
class _StartupGate extends StatefulWidget {
  /// 创建启动门。
  ///
  /// [settings] 设置控制器；[shell] 已完成装配的主界面。
  const _StartupGate({required this.settings, required this.shell});

  /// 设置控制器。
  final SettingsController settings;

  /// 主界面。
  final Widget shell;

  @override
  State<_StartupGate> createState() => _StartupGateState();
}

/// 启动门状态：只记「是否还需显示首启页」。
class _StartupGateState extends State<_StartupGate> {
  /// 是否还需要展示首启页。
  late bool _needsFirstRun;

  @override
  void initState() {
    super.initState();
    _needsFirstRun = !widget.settings.settings.value.firstRunCompleted;
  }

  @override
  Widget build(BuildContext context) {
    if (_needsFirstRun) {
      return FirstRunPage(
        controller: widget.settings,
        onDone: () => setState(() => _needsFirstRun = false),
      );
    }
    return widget.shell;
  }
}
