// ============================================================
// 页面用途：远程访问权限页（按 docs/13 的 7 个区块）
//
// 层级：view/pages
// 被谁组装：settings_page.dart 通过 Navigator 打开
// 数据来源：
//   - PermissionController.settings：策略写入 Rust 设置文件
//   - PermissionController.kernel：授权/本机保护区/信任设备/审计（S5-5 真内核）
// 布局规矩：上下单栏、分组框、标签在上控件在下；条目后置操作按钮
// ============================================================

import 'package:flutter/material.dart';

import '../../controller/permission_controller.dart';
import '../../core/l10n/strings.dart';
import '../../core/theme/tokens.dart';
import '../../model/gateway/rust_types.dart';
import '../components/fields.dart';
import '../components/home_access_card.dart';
import '../components/info_row.dart';
import '../components/rdc_button.dart';
import '../components/rdc_dialog.dart';
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
  /// 审计 kind → 中文（把内核英文枚举转成用户能读的词）。
  static const Map<String, String> _auditKindLabels = {
    'takeover': '接管',
    'grant': '授权',
    'grant_updated': '授权变更',
    'kick': '断开',
    'session': '会话',
    'remote_access': '远程访问',
    'stop': '停止访问',
    'resume': '恢复访问',
    'untrust': '取消信任',
  };

  /// 审计 action → 中文。
  static const Map<String, String> _auditActionLabels = {
    'owner_approve': '本机确认',
    'controller_yield': '控制者让位',
    'auto_approve': '自动接管',
    'deny': '拒绝',
    'create': '创建',
    'update': '更新',
    'reissue': '重签',
    'revoke': '吊销',
    'remove': '移除',
    'stop': '停止',
    'resume': '恢复',
    'kick': '踢出',
    'connect': '连接',
    'disconnect': '断开',
    'untrust': '取消信任',
  };

  /// 审计 outcome → 中文。
  static const Map<String, String> _auditOutcomeLabels = {
    'allowed': '允许',
    'denied': '拒绝',
    'ok': '成功',
    'yielded': '已让位',
    'revoked': '已吊销',
    'failed': '失败',
  };

  /// 模板名 → 中文标签。
  static const Map<String, String> _templateLabels = {
    'readonly': '只观看',
    'operate': '可操作',
    'standard': '可操作',
    'full': '完全控制',
    'custom': '自定义',
  };

  @override
  void initState() {
    super.initState();
    widget.controller.load();
  }

  // =====** 组件：区块① 远程访问 =====**

  Widget _buildSwitchSection(
    PermissionSnapshot? snapshot,
    bool settingsEnabled,
  ) {
    final enabled = snapshot?.remoteAccessEnabled ?? settingsEnabled;
    final error = widget.controller.errorMessage.value;
    // 旧实现是一个「禁用却高亮」的伪按钮 + 右上角重复胶囊；
    // 现在改为「一句状态 + 一个动作」的状态卡（清单 F1 / F4 / P0-12）。
    return RdcAccessCard(
      title: AppStrings.permSwitchSection,
      enabled: enabled,
      onlineLabel:
          snapshot?.connectedLabel ?? AppStrings.homeAccessNoSession,
      busy: widget.controller.saving.value,
      // 内核快照读不到且已有错误时，显示明确空状态而不是猜状态。
      unavailable: snapshot == null && error != null,
      onStop: _stopRemoteAccess,
      onResume: _resumeRemoteAccess,
      onRetry: () => widget.controller.loadKernel(),
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
          const SizedBox(height: RdcTokens.spaceSm),
          const Text(
            '接管流程：控制者 15 秒未响应视为让位；本机二次确认 30 秒未操作则拒绝。',
            style: TextStyle(
              fontSize: RdcTokens.fontCaption,
              color: RdcTokens.textMuted,
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
          const SizedBox(height: RdcTokens.spaceSm),
          const Text(
            '重启/关机与切换显示器属于敏感位：即使勾选，连接时仍需本机逐次确认。',
            style: TextStyle(
              fontSize: RdcTokens.fontCaption,
              color: RdcTokens.textMuted,
            ),
          ),
        ],
      ),
    );
  }

  // =====** 组件：区块④ 授权访问者（真内核） =====**

  Widget _buildGrantsSection(PermissionSnapshot? snapshot) {
    final grants = snapshot?.grants ?? const <GrantEntry>[];
    final activeCount = snapshot?.activeGrants ?? 0;
    return RdcSectionCard(
      title: AppStrings.permGrantsSection,
      trailing: RdcStatusPill(
        text: '$activeCount 条生效',
        color: activeCount > 0 ? RdcTokens.accent : RdcTokens.textMuted,
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              RdcButton(
                label: AppStrings.grantAdd,
                icon: Icons.add_moderator,
                onPressed: () => _showCreateGrantDialog(snapshot),
              ),
              const SizedBox(width: RdcTokens.spaceMd),
              const Expanded(
                child: Text(
                  '只授权给已配对设备；权限只能削减，提权需换新令牌重签。',
                  style: TextStyle(
                    fontSize: RdcTokens.fontCaption,
                    color: RdcTokens.textMuted,
                  ),
                ),
              ),
            ],
          ),
          const SizedBox(height: RdcTokens.spaceMd),
          if (grants.isEmpty)
            _buildEmptyBox(AppStrings.grantsEmpty)
          else
            for (final grant in grants)
              Padding(
                padding: const EdgeInsets.only(bottom: RdcTokens.spaceSm),
                child: _buildGrantRow(grant),
              ),
          const SizedBox(height: RdcTokens.spaceSm),
          Text(
            snapshot?.revocationNote ?? _fallbackRevocationNote,
            style: const TextStyle(
              fontSize: RdcTokens.fontCaption,
              color: RdcTokens.textMuted,
            ),
          ),
        ],
      ),
    );
  }

  /// 与 Rust 保持一致的兜底文案（不得写「立即生效」）。
  static const String _fallbackRevocationNote =
      '撤销将在最长 15 分 90 秒内在各设备生效；本机可立即断开当前会话（踢出）。';

  Widget _buildGrantRow(GrantEntry grant) {
    final revoked = grant.revoked;
    return Container(
      padding: const EdgeInsets.symmetric(
        horizontal: RdcTokens.spaceMd,
        vertical: RdcTokens.spaceSm,
      ),
      decoration: BoxDecoration(
        color: RdcTokens.surfaceRaised,
        borderRadius: BorderRadius.circular(RdcTokens.radiusControl),
        border: Border.all(color: RdcTokens.border),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Icon(
                revoked ? Icons.link_off : Icons.verified_user,
                size: 16,
                color: revoked ? RdcTokens.textMuted : RdcTokens.success,
              ),
              const SizedBox(width: RdcTokens.spaceSm),
              Expanded(
                child: Text(
                  '${grant.granteeName} · '
                  '${grant.granteeFingerprint.isEmpty ? '设备标识未采集' : grant.granteeFingerprint}',
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(fontSize: RdcTokens.fontBody),
                ),
              ),
              RdcStatusPill(
                text: revoked ? AppStrings.grantRevoked : '生效',
                color: revoked ? RdcTokens.textMuted : RdcTokens.success,
              ),
            ],
          ),
          const SizedBox(height: RdcTokens.spaceSm),
          Text(
            '模板：${_templateLabels[grant.template] ?? '自定义'}'
            ' · 令牌权限：${_permsText(grant.perms)}'
            ' · 本机生效：${_permsText(grant.effectivePerms)}',
            style: const TextStyle(
              fontSize: RdcTokens.fontCaption,
              color: RdcTokens.textMuted,
            ),
          ),
          Text(
            '到期：${_formatMs(grant.expiresAtMs)}'
            ' · 授权 ID：${grant.grantId}',
            style: const TextStyle(
              fontSize: RdcTokens.fontCaption,
              color: RdcTokens.textMuted,
            ),
          ),
          if (!revoked) ...[
            const SizedBox(height: RdcTokens.spaceSm),
            Wrap(
              spacing: RdcTokens.spaceSm,
              runSpacing: RdcTokens.spaceSm,
              children: [
                RdcButton(
                  label: AppStrings.grantReissue,
                  icon: Icons.tune,
                  onPressed: () => _showReissueDialog(grant),
                ),
                RdcButton(
                  label: AppStrings.grantRevoke,
                  icon: Icons.block,
                  danger: true,
                  onPressed: () => _revokeGrant(grant),
                ),
                RdcButton(
                  label: '移除',
                  icon: Icons.delete_outline,
                  onPressed: () => _removeGrant(grant),
                ),
              ],
            ),
          ],
        ],
      ),
    );
  }

  /// 权限位 → 可读文本。
  String _permsText(int perms) {
    final names = <String>[];
    for (var bit = 0;
        bit < PermissionController.permissionBitLabels.length;
        bit++) {
      if ((perms & (1 << bit)) != 0) {
        names.add(PermissionController.permissionBitLabels[bit]);
      }
    }
    return names.isEmpty ? '无' : names.join(' / ');
  }

  /// Unix 毫秒 → `yyyy-MM-dd`。
  String _formatMs(int ms) {
    if (ms <= 0) return '—';
    final time = DateTime.fromMillisecondsSinceEpoch(ms);
    String two(int value) => value.toString().padLeft(2, '0');
    return '${time.year}-${two(time.month)}-${two(time.day)} '
        '${two(time.hour)}:${two(time.minute)}';
  }

  Widget _buildEmptyBox(String text) {
    return Container(
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
      child: Text(
        text,
        style: const TextStyle(
          fontSize: RdcTokens.fontCaption,
          color: RdcTokens.textMuted,
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

  Widget _buildAuditSection(int retention, PermissionSnapshot? snapshot) {
    final audit = snapshot?.auditTail ?? const <PermissionAuditEntry>[];
    return RdcSectionCard(
      title: AppStrings.permAuditSection,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          RdcFieldRow(
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
          const SizedBox(height: RdcTokens.spaceMd),
          const Text(
            '最近操作（只记元数据：时间、主体、动作、旧值/新值；不记画面与内容）',
            style: TextStyle(
              fontSize: RdcTokens.fontCaption,
              color: RdcTokens.textMuted,
            ),
          ),
          const SizedBox(height: RdcTokens.spaceSm),
          if (audit.isEmpty)
            _buildEmptyBox(AppStrings.auditEmpty)
          else
            for (final entry in audit.reversed.take(8))
              Padding(
                padding: const EdgeInsets.only(bottom: 4),
                child: Text(
                  // 英文枚举转中文，用户不需要读懂内核术语（清单 F5）。
                  '${_formatMs(entry.tsMs)} · '
                  '${_auditKindLabels[entry.kind] ?? entry.kind} / '
                  '${_auditActionLabels[entry.action] ?? entry.action}'
                  '${entry.oldValue.isEmpty ? '' : '（${entry.oldValue} → ${entry.newValue}）'}'
                  ' · ${_auditOutcomeLabels[entry.outcome] ?? entry.outcome}',
                  style: const TextStyle(
                    fontSize: RdcTokens.fontCaption,
                    color: RdcTokens.textMuted,
                  ),
                ),
              ),
        ],
      ),
    );
  }

  // =====** 组件：区块⑦ 信任设备 =====**

  Widget _buildTrustSection(
    String localPair,
    String sameAccount,
    bool passwordless,
    PermissionSnapshot? snapshot,
  ) {
    final devices = snapshot?.trustDevices ?? const <TrustDeviceEntry>[];
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
            '信任设备命中且公钥一致时跳过配对码，走密钥挑战-应答；账号服务不可达时按本地信任回退，不影响连接。',
            style: TextStyle(
              fontSize: RdcTokens.fontCaption,
              color: RdcTokens.textMuted,
            ),
          ),
          const SizedBox(height: RdcTokens.spaceMd),
          if (devices.isEmpty)
            _buildEmptyBox(
              '暂无信任设备；在设备列表里把设备标记为信任后会出现在这里。',
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
                        child: Column(
                          crossAxisAlignment: CrossAxisAlignment.start,
                          children: [
                            Text(
                              '${device.name} · ${device.deviceId}',
                              overflow: TextOverflow.ellipsis,
                              style:
                                  const TextStyle(fontSize: RdcTokens.fontBody),
                            ),
                            Text(
                              device.fingerprint.isEmpty
                                  ? '设备标识未采集（需重新配对）'
                                  : '${AppStrings.grantFingerprintLabel} ${device.fingerprint}',
                              style: const TextStyle(
                                fontSize: RdcTokens.fontCaption,
                                color: RdcTokens.textMuted,
                              ),
                            ),
                          ],
                        ),
                      ),
                      if (device.sameAccount)
                        const RdcStatusPill(
                          text: '同账号',
                          color: RdcTokens.textMuted,
                        ),
                      const SizedBox(width: RdcTokens.spaceSm),
                      RdcStatusPill(
                        text: device.authMode == 'passwordless' ? '免密直连' : '需配对码',
                        // 「需配对码」是正常的安全策略，不是错误（清单 G6）。
                        color: device.authMode == 'passwordless'
                            ? RdcTokens.success
                            : RdcTokens.textMuted,
                      ),
                      const SizedBox(width: RdcTokens.spaceSm),
                      RdcButton(
                        label: '取消信任',
                        icon: Icons.link_off,
                        onPressed: () => _untrust(device),
                      ),
                    ],
                  ),
                ),
              ),
        ],
      ),
    );
  }

  // =====** 组件：本机保护区 =====**

  Widget _buildLocalZoneSection(PermissionSnapshot? snapshot) {
    final fingerprint = snapshot?.identityFingerprint ?? '—';
    final localPerms = snapshot?.localPerms ?? 0;
    return RdcSectionCard(
      title: '本机保护区',
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          // 只保留用户能理解的标识；授权文件的绝对路径属于工程细节，已移除（清单 P0-6）。
          RdcInfoRow(label: AppStrings.grantFingerprintLabel, value: fingerprint),
          RdcInfoRow(
            label: '本机允许权限',
            value: _permsText(localPerms),
          ),
          RdcInfoRow(
            label: '撤销世代',
            value: '${snapshot?.revokedEpoch ?? 0}',
          ),
          if ((snapshot?.hint ?? '').isNotEmpty) ...[
            const SizedBox(height: RdcTokens.spaceSm),
            Text(
              snapshot!.hint,
              style: const TextStyle(
                fontSize: RdcTokens.fontCaption,
                color: RdcTokens.danger,
              ),
            ),
          ],
        ],
      ),
    );
  }

  // =====** 交互：授权增删改 =====**

  Future<void> _showCreateGrantDialog(PermissionSnapshot? snapshot) async {
    final candidates = (snapshot?.trustDevices ?? const <TrustDeviceEntry>[])
        .where((device) => device.passwordlessReady)
        .toList(growable: false);
    if (candidates.isEmpty) {
      _snack('没有可用信任设备：需要先配对并采集到公钥，才能签发授权。');
      return;
    }
    var deviceId = candidates.first.deviceId;
    var template = 'readonly';
    var days = 30;
    // 统一弹窗：标签在上、控件在下，不再使用 Material 浮标签（清单 G4 / P0-14）。
    final result = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => StatefulBuilder(
        builder: (context, setDialogState) => RdcDialog(
          title: '签发授权',
          width: RdcTokens.dialogWidthLarge,
          actions: [
            RdcButton(
              label: AppStrings.cancel,
              onPressed: () => Navigator.of(dialogContext).pop(false),
            ),
            RdcButton(
              label: '签发',
              primary: true,
              onPressed: () => Navigator.of(dialogContext).pop(true),
            ),
          ],
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              RdcField(
                label: '被授权设备',
                expand: false,
                child: RdcSelectField<String>(
                  value: deviceId,
                  items: [
                    for (final device in candidates)
                      DropdownMenuItem(
                        value: device.deviceId,
                        child: Text('${device.name}（${device.deviceId}）'),
                      ),
                  ],
                  onChanged: (value) {
                    if (value != null) setDialogState(() => deviceId = value);
                  },
                ),
              ),
              const SizedBox(height: RdcTokens.spaceLg),
              RdcField(
                label: '权限模板',
                expand: false,
                child: RdcSelectField<String>(
                  value: template,
                  items: const [
                    DropdownMenuItem(value: 'readonly', child: Text('只观看')),
                    DropdownMenuItem(value: 'operate', child: Text('可操作')),
                    DropdownMenuItem(value: 'full', child: Text('完全控制')),
                  ],
                  onChanged: (value) {
                    if (value != null) {
                      setDialogState(() => template = value);
                    }
                  },
                ),
              ),
              const SizedBox(height: RdcTokens.spaceLg),
              RdcField(
                label: '有效期',
                expand: false,
                child: RdcSelectField<int>(
                  value: days,
                  items: const [
                    DropdownMenuItem(value: 30, child: Text('30 天（默认）')),
                    DropdownMenuItem(value: 60, child: Text('60 天')),
                    DropdownMenuItem(value: 90, child: Text('90 天（最长）')),
                  ],
                  onChanged: (value) {
                    if (value != null) setDialogState(() => days = value);
                  },
                ),
              ),
              const SizedBox(height: RdcTokens.spaceMd),
              const Text(
                '完全控制不含重启/关机与切换显示器；敏感操作仍需本机逐次确认。',
                style: TextStyle(
                  fontSize: RdcTokens.fontCaption,
                  color: RdcTokens.textMuted,
                ),
              ),
            ],
          ),
        ),
      ),
    );
    if (result != true || !mounted) return;
    final error = await widget.controller.createGrantForDevice(
      deviceId: deviceId,
      template: template,
      validityDays: days,
    );
    _snack(error ?? '已签发授权');
  }

  Future<void> _showReissueDialog(GrantEntry grant) async {
    var template = grant.template == 'custom' ? 'operate' : grant.template;
    var days = 30;
    final result = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => StatefulBuilder(
        builder: (context, setDialogState) => RdcDialog(
          title: '改权限（换新令牌）',
          actions: [
            RdcButton(
              label: AppStrings.cancel,
              onPressed: () => Navigator.of(dialogContext).pop(false),
            ),
            RdcButton(
              label: '重签',
              primary: true,
              onPressed: () => Navigator.of(dialogContext).pop(true),
            ),
          ],
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              RdcField(
                label: '新模板',
                expand: false,
                child: RdcSelectField<String>(
                  value: template,
                  items: const [
                    DropdownMenuItem(value: 'readonly', child: Text('只观看')),
                    DropdownMenuItem(value: 'operate', child: Text('可操作')),
                    DropdownMenuItem(value: 'full', child: Text('完全控制')),
                  ],
                  onChanged: (value) {
                    if (value != null) {
                      setDialogState(() => template = value);
                    }
                  },
                ),
              ),
              const SizedBox(height: RdcTokens.spaceLg),
              RdcField(
                label: '新有效期',
                expand: false,
                child: RdcSelectField<int>(
                  value: days,
                  items: const [
                    DropdownMenuItem(value: 30, child: Text('30 天')),
                    DropdownMenuItem(value: 60, child: Text('60 天')),
                    DropdownMenuItem(value: 90, child: Text('90 天')),
                  ],
                  onChanged: (value) {
                    if (value != null) setDialogState(() => days = value);
                  },
                ),
              ),
              const SizedBox(height: RdcTokens.spaceMd),
              // 白话说明：为什么改权限要重连（清单 F2）。
              const Text(
                '放宽权限需要换发新令牌；旧令牌会同时失效，对方需要重新连接。',
                style: TextStyle(
                  fontSize: RdcTokens.fontCaption,
                  color: RdcTokens.textMuted,
                ),
              ),
            ],
          ),
        ),
      ),
    );
    if (result != true || !mounted) return;
    final error = await widget.controller.reissueGrant(
      grantId: grant.grantId,
      template: template,
      validityDays: days,
    );
    _snack(error ?? '已换发新令牌，旧令牌已吊销');
  }

  Future<void> _revokeGrant(GrantEntry grant) async {
    final confirmed = await _confirm(
      title: '吊销授权',
      body: '吊销不会立即生效，最长需要 15 分 90 秒同步到各设备。'
          '如需马上断开，请用「立即停止远程访问」。',
      confirmLabel: '吊销',
      danger: true,
    );
    if (!confirmed || !mounted) return;
    final error = await widget.controller.revokeGrant(grant.grantId);
    _snack(error ?? '已吊销；各设备将在最终一致窗口内生效');
  }

  Future<void> _removeGrant(GrantEntry grant) async {
    final confirmed = await _confirm(
      title: '移除授权记录',
      body: '只清理本机记录，不会广播撤销；已连接的对端不会因此断开。',
      confirmLabel: '移除',
    );
    if (!confirmed || !mounted) return;
    final error = await widget.controller.removeGrant(grant.grantId);
    _snack(error ?? '已移除记录');
  }

  Future<void> _stopRemoteAccess() async {
    final confirmed = await _confirm(
      title: AppStrings.stopConfirmTitle,
      body: AppStrings.stopConfirmBody,
      confirmLabel: '立即停止',
      danger: true,
    );
    if (!confirmed || !mounted) return;
    final report = await widget.controller.stopRemoteAccess();
    if (!mounted) return;
    if (report == null) {
      _snack(widget.controller.errorMessage.value ?? '停止失败');
      return;
    }
    // 主界面只给结果；输入/断流/收尾字段属于诊断细节，不再堆在提示条里。
    _snack(AppStrings.stopDone);
  }

  Future<void> _resumeRemoteAccess() async {
    final confirmed = await _confirm(
      title: AppStrings.resumeConfirmTitle,
      body: AppStrings.resumeConfirmBody,
      confirmLabel: '恢复',
    );
    if (!confirmed || !mounted) return;
    final error = await widget.controller.resumeRemoteAccess();
    _snack(error ?? AppStrings.resumeDone);
  }

  Future<void> _untrust(TrustDeviceEntry device) async {
    final message = await widget.controller.untrust(
      device.deviceId,
      name: device.name,
      sameAccount: device.sameAccount,
    );
    if (!mounted) return;
    _snack(message ?? '已取消信任；该设备下次连接必须配对码');
  }

  /// 统一二次确认（走 RdcDialog，边距/圆角/按钮与全应用一致）。
  Future<bool> _confirm({
    required String title,
    required String body,
    required String confirmLabel,
    bool danger = false,
  }) {
    return showRdcConfirmDialog(
      context,
      title: title,
      body: body,
      confirmLabel: confirmLabel,
      danger: danger,
    );
  }

  void _snack(String message) {
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(content: Text(message)),
    );
  }

  // =====** 组件：错误态 =====**

  Widget _buildErrorCard(String message) {
    return Padding(
      padding: const EdgeInsets.only(bottom: RdcTokens.spaceLg),
      child: RdcSectionCard(
        title: AppStrings.kernelError,
        trailing: const RdcStatusPill(
          text: '错误',
          color: RdcTokens.danger,
        ),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            SelectableText(
              message,
              style: const TextStyle(
                fontSize: RdcTokens.fontCaption,
                color: RdcTokens.danger,
              ),
            ),
            const SizedBox(height: RdcTokens.spaceMd),
            RdcButton(
              label: AppStrings.retry,
              icon: Icons.refresh,
              onPressed: () => widget.controller.loadKernel(),
            ),
          ],
        ),
      ),
    );
  }

  // =====** 组件：页面骨架 =====**

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: SafeArea(
        child: Column(
          children: [
            // 页头：返回 + 18px 标题，与下方卡片左对齐；去掉默认 AppBar（清单 F3 / G3）。
            Padding(
              padding: const EdgeInsets.fromLTRB(
                RdcTokens.spacePage,
                RdcTokens.spaceMd,
                RdcTokens.spacePage,
                RdcTokens.spaceSm,
              ),
              child: Center(
                child: ConstrainedBox(
                  constraints: const BoxConstraints(
                    maxWidth: RdcTokens.pageMaxWidth,
                  ),
                  child: Row(
                    children: [
                      Semantics(
                        button: true,
                        label: '返回',
                        child: IconButton(
                          tooltip: '返回',
                          icon: const Icon(Icons.arrow_back, size: 20),
                          onPressed: () => Navigator.of(context).pop(),
                        ),
                      ),
                      const SizedBox(width: 4),
                      const Text(
                        AppStrings.permissionTitle,
                        style: TextStyle(
                          fontSize: RdcTokens.fontPageTitle,
                          fontWeight: FontWeight.w600,
                          color: RdcTokens.textPrimary,
                        ),
                      ),
                    ],
                  ),
                ),
              ),
            ),
            Expanded(
              child: SingleChildScrollView(
                padding: const EdgeInsets.fromLTRB(
                  RdcTokens.spacePage,
                  0,
                  RdcTokens.spacePage,
                  RdcTokens.spacePage,
                ),
                child: Center(
                  child: ConstrainedBox(
                    constraints: const BoxConstraints(
                      maxWidth: RdcTokens.pageMaxWidth,
                    ),
              child: ValueListenableBuilder(
                valueListenable: widget.controller.settings,
                builder: (context, settings, _) {
                  if (settings == null) {
                    return const Padding(
                      padding: EdgeInsets.all(RdcTokens.spaceXl),
                      child: Center(child: CircularProgressIndicator()),
                    );
                  }
                  return ValueListenableBuilder<PermissionSnapshot?>(
                    valueListenable: widget.controller.kernel,
                    builder: (context, snapshot, _) {
                      final error = widget.controller.errorMessage.value;
                      return Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          if (error != null) _buildErrorCard(error),
                          _buildSwitchSection(
                            snapshot,
                            settings.remoteAccessEnabled,
                          ),
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
                          _buildGrantsSection(snapshot),
                          const SizedBox(height: RdcTokens.spaceLg),
                          _buildLocalZoneSection(snapshot),
                          const SizedBox(height: RdcTokens.spaceLg),
                          _buildNoticeSection(settings.localNotice),
                          const SizedBox(height: RdcTokens.spaceLg),
                          _buildAuditSection(
                            settings.auditRetentionDays,
                            snapshot,
                          ),
                          const SizedBox(height: RdcTokens.spaceLg),
                          _buildTrustSection(
                            settings.localPairPolicy,
                            settings.sameAccountPolicy,
                            settings.passwordlessPolicy,
                            snapshot,
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
        ],
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
