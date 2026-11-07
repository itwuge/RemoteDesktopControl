// ============================================================
// 页面用途：远程访问权限页（按 docs/13 的 7 个区块）
//
// 层级：view/pages
// 被谁组装：settings_page.dart 通过 Navigator 打开
// 数据来源：PermissionController（策略写入 Rust 设置；信任设备来自 Rust 设备列表）
// 布局规矩：上下单栏、分组框、标签在上控件在下；条目后置操作按钮
// ============================================================

import 'package:flutter/material.dart';

import '../../controller/permission_controller.dart';
import '../../core/l10n/strings.dart';
import '../../core/theme/tokens.dart';
import '../components/info_row.dart';
import '../components/rdc_button.dart';
import '../components/section_card.dart';

/// 远程访问权限页。
class PermissionPage extends StatefulWidget {
  /// 创建页面。
  ///
  /// [controller] 权限状态与持久化。
  const PermissionPage({super.key, required this.controller});

  /// 权限控制器。
  final PermissionController controller;

  @override
  State<PermissionPage> createState() => _PermissionPageState();
}

/// 权限页状态。
class _PermissionPageState extends State<PermissionPage> {
  @override
  void initState() {
    super.initState();
    widget.controller.load();
  }

  // =====** 组件：区块① 远程访问 =====**

  Widget _buildSwitchSection(bool enabled) {
    return RdcSectionCard(
      title: AppStrings.permSwitchSection,
      trailing: RdcStatusPill(
        text: enabled ? '已允许' : '已拒绝',
        color: enabled ? RdcTokens.success : RdcTokens.danger,
      ),
      child: Row(
        children: [
          RdcStateButton(
            label: enabled ? '远程访问：开启' : '远程访问：关闭',
            icon: Icons.language,
            active: enabled,
            onPressed: () => widget.controller.setRemoteAccess(!enabled),
          ),
          const SizedBox(width: RdcTokens.spaceMd),
          const Expanded(
            child: Text(
              AppStrings.permSwitchHint,
              style: TextStyle(
                fontSize: RdcTokens.fontCaption,
                color: RdcTokens.textMuted,
              ),
            ),
          ),
        ],
      ),
    );
  }

  // =====** 组件：区块② 并发与接管 =====**

  Widget _buildConcurrentSection(
    String mode,
    String takeover,
    int timeout,
  ) {
    return RdcSectionCard(
      title: AppStrings.permConcurrentSection,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          RdcFieldRow(
            label: AppStrings.concurrentModeLabel,
            child: Row(
              children: [
                RdcStateButton(
                  label: AppStrings.concurrentExclusive,
                  active: mode == 'exclusive',
                  onPressed: () =>
                      widget.controller.setConcurrentMode('exclusive'),
                ),
                const SizedBox(width: RdcTokens.spaceSm),
                RdcStateButton(
                  label: AppStrings.concurrentManyWatch,
                  active: mode == 'one_control_many_watch',
                  onPressed: () => widget.controller
                      .setConcurrentMode('one_control_many_watch'),
                ),
              ],
            ),
          ),
          const SizedBox(height: RdcTokens.spaceMd),
          RdcFieldRow(
            label: AppStrings.takeoverLabel,
            child: Row(
              children: [
                RdcStateButton(
                  label: AppStrings.takeoverAuto,
                  active: takeover == 'auto',
                  onPressed: () =>
                      widget.controller.setTakeoverPolicy('auto'),
                ),
                const SizedBox(width: RdcTokens.spaceSm),
                RdcStateButton(
                  label: AppStrings.takeoverConfirm,
                  active: takeover == 'confirm',
                  onPressed: () =>
                      widget.controller.setTakeoverPolicy('confirm'),
                ),
                const SizedBox(width: RdcTokens.spaceSm),
                RdcStateButton(
                  label: AppStrings.takeoverForbid,
                  active: takeover == 'forbid',
                  onPressed: () =>
                      widget.controller.setTakeoverPolicy('forbid'),
                ),
              ],
            ),
          ),
          const SizedBox(height: RdcTokens.spaceMd),
          RdcFieldRow(
            label: AppStrings.takeoverTimeoutLabel,
            child: Row(
              children: [
                for (final seconds in const [15, 30, 60]) ...[
                  if (seconds != 15) const SizedBox(width: RdcTokens.spaceSm),
                  RdcStateButton(
                    label: '$seconds 秒',
                    active: timeout == seconds,
                    onPressed: () =>
                        widget.controller.setTakeoverTimeout(seconds),
                  ),
                ],
              ],
            ),
          ),
        ],
      ),
    );
  }

  // =====** 组件：区块③ 权限模板 =====**

  Widget _buildTemplateSection(String template, int bits) {
    const templates = [
      ('readonly', AppStrings.templateReadonly),
      ('standard', AppStrings.templateStandard),
      ('full', AppStrings.templateFull),
      ('custom', AppStrings.templateCustom),
    ];
    return RdcSectionCard(
      title: AppStrings.permTemplateSection,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              for (final item in templates) ...[
                if (item != templates.first)
                  const SizedBox(width: RdcTokens.spaceSm),
                RdcStateButton(
                  label: item.$2,
                  active: template == item.$1,
                  onPressed: () => widget.controller.setTemplate(item.$1),
                ),
              ],
            ],
          ),
          const SizedBox(height: RdcTokens.spaceMd),
          Wrap(
            spacing: RdcTokens.spaceSm,
            runSpacing: RdcTokens.spaceSm,
            children: [
              for (var bit = 0;
                  bit < PermissionController.permissionBitLabels.length;
                  bit++)
                RdcStateButton(
                  // bit0（查看屏幕）是底线，按钮禁用并保持开启。
                  label: PermissionController.permissionBitLabels[bit],
                  active: (bits & (1 << bit)) != 0,
                  onPressed: bit == 0
                      ? null
                      : () => widget.controller.togglePermissionBit(bit),
                ),
            ],
          ),
        ],
      ),
    );
  }

  // =====** 组件：区块④ 授权访问者 =====**

  Widget _buildGrantsSection() {
    return RdcSectionCard(
      title: AppStrings.permGrantsSection,
      trailing: const RdcStatusPill(
        text: AppStrings.pendingBadge,
        color: RdcTokens.danger,
      ),
      child: Container(
        width: double.infinity,
        padding: const EdgeInsets.symmetric(
          horizontal: RdcTokens.spaceMd,
          vertical: RdcTokens.spaceLg,
        ),
        decoration: BoxDecoration(
          color: RdcTokens.surfaceRaised,
          borderRadius: BorderRadius.circular(RdcTokens.radiusControl),
          border: Border.all(color: RdcTokens.border),
        ),
        child: const Text(
          AppStrings.grantsEmpty,
          style: TextStyle(
            fontSize: RdcTokens.fontCaption,
            color: RdcTokens.textMuted,
          ),
        ),
      ),
    );
  }

  // =====** 组件：区块⑤ 连接提示 =====**

  Widget _buildNoticeSection(bool localNotice) {
    return RdcSectionCard(
      title: AppStrings.permNoticeSection,
      child: Row(
        children: [
          RdcStateButton(
            label: AppStrings.localNoticeLabel,
            icon: Icons.notifications_active_outlined,
            active: localNotice,
            onPressed: () =>
                widget.controller.setLocalNotice(!localNotice),
          ),
          const SizedBox(width: RdcTokens.spaceMd),
          const Expanded(
            child: Text(
              '开启后，有连接接入时本机持续显示提示条，直到会话结束。',
              style: TextStyle(
                fontSize: RdcTokens.fontCaption,
                color: RdcTokens.textMuted,
              ),
            ),
          ),
        ],
      ),
    );
  }

  // =====** 组件：区块⑥ 审计与保留 =====**

  Widget _buildAuditSection(int retention) {
    return RdcSectionCard(
      title: AppStrings.permAuditSection,
      child: RdcFieldRow(
        label: AppStrings.retentionLabel,
        child: Row(
          children: [
            for (final days in const [7, 30, 90]) ...[
              if (days != 7) const SizedBox(width: RdcTokens.spaceSm),
              RdcStateButton(
                label: '$days 天',
                active: retention == days,
                onPressed: () => widget.controller.setAuditRetention(days),
              ),
            ],
          ],
        ),
      ),
    );
  }

  // =====** 组件：区块⑦ 信任设备 =====**

  Widget _buildTrustSection(
    String localPair,
    String sameAccount,
    bool passwordless,
    List<TrustedDeviceRow> devices,
  ) {
    return RdcSectionCard(
      title: AppStrings.permTrustSection,
      trailing: RdcStatusPill(
        text: '${devices.length} 台',
        color: RdcTokens.accent,
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          RdcFieldRow(
            label: AppStrings.localPairPolicyLabel,
            child: Row(
              children: [
                RdcStateButton(
                  label: AppStrings.policyAskShort,
                  active: localPair == 'ask',
                  onPressed: () =>
                      widget.controller.setLocalPairPolicy('ask'),
                ),
                const SizedBox(width: RdcTokens.spaceSm),
                RdcStateButton(
                  label: AppStrings.policyAutoShort,
                  active: localPair == 'auto',
                  onPressed: () =>
                      widget.controller.setLocalPairPolicy('auto'),
                ),
              ],
            ),
          ),
          const SizedBox(height: RdcTokens.spaceMd),
          RdcFieldRow(
            label: AppStrings.sameAccountPolicyLabel,
            child: Row(
              children: [
                RdcStateButton(
                  label: AppStrings.policyAskShort,
                  active: sameAccount == 'ask',
                  onPressed: () =>
                      widget.controller.setSameAccountPolicy('ask'),
                ),
                const SizedBox(width: RdcTokens.spaceSm),
                RdcStateButton(
                  label: AppStrings.policyAutoShort,
                  active: sameAccount == 'auto',
                  onPressed: () =>
                      widget.controller.setSameAccountPolicy('auto'),
                ),
              ],
            ),
          ),
          const SizedBox(height: RdcTokens.spaceMd),
          RdcStateButton(
            label: AppStrings.passwordlessLabel,
            icon: Icons.lock_open,
            active: passwordless,
            onPressed: () =>
                widget.controller.setPasswordless(!passwordless),
          ),
          const SizedBox(height: RdcTokens.spaceLg),
          const Text(
            AppStrings.trustHint,
            style: TextStyle(
              fontSize: RdcTokens.fontCaption,
              color: RdcTokens.textMuted,
            ),
          ),
          const SizedBox(height: RdcTokens.spaceMd),
          if (devices.isEmpty)
            const Text(
              '暂无信任设备；在设备列表里把设备标记为信任后会出现在这里。',
              style: TextStyle(
                fontSize: RdcTokens.fontCaption,
                color: RdcTokens.textMuted,
              ),
            )
          else
            for (final device in devices)
              Padding(
                padding: const EdgeInsets.only(bottom: RdcTokens.spaceSm),
                child: Container(
                  padding: const EdgeInsets.symmetric(
                    horizontal: RdcTokens.spaceMd,
                    vertical: RdcTokens.spaceSm,
                  ),
                  decoration: BoxDecoration(
                    color: RdcTokens.surfaceRaised,
                    borderRadius: BorderRadius.circular(RdcTokens.radiusControl),
                    border: Border.all(color: RdcTokens.border),
                  ),
                  child: Row(
                    children: [
                      Icon(
                        Icons.verified_user,
                        size: 16,
                        color: RdcTokens.success,
                      ),
                      const SizedBox(width: RdcTokens.spaceSm),
                      Expanded(
                        child: Text(
                          '${device.name} · ${device.id}',
                          overflow: TextOverflow.ellipsis,
                          style: const TextStyle(fontSize: RdcTokens.fontBody),
                        ),
                      ),
                      if (device.sameAccount)
                        const RdcStatusPill(
                          text: '同账号',
                          color: RdcTokens.textMuted,
                        ),
                      const SizedBox(width: RdcTokens.spaceSm),
                      RdcButton(
                        label: '取消信任',
                        icon: Icons.link_off,
                        onPressed: () => _untrust(device.id),
                      ),
                    ],
                  ),
                ),
              ),
        ],
      ),
    );
  }

  /// 取消信任并提示。
  Future<void> _untrust(String deviceId) async {
    final message = await widget.controller.untrust(deviceId);
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(content: Text(message ?? '已取消信任')),
    );
  }

  // =====** 组件：页面骨架 =====**

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text(AppStrings.permissionTitle)),
      body: SafeArea(
        child: SingleChildScrollView(
          padding: const EdgeInsets.all(RdcTokens.spacePage),
          child: Center(
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 900),
              child: ValueListenableBuilder(
                valueListenable: widget.controller.settings,
                builder: (context, settings, _) {
                  if (settings == null) {
                    return const Padding(
                      padding: EdgeInsets.all(RdcTokens.spaceXl),
                      child: Center(child: CircularProgressIndicator()),
                    );
                  }
                  return ValueListenableBuilder(
                    valueListenable: widget.controller.trustedDevices,
                    builder: (context, devices, _) {
                      return Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          _buildSwitchSection(settings.remoteAccessEnabled),
                          const SizedBox(height: RdcTokens.spaceLg),
                          _buildConcurrentSection(
                            settings.concurrentMode,
                            settings.takeoverPolicy,
                            settings.takeoverTimeoutS,
                          ),
                          const SizedBox(height: RdcTokens.spaceLg),
                          _buildTemplateSection(
                            settings.permissionTemplate,
                            settings.permissionBits,
                          ),
                          const SizedBox(height: RdcTokens.spaceLg),
                          _buildGrantsSection(),
                          const SizedBox(height: RdcTokens.spaceLg),
                          _buildNoticeSection(settings.localNotice),
                          const SizedBox(height: RdcTokens.spaceLg),
                          _buildAuditSection(settings.auditRetentionDays),
                          const SizedBox(height: RdcTokens.spaceLg),
                          _buildTrustSection(
                            settings.localPairPolicy,
                            settings.sameAccountPolicy,
                            settings.passwordlessPolicy,
                            devices,
                          ),
                        ],
                      );
                    },
                  );
                },
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// 标签在上、控件在下的行容器（权限页内部复用）。
class RdcFieldRow extends StatelessWidget {
  /// 创建行。
  ///
  /// [label] 标签；[child] 控件。
  const RdcFieldRow({super.key, required this.label, required this.child});

  /// 标签。
  final String label;

  /// 控件。
  final Widget child;

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(
          label,
          style: const TextStyle(
            fontSize: RdcTokens.fontCaption,
            color: RdcTokens.textMuted,
          ),
        ),
        const SizedBox(height: 6),
        child,
      ],
    );
  }
}
