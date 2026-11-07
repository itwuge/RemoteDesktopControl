// ============================================================
// 组件用途：本机可被控状态卡（谁能连、谁在连、一键停止）
// 由谁组装：shell_page（首页）与 permission_page（远程访问分组）
// 数据来源：PermissionController 的内核快照与设置；本组件只做展示与回调
// 依据：清单 A1 / A2 / A3 / F4（状态用文字+按钮表达，不用假开关）
// ============================================================

import 'package:flutter/material.dart';

import '../../core/l10n/strings.dart';
import '../../core/theme/tokens.dart';
import 'rdc_button.dart';
import 'section_card.dart';

/// 本机可被控状态卡。
class RdcAccessCard extends StatelessWidget {
  /// 创建状态卡。
  ///
  /// [enabled] 是否允许入站连接；[onlineLabel] 在线人数文案；
  /// [unavailable] 接口读取失败（显示明确空状态，不猜状态）；
  /// [busy] 正在停止/恢复；[onStop] 一键停止；[onResume] 恢复；
  /// [onRetry] 重新读取；[title] 分组标题。
  const RdcAccessCard({
    super.key,
    required this.enabled,
    required this.onlineLabel,
    this.unavailable = false,
    this.busy = false,
    this.onStop,
    this.onResume,
    this.onRetry,
    this.title = AppStrings.homeAccessSection,
  });

  /// 是否允许入站连接。
  final bool enabled;

  /// 在线人数文案。
  final String onlineLabel;

  /// 状态是否不可读。
  final bool unavailable;

  /// 是否处理中。
  final bool busy;

  /// 停止回调。
  final VoidCallback? onStop;

  /// 恢复回调。
  final VoidCallback? onResume;

  /// 重试回调。
  final VoidCallback? onRetry;

  /// 分组标题。
  final String title;

  @override
  Widget build(BuildContext context) {
    return RdcSectionCard(
      title: title,
      child: unavailable ? _buildUnavailable() : _buildStatus(),
    );
  }

  /// 接口不可读时的明确空状态（绝不用假开关冒充“已开启”）。
  Widget _buildUnavailable() {
    return Row(
      crossAxisAlignment: CrossAxisAlignment.center,
      children: [
        const Icon(Icons.cloud_off, size: 20, color: RdcTokens.textMuted),
        const SizedBox(width: 10),
        const Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                AppStrings.homeAccessUnknown,
                style: TextStyle(
                  fontSize: RdcTokens.fontBody,
                  fontWeight: FontWeight.w600,
                  color: RdcTokens.textPrimary,
                ),
              ),
              SizedBox(height: 4),
              Text(
                AppStrings.homeAccessUnknownHint,
                style: TextStyle(
                  fontSize: RdcTokens.fontCaption,
                  color: RdcTokens.textMuted,
                ),
              ),
            ],
          ),
        ),
        const SizedBox(width: 12),
        RdcButton(
          label: AppStrings.retry,
          onPressed: onRetry,
        ),
      ],
    );
  }

  /// 正常状态：一句话说清「是否可连 + 谁在连」，右侧只放一个动作。
  Widget _buildStatus() {
    final statusColor = enabled ? RdcTokens.success : RdcTokens.warning;
    final statusText = enabled
        ? AppStrings.homeAccessAllowed
        : AppStrings.homeAccessStopped;
    final action = enabled
        ? RdcButton(
            label: busy ? AppStrings.processing : AppStrings.stopRemote,
            danger: true,
            onPressed: busy ? null : onStop,
          )
        : RdcButton(
            label: busy ? AppStrings.processing : AppStrings.homeResume,
            primary: true,
            onPressed: busy ? null : onResume,
          );

    return Semantics(
      container: true,
      label: '${AppStrings.homeAccessSection}：$statusText，$onlineLabel',
      child: Row(
        crossAxisAlignment: CrossAxisAlignment.center,
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Row(
                  children: [
                    Icon(
                      enabled ? Icons.shield : Icons.shield_outlined,
                      size: 18,
                      color: statusColor,
                    ),
                    const SizedBox(width: 8),
                    Flexible(
                      child: Text(
                        statusText,
                        style: TextStyle(
                          fontSize: RdcTokens.fontPageTitle,
                          fontWeight: FontWeight.w600,
                          color: statusColor,
                        ),
                      ),
                    ),
                    const SizedBox(width: 8),
                    Flexible(
                      child: Text(
                        '· $onlineLabel',
                        overflow: TextOverflow.ellipsis,
                        style: const TextStyle(
                          fontSize: RdcTokens.fontCaption,
                          color: RdcTokens.textMuted,
                        ),
                      ),
                    ),
                  ],
                ),
                const SizedBox(height: 6),
                const Text(
                  AppStrings.permSwitchHint,
                  style: TextStyle(
                    fontSize: RdcTokens.fontCaption,
                    color: RdcTokens.textMuted,
                    height: 1.4,
                  ),
                ),
              ],
            ),
          ),
          const SizedBox(width: 16),
          action,
        ],
      ),
    );
  }
}
