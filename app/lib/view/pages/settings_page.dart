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
import '../../controller/shell_controller.dart';
import '../../core/l10n/strings.dart';
import '../../core/theme/tokens.dart';
import '../../core/utils/formatters.dart';
import '../../model/account/account_model.dart';
import '../components/fields.dart';
import '../components/info_row.dart';
import '../components/rdc_button.dart';
import '../components/rdc_dialog.dart';
import '../components/section_card.dart';
import 'permission_page.dart';

/// 设置页。
class SettingsPage extends StatefulWidget {
  /// 创建设置页。
  ///
  /// [controller] 设置状态；[permission] 权限页控制器；
  /// [account] 账号控制器；[shell] 提供版本/平台/桥接状态（关于与诊断）。
  const SettingsPage({
    super.key,
    required this.controller,
    required this.permission,
    required this.account,
    required this.shell,
  });

  /// 设置控制器。
  final SettingsController controller;

  /// 权限页控制器。
  final PermissionController permission;

  /// 账号控制器（本地登录态 + 服务探测）。
  final AccountController account;

  /// 壳控制器（只读核心信息，用于「关于与诊断」）。
  final ShellController shell;

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
    // 去掉右上角重复胶囊：状态已由按钮与说明表达（清单 P1-1 / G6）。
    return RdcSectionCard(
      title: AppStrings.networkSection,
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
                    // 开启中继是「需要注意」用警示橙；关闭是常态，用中性灰而不是绿色成功色（清单 P1-1 / G6）。
                    color: allowRelay ? RdcTokens.warning : RdcTokens.textMuted,
                  ),
                ),
              ),
            ],
          ),
          if (allowRelay) ...[
            const SizedBox(height: RdcTokens.spaceLg),
            // 窄窗把地址与策略自动换行，不再把两个字段硬塞进一行（清单 P1-11）。
            Wrap(
              spacing: RdcTokens.spaceMd,
              runSpacing: RdcTokens.spaceMd,
              crossAxisAlignment: WrapCrossAlignment.start,
              children: [
                RdcField(
                  label: AppStrings.relayAddress,
                  width: 360,
                  child: RdcTextField(
                    controller: _relayAddress,
                    hint: 'relay.example.com:21117',
                    onChanged: widget.controller.setRelayAddress,
                  ),
                ),
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
      // 窄窗自动换行，不把三个字段压成一行（清单 E4）。
      child: Wrap(
        spacing: RdcTokens.spaceMd,
        runSpacing: RdcTokens.spaceMd,
        crossAxisAlignment: WrapCrossAlignment.start,
        children: [
          RdcField(
            label: AppStrings.resolutionLabel,
            expand: false,
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
      // 方向改为三个并列按钮（一眼看出当前与可选项，不用循环点击，清单 P1-12）。
      child: Wrap(
        spacing: RdcTokens.spaceMd,
        runSpacing: RdcTokens.spaceMd,
        crossAxisAlignment: WrapCrossAlignment.start,
        children: [
          RdcField(
            label: AppStrings.orientationShort,
            expand: false,
            child: Row(
              children: [
                for (var index = 0; index < 3; index++) ...[
                  if (index != 0) const SizedBox(width: RdcTokens.spaceSm),
                  RdcStateButton(
                    label: const [
                      AppStrings.orientationFollow,
                      AppStrings.orientationLandscape,
                      AppStrings.orientationPortrait,
                    ][index],
                    active: widget.controller.orientationIndex == index,
                    onPressed: () =>
                        widget.controller.setOrientationByIndex(index),
                  ),
                ],
              ],
            ),
          ),
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
          RdcField(
            label: AppStrings.bandwidthLabel,
            width: 320,
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

  // =====** 组件：启动与托盘（清单 P1-14） =====**

  /// 启动与托盘。
  ///
  /// 内核尚未提供开机自启 / 托盘驻留 / 防休眠接口，这里只给明确说明，
  /// 不放看起来能点、实际没有效果的假开关。
  Widget _buildStartupSection() {
    return RdcSectionCard(
      title: AppStrings.startupSection,
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          const Icon(
            Icons.info_outline,
            size: 16,
            color: RdcTokens.textMuted,
          ),
          const SizedBox(width: RdcTokens.spaceSm),
          Expanded(
            child: Text(
              AppStrings.startupUnavailable,
              style: const TextStyle(
                fontSize: RdcTokens.fontCaption,
                color: RdcTokens.textMuted,
                height: 1.5,
              ),
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
          // 主操作固定靠右：恢复默认（次要）+ 保存（主要），宽度 120 保持稳定（清单 P1-13）。
          Align(
            alignment: Alignment.centerRight,
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                SizedBox(
                  width: 120,
                  child: RdcButton(
                    label: AppStrings.restoreDefaults,
                    icon: Icons.settings_backup_restore,
                    onPressed: _resetDefaults,
                  ),
                ),
                const SizedBox(width: RdcTokens.spaceSm),
                SizedBox(
                  width: 120,
                  child: ValueListenableBuilder(
                    valueListenable: widget.controller.isSaving,
                    builder: (context, saving, _) => RdcButton(
                      label: saving ? '保存中…' : AppStrings.saveButton,
                      primary: true,
                      onPressed: saving ? null : _save,
                    ),
                  ),
                ),
              ],
            ),
          ),
          // 「设置文件：/AppData/...」属于工程细节，已移出主界面（清单 E5 / P0-6）。
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
    final confirmed = await showRdcConfirmDialog(
      context,
      title: AppStrings.relayConfirmTitle,
      body: AppStrings.relayConfirmBody,
      confirmLabel: AppStrings.confirm,
      danger: true,
    );
    if (confirmed) {
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

  /// 恢复默认：确认后只重置草稿，仍需点「保存设置」才落盘。
  Future<void> _resetDefaults() async {
    final confirmed = await showRdcConfirmDialog(
      context,
      title: AppStrings.resetDefaultsTitle,
      body: AppStrings.resetDefaultsBody,
      confirmLabel: AppStrings.restoreDefaults,
    );
    if (!confirmed || !mounted) return;
    widget.controller.resetDrafts();
    ScaffoldMessenger.of(context).showSnackBar(
      const SnackBar(content: Text(AppStrings.resetDefaultsDone)),
    );
  }

  /// 检查更新：没有更新通道时给出明确说明，不假装「已是最新」。
  void _checkUpdate() {
    ScaffoldMessenger.of(context).showSnackBar(
      const SnackBar(content: Text(AppStrings.checkUpdateUnavailable)),
    );
  }

  /// 能力与限制：静态说明，放在「关于」里，不占主界面。
  Future<void> _showCapabilities() async {
    await showDialog<void>(
      context: context,
      builder: (dialogContext) => RdcDialog(
        title: AppStrings.capabilities,
        actions: [
          RdcButton(
            label: AppStrings.close,
            primary: true,
            onPressed: () => Navigator.of(dialogContext).pop(),
          ),
        ],
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            for (final line in AppStrings.capabilitiesLines)
              Padding(
                padding: const EdgeInsets.only(bottom: RdcTokens.spaceSm),
                child: Text(
                  '· $line',
                  style: const TextStyle(
                    fontSize: RdcTokens.fontBody,
                    color: RdcTokens.textPrimary,
                    height: 1.5,
                  ),
                ),
              ),
          ],
        ),
      ),
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
                      // 登录/重试是动作，不是开关：改用普通按钮（清单 G5）。
                      RdcButton(
                        label: AppStrings.accountRetry,
                        onPressed: widget.account.refreshService,
                      ),
                      const SizedBox(width: RdcTokens.spaceSm),
                      RdcButton(
                        label: account.signedIn
                            ? AppStrings.accountLogout
                            : AppStrings.accountLogin,
                        primary: !account.signedIn,
                        danger: account.signedIn,
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

  /// 打开本地登录弹窗（标签在上，不使用浮标签）。
  Future<void> _openLogin() async {
    final controller = TextEditingController();
    final email = await showDialog<String>(
      context: context,
      builder: (dialogContext) => RdcDialog(
        title: AppStrings.accountLogin,
        actions: [
          RdcButton(
            label: AppStrings.cancel,
            onPressed: () => Navigator.of(dialogContext).pop(),
          ),
          RdcButton(
            label: AppStrings.confirm,
            primary: true,
            onPressed: () => Navigator.of(dialogContext).pop(controller.text),
          ),
        ],
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            const Text(
              AppStrings.accountLoginBody,
              style: TextStyle(
                fontSize: RdcTokens.fontCaption,
                color: RdcTokens.textMuted,
                height: 1.5,
              ),
            ),
            const SizedBox(height: RdcTokens.spaceLg),
            RdcField(
              label: AppStrings.accountEmailLabel,
              expand: false,
              child: RdcTextField(
                controller: controller,
                hint: 'name@example.com',
              ),
            ),
          ],
        ),
      ),
    );
    controller.dispose();
    if (email == null) return;
    await widget.account.signIn(email);
  }

  // =====** 组件：关于与诊断（清单 P0-2 / P1-18） =====**

  /// 承载版本、平台与核心连接状态；主界面不再显示这些工程信息。
  Widget _buildAboutSection() {
    return RdcSectionCard(
      title: AppStrings.aboutSection,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          ValueListenableBuilder<String>(
            valueListenable: widget.shell.coreVersion,
            builder: (context, version, _) => RdcInfoRow(
              label: AppStrings.aboutVersion,
              value: version,
            ),
          ),
          ValueListenableBuilder<String>(
            valueListenable: widget.shell.platform,
            builder: (context, platform, _) => RdcInfoRow(
              label: AppStrings.aboutPlatform,
              value: platform,
            ),
          ),
          ValueListenableBuilder<bool>(
            valueListenable: widget.shell.bridgeHealthy,
            builder: (context, healthy, _) => RdcInfoRow(
              label: AppStrings.aboutBridge,
              value: healthy
                  ? AppStrings.aboutBridgeOk
                  : AppStrings.aboutBridgeDown,
              valueColor: healthy ? RdcTokens.success : RdcTokens.danger,
            ),
          ),
          const SizedBox(height: RdcTokens.spaceSm),
          const Text(
            AppStrings.aboutHint,
            style: TextStyle(
              fontSize: RdcTokens.fontCaption,
              color: RdcTokens.textMuted,
            ),
          ),
          const SizedBox(height: RdcTokens.spaceMd),
          // 更新与能力边界放在「关于」里，属于低频信息（清单 P1-18）。
          Wrap(
            spacing: RdcTokens.spaceSm,
            runSpacing: RdcTokens.spaceSm,
            children: [
              RdcButton(
                label: AppStrings.checkUpdate,
                icon: Icons.system_update,
                onPressed: _checkUpdate,
              ),
              RdcButton(
                label: AppStrings.capabilities,
                icon: Icons.info_outline,
                onPressed: _showCapabilities,
              ),
            ],
          ),
        ],
      ),
    );
  }

  // =====** 组件：页面骨架 =====**

  @override
  Widget build(BuildContext context) {
    return SingleChildScrollView(
      padding: const EdgeInsets.all(RdcTokens.spacePage),
      child: ValueListenableBuilder(
        valueListenable: widget.controller.settings,
        builder: (context, settings, _) {
          // 顺序：先本地能力，再可选账号，最后诊断信息（清单 P0-11）。
          return Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
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
              _buildStartupSection(),
              const SizedBox(height: RdcTokens.spaceLg),
              _buildQuickSection(),
              const SizedBox(height: RdcTokens.spaceLg),
              _buildAccountSection(),
              const SizedBox(height: RdcTokens.spaceLg),
              _buildAboutSection(),
            ],
          );
        },
      ),
    );
  }
}
