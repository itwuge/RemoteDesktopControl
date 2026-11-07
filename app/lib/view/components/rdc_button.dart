// ============================================================
// 组件用途：统一按钮（高度 36 / 圆角 10 / 单个图标）
// 由谁组装：全部页面与对话框
// 数据来源：调用方传入文案、图标与回调
// 无障碍：按钮自带语义标签；禁用态同时改变配色与语义（清单 F1 / P1-19）
// ============================================================

import 'package:flutter/material.dart';

import '../../core/theme/tokens.dart';

/// 统一的方形操作按钮。
class RdcButton extends StatelessWidget {
  /// 创建按钮。
  ///
  /// [label] 文案；[icon] 唯一图标（功能区不重复放图标）；
  /// [onPressed] 为空表示禁用；[primary] 是否主按钮（蓝底白字）；
  /// [danger] 是否破坏性动作（红字红边；与 [primary] 同用时为红底白字）；
  /// [semanticLabel] 可选的无障碍标签，缺省用 [label]。
  const RdcButton({
    super.key,
    required this.label,
    this.icon,
    this.onPressed,
    this.primary = false,
    this.compact = false,
    this.danger = false,
    this.semanticLabel,
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

  /// 是否为破坏性动作（删除/断开/吊销）。
  final bool danger;

  /// 无障碍标签。
  final String? semanticLabel;

  @override
  Widget build(BuildContext context) {
    final enabled = onPressed != null;
    // 禁用态必须有可辨识的弱化配色，不能沿用「看起来还能点」的高亮。
    final disabledForeground = RdcTokens.textMuted.withValues(alpha: 0.65);
    final enabledForeground = danger
        ? RdcTokens.danger
        : (primary ? Colors.white : RdcTokens.textPrimary);

    final backgroundColor = WidgetStateProperty.resolveWith<Color>((states) {
      if (states.contains(WidgetState.disabled)) {
        return RdcTokens.surfaceRaised.withValues(alpha: 0.45);
      }
      if (primary && danger) return RdcTokens.danger;
      if (primary) return RdcTokens.accent;
      if (danger) return RdcTokens.danger.withValues(alpha: 0.10);
      return Colors.transparent;
    });
    final foregroundColor = WidgetStateProperty.resolveWith<Color>((states) {
      return states.contains(WidgetState.disabled)
          ? disabledForeground
          : enabledForeground;
    });
    final side = WidgetStateProperty.resolveWith<BorderSide>((states) {
      if (states.contains(WidgetState.disabled)) {
        return BorderSide(color: RdcTokens.border.withValues(alpha: 0.55));
      }
      if (danger) return const BorderSide(color: RdcTokens.danger);
      if (primary) return const BorderSide(color: RdcTokens.accent);
      return const BorderSide(color: RdcTokens.border);
    });

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
      backgroundColor: backgroundColor,
      foregroundColor: foregroundColor,
      side: side,
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
    final button = primary
        ? FilledButton(style: style, onPressed: onPressed, child: child)
        : OutlinedButton(style: style, onPressed: onPressed, child: child);
    // excludeSemantics：只暴露我们自己写的标签，避免内部 Text/Icon 再播报一遍。
    return Semantics(
      button: true,
      enabled: enabled,
      label: semanticLabel ?? label,
      excludeSemantics: true,
      child: button,
    );
  }
}

/// 带下拉菜单的次要按钮。
///
/// 低频/易误点的动作（吊销、移除、剪贴板、文件传输）收进「更多」，
/// 与主动作同一套 36 高 / 圆角 10 的视觉（清单 P1-9 / P1-16）。
class RdcMenuButton extends StatelessWidget {
  /// 创建菜单按钮。
  ///
  /// [label] 按钮文案；[itemBuilder] 菜单项；[onSelected] 选中回调。
  const RdcMenuButton({
    super.key,
    required this.label,
    required this.itemBuilder,
    required this.onSelected,
  });

  /// 按钮文案。
  final String label;

  /// 菜单项构造器。
  final PopupMenuItemBuilder<String> itemBuilder;

  /// 选中回调。
  final ValueChanged<String> onSelected;

  @override
  Widget build(BuildContext context) {
    return Semantics(
      button: true,
      label: label,
      excludeSemantics: true,
      child: PopupMenuButton<String>(
        tooltip: label,
        itemBuilder: itemBuilder,
        onSelected: onSelected,
        color: RdcTokens.surfaceRaised,
        offset: const Offset(0, RdcTokens.controlHeight),
        child: Container(
          height: RdcTokens.controlHeight,
          padding: const EdgeInsets.symmetric(horizontal: 12),
          decoration: BoxDecoration(
            color: RdcTokens.surfaceRaised,
            borderRadius: BorderRadius.circular(RdcTokens.radiusControl),
            border: Border.all(color: RdcTokens.border),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(
                label,
                style: const TextStyle(
                  fontSize: RdcTokens.fontBody,
                  fontWeight: FontWeight.w600,
                  color: RdcTokens.textPrimary,
                ),
              ),
              const SizedBox(width: 4),
              const Icon(
                Icons.expand_more,
                size: 16,
                color: RdcTokens.textMuted,
              ),
            ],
          ),
        ),
      ),
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

  /// 点击回调；`null` 表示只读/不可用。
  final VoidCallback? onPressed;

  @override
  Widget build(BuildContext context) {
    final enabled = onPressed != null;
    // 可点：开启蓝、关闭灰；不可用：统一弱化为灰（清单 F1「禁用却高亮」）。
    final color = enabled
        ? (active ? RdcTokens.accent : RdcTokens.textMuted)
        : RdcTokens.textMuted.withValues(alpha: 0.6);
    final background = enabled
        ? color.withValues(alpha: active ? 0.16 : 0.04)
        : RdcTokens.surfaceRaised.withValues(alpha: 0.4);
    final borderColor = enabled
        ? (active ? RdcTokens.accent : RdcTokens.border)
        : RdcTokens.border.withValues(alpha: 0.55);

    final button = OutlinedButton(
      style: ButtonStyle(
        minimumSize: const WidgetStatePropertyAll(
          Size(84, RdcTokens.controlHeight),
        ),
        padding: const WidgetStatePropertyAll(
          EdgeInsets.symmetric(horizontal: 12),
        ),
        backgroundColor: WidgetStatePropertyAll(background),
        side: WidgetStatePropertyAll(BorderSide(color: borderColor)),
        foregroundColor: WidgetStatePropertyAll(color),
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
    // 读屏需要知道「这是什么状态、现在能不能操作」。
    final stateText = active ? '已开启' : '已关闭';
    return Semantics(
      button: true,
      enabled: enabled,
      label: enabled ? '$label，$stateText' : '$label，$stateText，不可用',
      excludeSemantics: true,
      child: button,
    );
  }
}

/// 主导航标签：文字 + 底部选中条，不使用开关式蓝底蓝边（清单 A4 / P1-20）。
class RdcNavTab extends StatelessWidget {
  /// 创建导航标签。
  ///
  /// [label] 文案；[icon] 该功能区的唯一图标；[selected] 是否当前页；
  /// [onPressed] 点击切换。
  const RdcNavTab({
    super.key,
    required this.label,
    required this.selected,
    this.icon,
    this.onPressed,
  });

  /// 文案。
  final String label;

  /// 是否选中。
  final bool selected;

  /// 单个图标。
  final IconData? icon;

  /// 点击回调。
  final VoidCallback? onPressed;

  @override
  Widget build(BuildContext context) {
    final color = selected ? RdcTokens.accent : RdcTokens.textMuted;
    final button = TextButton(
      onPressed: onPressed,
      style: ButtonStyle(
        minimumSize: const WidgetStatePropertyAll(
          Size(0, RdcTokens.controlHeight + 6),
        ),
        padding: const WidgetStatePropertyAll(
          EdgeInsets.symmetric(horizontal: 12, vertical: 6),
        ),
        backgroundColor: const WidgetStatePropertyAll(Colors.transparent),
        overlayColor: WidgetStatePropertyAll(
          RdcTokens.accent.withValues(alpha: 0.08),
        ),
        shape: WidgetStatePropertyAll(
          RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(RdcTokens.radiusControl),
          ),
        ),
      ),
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              if (icon != null) ...[
                Icon(icon, size: 16, color: color),
                const SizedBox(width: 6),
              ],
              Text(
                label,
                style: TextStyle(
                  fontSize: RdcTokens.fontBody,
                  fontWeight: FontWeight.w600,
                  color: color,
                ),
              ),
            ],
          ),
          const SizedBox(height: 4),
          // 选中指示条：只表达「在哪一页」，不表达「开关已打开」。
          AnimatedContainer(
            duration: const Duration(milliseconds: 120),
            height: 2,
            width: selected ? 32 : 0,
            decoration: BoxDecoration(
              color: selected ? RdcTokens.accent : Colors.transparent,
              borderRadius: BorderRadius.circular(1),
            ),
          ),
        ],
      ),
    );
    return Semantics(
      button: true,
      selected: selected,
      label: label,
      excludeSemantics: true,
      child: button,
    );
  }
}
