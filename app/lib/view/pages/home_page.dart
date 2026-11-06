// ============================================================
// 页面用途：首页（版本/桥接状态 + 环境自检）
//
// 层级：view/pages
// 被谁组装：main.dart
// 数据来源：HomeController（版本信息、5 行自检）
// ============================================================

import 'package:flutter/material.dart';

import '../../controller/home_controller.dart';
import '../../controller/settings_controller.dart';
import '../../core/l10n/strings.dart';
import '../../core/theme/tokens.dart';
import '../components/info_row.dart';
import '../components/section_card.dart';
import 'settings_page.dart';

/// 应用首页。
class HomePage extends StatelessWidget {
  /// 创建首页。
  ///
  /// [controller] 持有首页状态；[settingsController] 仅用于跳转设置页时传递。
  const HomePage({
    super.key,
    required this.controller,
    required this.settingsController,
  });

  /// 首页控制器。
  final HomeController controller;

  /// 设置控制器（跳转用）。
  final SettingsController settingsController;

  // =====** 组件：页面骨架 =====**

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      body: SafeArea(
        child: SingleChildScrollView(
          padding: const EdgeInsets.all(RdcTokens.spacePage),
          child: Center(
            child: ConstrainedBox(
              constraints: const BoxConstraints(maxWidth: 920),
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  _buildHeader(context),
                  const SizedBox(height: RdcTokens.spaceXl),
                  _buildCoreInfoCard(),
                  const SizedBox(height: RdcTokens.spaceXl),
                  _buildSelfCheckCard(context),
                  const SizedBox(height: RdcTokens.spaceXl),
                  _buildFooter(),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }

  // =====** 组件：页头 =====**

  Widget _buildHeader(BuildContext context) {
    return Row(
      children: [
        // =====** 品牌标识 =====**
        Container(
          width: 44,
          height: 44,
          alignment: Alignment.center,
          decoration: BoxDecoration(
            color: RdcTokens.accent.withValues(alpha: 0.15),
            borderRadius: BorderRadius.circular(RdcTokens.radiusControl),
            border: Border.all(color: RdcTokens.accent.withValues(alpha: 0.5)),
          ),
          child: const Text(
            'R',
            style: TextStyle(
              fontSize: 22,
              fontWeight: FontWeight.w800,
              color: RdcTokens.accent,
            ),
          ),
        ),
        const SizedBox(width: RdcTokens.spaceMd),
        // =====** 标题与副标题 =====**
        const Expanded(
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(
                AppStrings.appName,
                style: TextStyle(
                  fontSize: RdcTokens.fontTitle,
                  fontWeight: FontWeight.w700,
                  color: RdcTokens.textPrimary,
                ),
              ),
              SizedBox(height: 2),
              Text(
                AppStrings.appTagline,
                style: TextStyle(fontSize: RdcTokens.fontCaption, color: RdcTokens.textMuted),
              ),
            ],
          ),
        ),
        IconButton(
          tooltip: AppStrings.settingsTitle,
          onPressed: () => _openSettings(context),
          icon: const Icon(Icons.settings_outlined),
        ),
      ],
    );
  }

  // =====** 组件：核心信息卡 =====**

  Widget _buildCoreInfoCard() {
    return ValueListenableBuilder(
      valueListenable: controller.coreInfo,
      builder: (context, info, _) {
        return RdcSectionCard(
          title: '运行状态',
          trailing: RdcStatusPill(
            text: info.isBridgeHealthy ? '桥接正常' : '桥接未就绪',
            color: info.isBridgeHealthy ? RdcTokens.success : RdcTokens.danger,
          ),
          child: Column(
            children: [
              RdcInfoRow(label: AppStrings.versionLabel, value: info.coreVersion),
              const Divider(height: 20),
              RdcInfoRow(label: AppStrings.platformLabel, value: info.platform),
              const Divider(height: 20),
              RdcInfoRow(
                label: AppStrings.bridgeLabel,
                value: info.bridgeStatus,
                valueColor: info.isBridgeHealthy ? RdcTokens.success : RdcTokens.danger,
              ),
              ValueListenableBuilder(
                valueListenable: controller.errorMessage,
                builder: (context, error, _) {
                  if (error == null) return const SizedBox.shrink();
                  return Padding(
                    padding: const EdgeInsets.only(top: RdcTokens.spaceMd),
                    child: Row(
                      children: [
                        const Icon(Icons.error_outline, size: 16, color: RdcTokens.danger),
                        const SizedBox(width: 6),
                        Expanded(
                          child: Text(
                            error,
                            style: const TextStyle(fontSize: RdcTokens.fontCaption, color: RdcTokens.danger),
                          ),
                        ),
                      ],
                    ),
                  );
                },
              ),
            ],
          ),
        );
      },
    );
  }

  // =====** 组件：自检卡 =====**

  Widget _buildSelfCheckCard(BuildContext context) {
    return RdcSectionCard(
      title: AppStrings.selfCheckTitle,
      trailing: ValueListenableBuilder(
        valueListenable: controller.isChecking,
        builder: (context, checking, _) {
          return FilledButton(
            onPressed: checking ? null : controller.runSelfCheck,
            child: Text(checking ? AppStrings.selfChecking : AppStrings.selfCheckButton),
          );
        },
      ),
      child: ValueListenableBuilder(
        valueListenable: controller.selfCheckLines,
        builder: (context, lines, _) {
          if (lines.isEmpty) {
            return const Text(
              AppStrings.selfCheckIdle,
              style: TextStyle(fontSize: RdcTokens.fontBody, color: RdcTokens.textMuted),
            );
          }
          return Column(
            children: [
              for (final line in lines)
                Padding(
                  padding: const EdgeInsets.symmetric(vertical: 5),
                  child: Row(
                    children: [
                      Icon(
                        line.startsWith('[OK]') ? Icons.check_circle_outline : Icons.cancel_outlined,
                        size: 18,
                        color: line.startsWith('[OK]') ? RdcTokens.success : RdcTokens.danger,
                      ),
                      const SizedBox(width: RdcTokens.spaceSm),
                      Expanded(
                        child: Text(
                          line,
                          style: const TextStyle(
                            fontSize: RdcTokens.fontBody,
                            color: RdcTokens.textPrimary,
                            fontFamily: 'monospace',
                          ),
                        ),
                      ),
                    ],
                  ),
                ),
            ],
          );
        },
      ),
    );
  }

  // =====** 组件：页脚 =====**

  Widget _buildFooter() {
    return const Text(
      'S1 · 可构建空壳骨架',
      style: TextStyle(fontSize: RdcTokens.fontCaption, color: RdcTokens.textMuted),
    );
  }

  // =====** 行为：跳转设置 =====**

  void _openSettings(BuildContext context) {
    Navigator.of(context).push(
      MaterialPageRoute<void>(
        builder: (_) => SettingsPage(controller: settingsController),
      ),
    );
  }
}
