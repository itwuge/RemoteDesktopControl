// ============================================================
// 组件用途：分组卡片（设置页/首页统一的分组容器）
// 由谁组装：view/pages/home_page.dart、view/pages/settings_page.dart
// 数据来源：由调用方传入 title 与 child，组件自身无状态
// ============================================================

import 'package:flutter/material.dart';

import '../../core/theme/tokens.dart';

/// 带标题的分组卡片。
class RdcSectionCard extends StatelessWidget {
  /// 创建分组卡片。
  ///
  /// [title] 为分组标题；[child] 为内容；[trailing] 可选的右上角操作区。
  const RdcSectionCard({
    super.key,
    required this.title,
    required this.child,
    this.trailing,
  });

  /// 分组标题。
  final String title;

  /// 分组内容。
  final Widget child;

  /// 右上角操作区（如状态徽章）。
  final Widget? trailing;

  @override
  Widget build(BuildContext context) {
    return Container(
      width: double.infinity,
      padding: const EdgeInsets.all(RdcTokens.spaceCard),
      decoration: BoxDecoration(
        color: RdcTokens.surface,
        borderRadius: BorderRadius.circular(RdcTokens.radiusCard),
        border: Border.all(color: RdcTokens.border),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          // =====** 标题行 =====**
          Row(
            children: [
              Expanded(
                child: Text(
                  title,
                  style: const TextStyle(
                    fontSize: RdcTokens.fontSection,
                    fontWeight: FontWeight.w600,
                    color: RdcTokens.textPrimary,
                  ),
                ),
              ),
              ?trailing,
            ],
          ),
          const SizedBox(height: RdcTokens.spaceMd),
          // =====** 内容区 =====**
          child,
        ],
      ),
    );
  }
}
