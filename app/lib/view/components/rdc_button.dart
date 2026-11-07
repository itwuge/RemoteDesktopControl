// ============================================================
// 组件用途：统一按钮（高度 36 / 圆角 10 / 单个图标）
// 由谁组装：全部页面与对话框
// 数据来源：调用方传入文案、图标与回调
// ============================================================

import 'package:flutter/material.dart';

import '../../core/theme/tokens.dart';

/// 统一的方形操作按钮。
class RdcButton extends StatelessWidget {
  /// 创建按钮。
  ///
  /// [label] 文案；[icon] 唯一图标（功能区不重复放图标）；
  /// [onPressed] 为空表示禁用；[primary] 是否主按钮（蓝底白字）。
  const RdcButton({
    super.key,
    required this.label,
    this.icon,
    this.onPressed,
    this.primary = false,
    this.compact = false,
  });

  /// 文案。
  final String label;

  /// 单个图标。
  final IconData? icon;

  /// 点击回调；`null` 禁用。
  final VoidCallback? onPressed;

  /// 是否主按钮。
  final bool primary;

  /// 是否紧凑（无最小宽度）。
  final bool compact;

  @override
  Widget build(BuildContext context) {
    final style = ButtonStyle(
      minimumSize: WidgetStatePropertyAll(
        Size(compact ? 0 : 84, RdcTokens.controlHeight),
      ),
      padding: const WidgetStatePropertyAll(
        EdgeInsets.symmetric(horizontal: 12),
      ),
      shape: WidgetStatePropertyAll(
        RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(RdcTokens.radiusControl),
        ),
      ),
      textStyle: const WidgetStatePropertyAll(
        TextStyle(fontSize: RdcTokens.fontBody, fontWeight: FontWeight.w600),
      ),
    );
    final child = Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        if (icon != null) ...[
          Icon(icon, size: 16),
          const SizedBox(width: 6),
        ],
        Text(label),
      ],
    );
    if (primary) {
      return FilledButton(
        style: style,
        onPressed: onPressed,
        child: child,
      );
    }
    return OutlinedButton(
      style: style,
      onPressed: onPressed,
      child: child,
    );
  }
}

/// 状态按钮：用按钮态表达开关（不用 Switch）。
class RdcStateButton extends StatelessWidget {
  /// 创建状态按钮。
  ///
  /// [label] 文案；[active] 是否开启；[onPressed] 点击切换；[icon] 唯一图标。
  const RdcStateButton({
    super.key,
    required this.label,
    required this.active,
    this.icon,
    this.onPressed,
  });

  /// 文案。
  final String label;

  /// 是否开启。
  final bool active;

  /// 单个图标。
  final IconData? icon;

  /// 点击回调。
  final VoidCallback? onPressed;

  @override
  Widget build(BuildContext context) {
    final color = active ? RdcTokens.accent : RdcTokens.textMuted;
    return OutlinedButton(
      style: ButtonStyle(
        minimumSize: const WidgetStatePropertyAll(
          Size(84, RdcTokens.controlHeight),
        ),
        padding: const WidgetStatePropertyAll(
          EdgeInsets.symmetric(horizontal: 12),
        ),
        backgroundColor:
            WidgetStatePropertyAll(color.withValues(alpha: active ? 0.16 : 0.04)),
        side: WidgetStatePropertyAll(
          BorderSide(color: active ? RdcTokens.accent : RdcTokens.border),
        ),
        shape: WidgetStatePropertyAll(
          RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(RdcTokens.radiusControl),
          ),
        ),
      ),
      onPressed: onPressed,
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (icon != null) ...[
            Icon(icon, size: 16, color: color),
            const SizedBox(width: 6),
          ],
          Text(label, style: TextStyle(color: color)),
        ],
      ),
    );
  }
}
