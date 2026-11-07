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
import '../../controller/update_controller.dart';
import '../../controller/wall_controller.dart';
import '../../core/l10n/strings.dart';
import '../../core/theme/tokens.dart';
import '../components/rdc_button.dart';
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
    required this.update,
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

  /// 软件更新控制器（设置页更新分组用）。
  final UpdateController update;

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
          Wrap(
            spacing: RdcTokens.spaceSm,
            runSpacing: RdcTokens.spaceSm,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              _chip(
                label: '${AppStrings.hostLabel} · ${widget.shell.hostName.value}',
                valueListenable: widget.shell.hostName,
              ),
              _chip(
                label: AppStrings.versionLabel,
                valueListenable: widget.shell.coreVersion,
              ),
              _chip(
                label: AppStrings.platformLabel,
                valueListenable: widget.shell.platform,
              ),
              ValueListenableBuilder(
                valueListenable: widget.shell.bridgeHealthy,
                builder: (context, healthy, _) => Container(
                  padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
                  decoration: BoxDecoration(
                    color: (healthy ? RdcTokens.success : RdcTokens.danger)
                        .withValues(alpha: 0.12),
                    borderRadius: BorderRadius.circular(RdcTokens.radiusPill),
                    border: Border.all(
                      color: (healthy ? RdcTokens.success : RdcTokens.danger)
                          .withValues(alpha: 0.4),
                    ),
                  ),
                  child: Text(
                    healthy ? AppStrings.bridgeOk : AppStrings.bridgeDown,
                    style: TextStyle(
                      fontSize: RdcTokens.fontCaption,
                      color: healthy ? RdcTokens.success : RdcTokens.danger,
                      fontWeight: FontWeight.w600,
                    ),
                  ),
                ),
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
                        child: RdcStateButton(
                          label: labels[index],
                          icon: icons[index],
                          active: active == index,
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
                      child: RdcStateButton(
                        label: labels[index],
                        icon: icons[index],
                        active: active == index,
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
              update: widget.update,
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
