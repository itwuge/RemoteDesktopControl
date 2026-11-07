// ============================================================
// 页面用途：设置页（网络与中继 / 编码参数 / 降级策略 / 快捷设置）
//
// 层级：view/pages
// 被谁组装：shell_page.dart
// 数据来源：SettingsController（草稿设置、保存结果）
// 布局规矩：上下单栏、分组框；标签在上控件在下；用按钮态替代开关
// ============================================================

import 'package:flutter/material.dart';

import '../../controller/account_controller.dart';
import '../../controller/permission_controller.dart';
import '../../controller/settings_controller.dart';
import '../../core/l10n/strings.dart';
import '../../core/theme/tokens.dart';
import '../../core/utils/formatters.dart';
import '../../model/account/account_model.dart';
import '../components/fields.dart';
import '../components/info_row.dart';
import '../components/rdc_button.dart';
import '../components/section_card.dart';
import 'permission_page.dart';

/// 设置页。
class SettingsPage extends StatefulWidget {
  /// 创建设置页。
  ///
  /// [controller] 设置状态；[permission] 权限页控制器。
  const SettingsPage({
    super.key,
    required this.controller,
    required this.permission,
    required this.account,
  });

  /// 设置控制器。
  final SettingsController controller;

  /// 权限页控制器。
  final PermissionController permission;

  /// 账号控制器（本地登录态 + 服务探测）。
  final AccountController account;

  @override
  State<SettingsPage> createState() => _SettingsPageState();
}

/// 设置页状态。
class _SettingsPageState extends State<SettingsPage> {
  /// 中继地址输入框。
  late final TextEditingController _relayAddress;

  @override
  void initState() {
    super.initState();
    _relayAddress = TextEditingController(
      text: widget.controller.settings.value.relayAddress,
    );
    widget.controller.settings.addListener(_syncRelayAddress);
    widget.controller.load();
    // 本地账号状态与账号服务可用性独立探测：服务缺失只显示「未连接」。
    widget.account.load();
    widget.account.refreshService();
  }

  /// 设置从磁盘加载后同步地址文本。
  void _syncRelayAddress() {
    final value = widget.controller.settings.value.relayAddress;
    if (_relayAddress.text != value) {
      _relayAddress.text = value;
    }
  }

  @override
  void dispose() {
    widget.controller.settings.removeListener(_syncRelayAddress);
    _relayAddress.dispose();
    super.dispose();
  }

  // =====** 组件：网络与中继 =====**

  Widget _buildNetworkSection(bool allowRelay) {
    return RdcSectionCard(
      title: AppStrings.networkSection,
      trailing: RdcStatusPill(
        text: allowRelay ? '已开启' : '默认关闭',
        color: allowRelay ? RdcTokens.danger : RdcTokens.success,
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              RdcStateButton(
                label: allowRelay ? AppStrings.relayOn : AppStrings.relayOff,
                icon: Icons.cable,
                active: allowRelay,
                onPressed: () => _onRelayChanged(!allowRelay),
              ),
              const SizedBox(width: RdcTokens.spaceMd),
              Expanded(
                child: Text(
                  allowRelay
                      ? AppStrings.relayOnHint
                      : AppStrings.relayOffHint,
                  style: TextStyle(
                    fontSize: RdcTokens.fontCaption,
                    color: allowRelay ? RdcTokens.danger : RdcTokens.success,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ),
            ],
          ),
          if (allowRelay) ...[
            const SizedBox(height: RdcTokens.spaceLg),
            Row(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                RdcField(
                  label: AppStrings.relayAddress,
                  child: RdcTextField(
                    controller: _relayAddress,
                    hint: 'relay.example.com:21117',
                    onChanged: widget.controller.setRelayAddress,
                  ),
                ),
                const SizedBox(width: RdcTokens.spaceMd),
                RdcField(
                  label: AppStrings.policyLabel,
                  expand: false,
                  child: Row(
                    children: [
                      RdcStateButton(
                        label: AppStrings.policyStrict,
                        active: widget.controller.policyIndex == 0,
                        onPressed: () =>
                            widget.controller.setConnectionPolicyByIndex(0),
                      ),
                      const SizedBox(width: RdcTokens.spaceSm),
                      RdcStateButton(
                        label: AppStrings.policyAsk,
                        active: widget.controller.policyIndex == 1,
                        onPressed: () =>
                            widget.controller.setConnectionPolicyByIndex(1),
                      ),
                      const SizedBox(width: RdcTokens.spaceSm),
                      RdcStateButton(
                        label: AppStrings.policyAuto,
                        active: widget.controller.policyIndex == 2,
                        onPressed: () =>
                            widget.controller.setConnectionPolicyByIndex(2),
                      ),
                    ],
                  ),
                ),
              ],
            ),
          ],
        ],
      ),
    );
  }

  // =====** 组件：编码参数 =====**

  Widget _buildEncodingSection(int width, int height, int fps, double mbps) {
    const presets = [
      (1920, 1080, '1080p'),
      (1280, 720, '720p'),
      (854, 480, '480p'),
    ];
    return RdcSectionCard(
      title: AppStrings.encodingSection,
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          RdcField(
            label: AppStrings.resolutionLabel,
            child: Row(
              children: [
                for (final preset in presets) ...[
                  if (preset != presets.first)
                    const SizedBox(width: RdcTokens.spaceSm),
                  RdcStateButton(
                    label: preset.$3,
                    active: width == preset.$1 && height == preset.$2,
                    onPressed: () =>
                        widget.controller.setVideoSize(preset.$1, preset.$2),
                  ),
                ],
              ],
            ),
          ),
          const SizedBox(width: RdcTokens.spaceMd),
          RdcField(
            label: AppStrings.fpsLabel,
            expand: false,
            child: Row(
              children: [
                for (final option in const [15, 30, 60]) ...[
                  if (option != 15) const SizedBox(width: RdcTokens.spaceSm),
                  RdcStateButton(
                    label: '$option',
                    active: fps == option,
                    onPressed: () => widget.controller.setVideoFps(option),
                  ),
                ],
              ],
            ),
          ),
          const SizedBox(width: RdcTokens.spaceMd),
          RdcField(
            label: AppStrings.bitrateLabel,
            expand: false,
            child: Row(
              children: [
                for (final option in const [4.0, 8.0, 16.0]) ...[
                  if (option != 4.0) const SizedBox(width: RdcTokens.spaceSm),
                  RdcStateButton(
                    label: '${option.toInt()} Mbps',
                    active: mbps == option,
                    onPressed: () => widget.controller.setVideoBitrate(option),
                  ),
                ],
              ],
            ),
          ),
        ],
      ),
    );
  }

  // =====** 组件：降级策略 =====**

  Widget _buildDegradationSection(bool autoReconnect) {
    return RdcSectionCard(
      title: AppStrings.degradationSection,
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          RdcField(
            label: AppStrings.orientationShort,
            expand: false,
            child: RdcStateButton(
              label: const [
                AppStrings.orientationFollow,
                AppStrings.orientationLandscape,
                AppStrings.orientationPortrait,
              ][widget.controller.orientationIndex],
              icon: Icons.screen_rotation,
              active: true,
              onPressed: () {
                final next = (widget.controller.orientationIndex + 1) % 3;
                widget.controller.setOrientationByIndex(next);
              },
            ),
          ),
          const SizedBox(width: RdcTokens.spaceMd),
          RdcField(
            label: AppStrings.autoReconnectLabel,
            expand: false,
            child: RdcStateButton(
              label: autoReconnect ? '已开启' : '已关闭',
              icon: Icons.refresh,
              active: autoReconnect,
              onPressed: () =>
                  widget.controller.setAutoReconnect(!autoReconnect),
            ),
          ),
          const SizedBox(width: RdcTokens.spaceMd),
          RdcField(
            label: AppStrings.bandwidthLabel,
            child: Row(
              children: [
                Expanded(
                  child: Slider(
                    value: widget.controller.settings.value.fileBandwidthRatio,
                    min: 0.1,
                    max: 0.7,
                    divisions: 6,
                    label: Formatters.percent(
                      widget.controller.settings.value.fileBandwidthRatio,
                    ),
                    onChanged: widget.controller.setFileBandwidthRatio,
                  ),
                ),
                Text(
                  Formatters.percent(
                    widget.controller.settings.value.fileBandwidthRatio,
                  ),
                  style: const TextStyle(
                    fontSize: RdcTokens.fontBody,
                    color: RdcTokens.accent,
                    fontWeight: FontWeight.w600,
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }

  // =====** 组件：快捷设置 =====**

  Widget _buildQuickSection() {
    return RdcSectionCard(
      title: AppStrings.quickSection,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              const Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      AppStrings.permissionEntry,
                      style: TextStyle(
                        fontSize: RdcTokens.fontBody,
                        color: RdcTokens.textPrimary,
                      ),
                    ),
                    SizedBox(height: 2),
                    Text(
                      AppStrings.permissionEntryHint,
                      style: TextStyle(
                        fontSize: RdcTokens.fontCaption,
                        color: RdcTokens.textMuted,
                      ),
                    ),
                  ],
                ),
              ),
              RdcButton(
                label: '管理',
                icon: Icons.shield_outlined,
                onPressed: _openPermission,
              ),
            ],
          ),
          const SizedBox(height: RdcTokens.spaceLg),
          Row(
            children: [
              Expanded(
                child: ValueListenableBuilder(
                  valueListenable: widget.controller.isSaving,
                  builder: (context, saving, _) => RdcButton(
                    label: saving ? '保存中…' : AppStrings.saveButton,
                    icon: Icons.save,
                    primary: true,
                    onPressed: saving ? null : _save,
                  ),
                ),
              ),
            ],
          ),
          const SizedBox(height: RdcTokens.spaceSm),
          Text(
            '设置文件：${widget.controller.repository.filePath}',
            style: const TextStyle(
              fontSize: RdcTokens.fontCaption,
              color: RdcTokens.textMuted,
            ),
          ),
        ],
      ),
    );
  }

  // =====** 行为：中继确认 =====**

  Future<void> _onRelayChanged(bool value) async {
    if (!value) {
      widget.controller.setAllowRelay(false);
      return;
    }
    // 开启中继属于高风险设置：必须显式确认，避免误触后流量走服务器。
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        contentPadding: const EdgeInsets.fromLTRB(24, 24, 24, 8),
        actionsPadding: const EdgeInsets.fromLTRB(24, 8, 24, 24),
        content: const SizedBox(
          width: 380,
          child: Text(AppStrings.relayConfirmBody),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(dialogContext).pop(false),
            child: const Text(AppStrings.cancel),
          ),
          FilledButton(
            onPressed: () => Navigator.of(dialogContext).pop(true),
            child: const Text(AppStrings.confirm),
          ),
        ],
      ),
    );
    if (confirmed == true) {
      widget.controller.setAllowRelay(true);
    }
  }

  /// 打开远程访问权限页。
  void _openPermission() {
    Navigator.of(context).push(
      MaterialPageRoute<void>(
        builder: (_) => PermissionPage(controller: widget.permission),
      ),
    );
  }

  /// 保存并提示结果。
  Future<void> _save() async {
    await widget.controller.save();
    if (!mounted) return;
    final message = widget.controller.message.value;
    if (message == null) return;
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(content: Text(message)),
    );
  }

  // =====** 组件：账号 =====**

  /// 账号分组：本地登录态 + 账号服务可用性 + 同账号设备发现。
  Widget _buildAccountSection() {
    return ValueListenableBuilder<AccountState>(
      valueListenable: widget.account.state,
      builder: (context, account, _) {
        return ValueListenableBuilder<AccountServiceState>(
          valueListenable: widget.account.service,
          builder: (context, service, _) {
            final online = service == AccountServiceState.online;
            return RdcSectionCard(
              title: AppStrings.accountSection,
              trailing: RdcStatusPill(
                text: switch (service) {
                  AccountServiceState.online => AppStrings.accountServiceOnline,
                  AccountServiceState.offline => AppStrings.accountServiceOffline,
                  AccountServiceState.unknown => AppStrings.accountServiceProbing,
                },
                color: online ? RdcTokens.success : RdcTokens.textMuted,
              ),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      Expanded(
                        child: Text(
                          account.signedIn
                              ? '${AppStrings.accountSignedIn}：${account.email}'
                              : AppStrings.accountSignedOut,
                          style: TextStyle(
                            fontSize: RdcTokens.fontBody,
                            color: RdcTokens.textPrimary,
                            fontWeight: FontWeight.w600,
                          ),
                        ),
                      ),
                      RdcStateButton(
                        label: AppStrings.accountRetry,
                        icon: Icons.refresh,
                        active: false,
                        onPressed: widget.account.refreshService,
                      ),
                      const SizedBox(width: RdcTokens.spaceSm),
                      RdcStateButton(
                        label: account.signedIn
                            ? AppStrings.accountLogout
                            : AppStrings.accountLogin,
                        icon: account.signedIn ? Icons.logout : Icons.login,
                        active: account.signedIn,
                        onPressed: account.signedIn
                            ? widget.account.signOut
                            : _openLogin,
                      ),
                    ],
                  ),
                  const SizedBox(height: RdcTokens.spaceMd),
                  Text(
                    AppStrings.accountNote,
                    style: TextStyle(
                      fontSize: RdcTokens.fontCaption,
                      color: RdcTokens.textMuted,
                    ),
                  ),
                  const SizedBox(height: RdcTokens.spaceLg),
                  Row(
                    children: [
                      RdcStateButton(
                        label: AppStrings.accountDiscovery,
                        icon: Icons.devices_other,
                        active: account.discoveryEnabled,
                        onPressed: account.signedIn
                            ? () => widget.account.setDiscoveryEnabled(
                                  !account.discoveryEnabled,
                                )
                            : null,
                      ),
                      const SizedBox(width: RdcTokens.spaceMd),
                      Expanded(
                        child: Text(
                          account.signedIn
                              ? (account.discoveryEnabled ? '已开启' : '已关闭')
                              : AppStrings.accountDiscoveryHint,
                          style: TextStyle(
                            fontSize: RdcTokens.fontCaption,
                            color: RdcTokens.textMuted,
                          ),
                        ),
                      ),
                    ],
                  ),
                ],
              ),
            );
          },
        );
      },
    );
  }

  /// 打开本地登录弹窗（账号服务未接入，只记录本地状态）。
  Future<void> _openLogin() async {
    final controller = TextEditingController();
    final email = await showDialog<String>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        content: SizedBox(
          width: 380,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                AppStrings.accountLoginBody,
                style: TextStyle(
                  fontSize: RdcTokens.fontCaption,
                  color: RdcTokens.textMuted,
                ),
              ),
              const SizedBox(height: RdcTokens.spaceLg),
              TextField(
                controller: controller,
                autofocus: true,
                decoration: const InputDecoration(
                  labelText: AppStrings.accountEmailLabel,
                ),
              ),
            ],
          ),
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(dialogContext).pop(),
            child: const Text(AppStrings.cancel),
          ),
          FilledButton(
            onPressed: () => Navigator.of(dialogContext).pop(controller.text),
            child: const Text(AppStrings.confirm),
          ),
        ],
      ),
    );
    controller.dispose();
    if (email == null) return;
    await widget.account.signIn(email);
  }

  // =====** 组件：页面骨架 =====**

  @override
  Widget build(BuildContext context) {
    return SingleChildScrollView(
      padding: const EdgeInsets.all(RdcTokens.spacePage),
      child: ValueListenableBuilder(
        valueListenable: widget.controller.settings,
        builder: (context, settings, _) {
          return Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              _buildAccountSection(),
              const SizedBox(height: RdcTokens.spaceLg),
              _buildNetworkSection(settings.allowRelay),
              const SizedBox(height: RdcTokens.spaceLg),
              _buildEncodingSection(
                settings.videoWidth,
                settings.videoHeight,
                settings.videoFps,
                settings.videoBitrateMbps,
              ),
              const SizedBox(height: RdcTokens.spaceLg),
              _buildDegradationSection(settings.autoReconnect),
              const SizedBox(height: RdcTokens.spaceLg),
              _buildQuickSection(),
            ],
          );
        },
      ),
    );
  }
}
