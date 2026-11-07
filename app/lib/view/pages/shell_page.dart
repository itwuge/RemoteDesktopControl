// ============================================================
// 页面用途：主界面骨架（顶部状态区 + 四个功能入口 + 单栏内容区）
//
// 层级：view/pages
// 被谁组装：main.dart
// 数据来源：ShellController（顶部状态、当前入口、当前设备）
// 布局规矩：上下单栏，不做左右分栏；入口用按钮态表达，不用 TabBar
// ============================================================

import 'package:flutter/foundation.dart';
import 'package:flutter/material.dart';

import '../../controller/account_controller.dart';
import '../../controller/device_controller.dart';
import '../../controller/permission_controller.dart';
import '../../controller/session_controller.dart';
import '../../controller/settings_controller.dart';
import '../../controller/shell_controller.dart';
import '../../controller/wall_controller.dart';
import '../../core/l10n/strings.dart';
import '../../core/theme/tokens.dart';
import '../components/home_access_card.dart';
import '../components/rdc_button.dart';
import '../components/rdc_dialog.dart';
import 'device_page.dart';
import 'session_page.dart';
import 'settings_page.dart';
import 'wall_page.dart';

/// 主界面壳。
class ShellPage extends StatefulWidget {
  /// 创建主界面。
  ///
  /// 各控制器由 main.dart 构造并注入；本页不创建 model 对象。
  const ShellPage({
    super.key,
    required this.shell,
    required this.devices,
    required this.wall,
    required this.session,
    required this.settings,
    required this.permission,
    required this.account,
  });

  /// 壳控制器。
  final ShellController shell;

  /// 设备列表控制器。
  final DeviceController devices;

  /// 屏幕墙控制器。
  final WallController wall;

  /// 远程控制控制器。
  final SessionController session;

  /// 设置控制器。
  final SettingsController settings;

  /// 远程访问权限控制器（设置页「管理」入口跳转用）。
  final PermissionController permission;

  /// 账号控制器（设置页账号分组用）。
  final AccountController account;

  @override
  State<ShellPage> createState() => _ShellPageState();
}

/// 主界面状态。
class _ShellPageState extends State<ShellPage> {
  @override
  void initState() {
    super.initState();
    widget.shell.loadStatus();
    widget.devices.load();
    widget.session.loadRole();
    // 首页要显示「能否被连 / 谁在连」，这里读取一次共享快照（幂等）。
    widget.permission.load();
  }

  // =====** 组件：顶部状态区 =====**

  Widget _buildStatusBar() {
    return Container(
      padding: const EdgeInsets.symmetric(
        horizontal: RdcTokens.spacePage,
        vertical: RdcTokens.spaceLg,
      ),
      decoration: const BoxDecoration(
        border: Border(bottom: BorderSide(color: RdcTokens.border)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              const Icon(Icons.desktop_windows, color: RdcTokens.accent, size: 22),
              const SizedBox(width: RdcTokens.spaceSm),
              const Text(
                AppStrings.appName,
                style: TextStyle(
                  fontSize: RdcTokens.fontTitle,
                  fontWeight: FontWeight.w700,
                  color: RdcTokens.textPrimary,
                ),
              ),
              const SizedBox(width: RdcTokens.spaceMd),
              Flexible(
                child: Text(
                  AppStrings.appTagline,
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(
                    fontSize: RdcTokens.fontCaption,
                    color: RdcTokens.textMuted,
                  ),
                ),
              ),
            ],
          ),
          const SizedBox(height: RdcTokens.spaceMd),
          // 顶部只保留 1 个身份胶囊；版本/平台/桥接状态统一移到「设置 → 关于与诊断」
          // （清单 A2 / E5 / P0-2）。
          Wrap(
            spacing: RdcTokens.spaceSm,
            runSpacing: RdcTokens.spaceSm,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              _chip(
                label: AppStrings.hostLabel,
                valueListenable: widget.shell.hostName,
              ),
            ],
          ),
        ],
      ),
    );
  }

  /// 单条状态胶囊。
  Widget _chip({
    required String label,
    required ValueListenable<String> valueListenable,
  }) {
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
      decoration: BoxDecoration(
        color: RdcTokens.surface,
        borderRadius: BorderRadius.circular(RdcTokens.radiusPill),
        border: Border.all(color: RdcTokens.border),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Text(
            '$label ',
            style: const TextStyle(
              fontSize: RdcTokens.fontCaption,
              color: RdcTokens.textMuted,
            ),
          ),
          ValueListenableBuilder(
            valueListenable: valueListenable,
            builder: (context, value, _) => Text(
              value,
              style: const TextStyle(
                fontSize: RdcTokens.fontCaption,
                color: RdcTokens.textPrimary,
                fontWeight: FontWeight.w600,
              ),
            ),
          ),
        ],
      ),
    );
  }

  // =====** 组件：功能入口 =====**

  Widget _buildEntries() {
    const labels = [
      AppStrings.entryControl,
      AppStrings.entryDevices,
      AppStrings.entryWall,
      AppStrings.entrySettings,
    ];
    const icons = [
      Icons.open_in_full,
      Icons.devices,
      Icons.grid_view,
      Icons.settings,
    ];
    return Padding(
      padding: const EdgeInsets.fromLTRB(
        RdcTokens.spacePage,
        RdcTokens.spaceMd,
        RdcTokens.spacePage,
        0,
      ),
      child: ValueListenableBuilder(
        valueListenable: widget.shell.activeTab,
        builder: (context, active, _) {
          // 窄屏（手机竖屏）四个入口排成两行两列，避免按钮被压扁溢出。
          return LayoutBuilder(
            builder: (context, constraints) {
              if (constraints.maxWidth < 480.0) {
                final itemWidth =
                    (constraints.maxWidth - RdcTokens.spaceSm) / 2;
                return Wrap(
                  spacing: RdcTokens.spaceSm,
                  runSpacing: RdcTokens.spaceSm,
                  children: [
                    for (var index = 0; index < labels.length; index++)
                      SizedBox(
                        width: itemWidth,
                        child: RdcNavTab(
                          label: labels[index],
                          icon: icons[index],
                          selected: active == index,
                          onPressed: () => widget.shell.selectTab(index),
                        ),
                      ),
                  ],
                );
              }
              return Row(
                children: [
                  for (var index = 0; index < labels.length; index++) ...[
                    if (index != 0) const SizedBox(width: RdcTokens.spaceSm),
                    Expanded(
                      child: RdcNavTab(
                        label: labels[index],
                        icon: icons[index],
                        selected: active == index,
                        onPressed: () => widget.shell.selectTab(index),
                      ),
                    ),
                  ],
                ],
              );
            },
          );
        },
      ),
    );
  }

  // =====** 组件：本机可被控状态（清单 A1） =====**

  /// 首页安全状态卡：只调用已有接口，不新增协议。
  Widget _buildAccessCard() {
    // 设置、内核快照、保存态任一变化都要刷新（用 merge 合并监听）。
    return ListenableBuilder(
      listenable: Listenable.merge([
        widget.permission.settings,
        widget.permission.kernel,
        widget.permission.saving,
      ]),
      builder: (context, _) {
        final snapshot = widget.permission.kernel.value;
        final settings = widget.permission.settings.value;
        // 两份数据都拿不到时才显示「不可读」，避免把加载中误判为已停止。
        if (snapshot == null && settings == null) {
          return RdcAccessCard(
            enabled: false,
            onlineLabel: '',
            unavailable: true,
            onRetry: widget.permission.load,
          );
        }
        final enabled =
            snapshot?.remoteAccessEnabled ?? settings!.remoteAccessEnabled;
        final online =
            snapshot?.connectedLabel ?? AppStrings.homeAccessNoSession;
        return RdcAccessCard(
          enabled: enabled,
          onlineLabel: online,
          busy: widget.permission.saving.value,
          onStop: _stopRemoteAccess,
          onResume: _resumeRemoteAccess,
        );
      },
    );
  }

  /// 停止远程访问：先二次确认，再调用既有内核接口。
  Future<void> _stopRemoteAccess() async {
    final confirmed = await showRdcConfirmDialog(
      context,
      title: AppStrings.stopConfirmTitle,
      body: AppStrings.stopConfirmBody,
      confirmLabel: AppStrings.stopRemote,
      danger: true,
    );
    if (!confirmed || !mounted) return;
    final report = await widget.permission.stopRemoteAccess();
    if (!mounted) return;
    final error = widget.permission.errorMessage.value;
    _showMessage(
      report != null ? AppStrings.stopDone : (error ?? AppStrings.unknownError),
    );
  }

  /// 恢复远程访问：调用前必须本机确认（内核要求 confirm=true）。
  Future<void> _resumeRemoteAccess() async {
    final confirmed = await showRdcConfirmDialog(
      context,
      title: AppStrings.resumeConfirmTitle,
      body: AppStrings.resumeConfirmBody,
      confirmLabel: AppStrings.confirm,
    );
    if (!confirmed || !mounted) return;
    final error = await widget.permission.resumeRemoteAccess();
    if (!mounted) return;
    _showMessage(error ?? AppStrings.resumeDone);
  }

  /// 统一提示条。
  void _showMessage(String message) {
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(content: Text(message)),
    );
  }

  // =====** 组件：当前功能区 =====**

  Widget _buildActivePage() {
    return ValueListenableBuilder(
      valueListenable: widget.shell.activeTab,
      builder: (context, active, _) {
        final page = switch (active) {
          ShellTab.devices => DevicePage(
              controller: widget.devices,
              shell: widget.shell,
            ),
          ShellTab.wall => WallPage(
              controller: widget.wall,
              shell: widget.shell,
            ),
          ShellTab.settings => SettingsPage(
              controller: widget.settings,
              permission: widget.permission,
              account: widget.account,
              shell: widget.shell,
            ),
          _ => SessionPage(
              controller: widget.session,
              shell: widget.shell,
            ),
        };
        return AnimatedSwitcher(
          duration: const Duration(milliseconds: 160),
          child: KeyedSubtree(
            key: ValueKey(active),
            child: page,
          ),
        );
      },
    );
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: SafeArea(
        child: Column(
          children: [
            _buildStatusBar(),
            _buildEntries(),
            // 安全状态只在「远程控制」首页出现，其他页面不占垂直空间。
            ValueListenableBuilder<int>(
              valueListenable: widget.shell.activeTab,
              builder: (context, active, _) {
                if (active != ShellTab.control) return const SizedBox.shrink();
                return Padding(
                  padding: const EdgeInsets.fromLTRB(
                    RdcTokens.spacePage,
                    RdcTokens.spaceMd,
                    RdcTokens.spacePage,
                    0,
                  ),
                  child: Align(
                    alignment: Alignment.topCenter,
                    child: ConstrainedBox(
                      constraints: const BoxConstraints(
                        maxWidth: RdcTokens.pageMaxWidth,
                      ),
                      child: _buildAccessCard(),
                    ),
                  ),
                );
              },
            ),
            Expanded(
              child: Align(
                alignment: Alignment.topCenter,
                child: ConstrainedBox(
                  constraints: const BoxConstraints(
                    maxWidth: RdcTokens.pageMaxWidth,
                  ),
                  child: _buildActivePage(),
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }
}
