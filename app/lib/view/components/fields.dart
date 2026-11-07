// ============================================================
// 组件用途：表单字段容器（标签在上、控件在下）
// 由谁组装：设备编辑弹窗、设置页、权限页
// 数据来源：调用方传入 label 与 child
// ============================================================

import 'package:flutter/material.dart';

import '../../core/l10n/strings.dart';
import '../../core/theme/tokens.dart';

/// 标签在上、控件在下的字段容器。
class RdcField extends StatelessWidget {
  /// 创建字段。
  ///
  /// [label] 标签；[child] 控件；[expand] 是否撑满；[width] 固定宽度（同一行多控件时使用）。
  const RdcField({
    super.key,
    required this.label,
    required this.child,
    this.expand = true,
    this.width,
  });

  /// 标签。
  final String label;

  /// 控件。
  final Widget child;

  /// 是否撑满剩余宽度。
  final bool expand;

  /// 固定宽度。
  final double? width;

  @override
  Widget build(BuildContext context) {
    final content = Column(
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
    if (width != null) return SizedBox(width: width, child: content);
    if (expand) return Expanded(child: content);
    return content;
  }
}

/// 单行输入框（统一深色底与圆角）。
class RdcTextField extends StatefulWidget {
  /// 创建输入框。
  ///
  /// [controller] 输入控制器；[hint] 占位文案；[onChanged] 变更回调；
  /// [obscure] 是否密码样式（自动带「显示/隐藏」眼睛按钮，星号右对齐）。
  const RdcTextField({
    super.key,
    required this.controller,
    this.hint,
    this.onChanged,
    this.obscure = false,
  });

  /// 输入控制器。
  final TextEditingController controller;

  /// 占位文案。
  final String? hint;

  /// 变更回调。
  final ValueChanged<String>? onChanged;

  /// 是否密码样式。
  final bool obscure;

  @override
  State<RdcTextField> createState() => _RdcTextFieldState();
}

/// 输入框状态：只维护「密码是否暂时可见」。
class _RdcTextFieldState extends State<RdcTextField> {
  /// 密码是否仍被遮住。
  bool _hidden = true;

  @override
  Widget build(BuildContext context) {
    final border = OutlineInputBorder(
      borderRadius: BorderRadius.circular(RdcTokens.radiusControl),
      borderSide: const BorderSide(color: RdcTokens.border),
    );
    return TextField(
      controller: widget.controller,
      obscureText: widget.obscure && _hidden,
      onChanged: widget.onChanged,
      // 密码星号靠右对齐，避免长度变化时文字左右跳动（清单 P2-2）。
      textAlign: widget.obscure ? TextAlign.right : TextAlign.start,
      style: const TextStyle(fontSize: RdcTokens.fontBody),
      decoration: InputDecoration(
        hintText: widget.hint,
        filled: true,
        fillColor: RdcTokens.surfaceRaised,
        isDense: true,
        contentPadding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
        border: border,
        enabledBorder: border,
        focusedBorder: border.copyWith(
          borderSide: const BorderSide(color: RdcTokens.accent),
        ),
        // 眼睛按钮：仅密码框出现，读屏读出「显示/隐藏密码」。
        suffixIcon: widget.obscure
            ? IconButton(
                tooltip: _hidden
                    ? AppStrings.showPassword
                    : AppStrings.hidePassword,
                icon: Icon(
                  _hidden ? Icons.visibility_off : Icons.visibility,
                  size: 18,
                  color: RdcTokens.textMuted,
                ),
                onPressed: () => setState(() => _hidden = !_hidden),
              )
            : null,
      ),
    );
  }
}

/// 下拉选择框（标签由外层 [RdcField] 提供，内部不再浮动标签）。
class RdcSelectField<T> extends StatelessWidget {
  /// 创建下拉框。
  ///
  /// [value] 当前值；[items] 选项；[onChanged] 变更回调；
  /// [enabled] 是否可用；[hint] 空值占位。
  const RdcSelectField({
    super.key,
    required this.value,
    required this.items,
    required this.onChanged,
    this.enabled = true,
    this.hint,
  });

  /// 当前值。
  final T? value;

  /// 选项。
  final List<DropdownMenuItem<T>> items;

  /// 变更回调。
  final ValueChanged<T?>? onChanged;

  /// 是否可用。
  final bool enabled;

  /// 空值占位。
  final String? hint;

  @override
  Widget build(BuildContext context) {
    final border = OutlineInputBorder(
      borderRadius: BorderRadius.circular(RdcTokens.radiusControl),
      borderSide: const BorderSide(color: RdcTokens.border),
    );
    return DropdownButtonFormField<T>(
      initialValue: value,
      isExpanded: true,
      items: items,
      hint: hint == null ? null : Text(hint!),
      onChanged: enabled ? onChanged : null,
      style: const TextStyle(
        fontSize: RdcTokens.fontBody,
        color: RdcTokens.textPrimary,
      ),
      dropdownColor: RdcTokens.surfaceRaised,
      borderRadius: BorderRadius.circular(RdcTokens.radiusControl),
      decoration: InputDecoration(
        filled: true,
        fillColor: RdcTokens.surfaceRaised,
        isDense: true,
        contentPadding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
        border: border,
        enabledBorder: border,
        disabledBorder: border.copyWith(
          borderSide: BorderSide(color: RdcTokens.border.withValues(alpha: 0.55)),
        ),
        focusedBorder: border.copyWith(
          borderSide: const BorderSide(color: RdcTokens.accent),
        ),
      ),
      // 读屏读出当前选择，而不是只有「下拉框」。
      selectedItemBuilder: (context) => items
          .map((item) => Align(
                alignment: Alignment.centerLeft,
                child: Text(
                  item.child is Text ? (item.child as Text).data ?? '' : '',
                  overflow: TextOverflow.ellipsis,
                  style: const TextStyle(
                    fontSize: RdcTokens.fontBody,
                    color: RdcTokens.textPrimary,
                  ),
                ),
              ))
          .toList(),
    );
  }
}

/// 空状态：图标 + 标题 + 说明（可选操作）。
class RdcEmptyState extends StatelessWidget {
  /// 创建空状态。
  ///
  /// [icon] 唯一图标；[title] 标题；[body] 说明；[action] 可选操作。
  const RdcEmptyState({
    super.key,
    required this.icon,
    required this.title,
    required this.body,
    this.action,
  });

  /// 图标。
  final IconData icon;

  /// 标题。
  final String title;

  /// 说明。
  final String body;

  /// 操作。
  final Widget? action;

  @override
  Widget build(BuildContext context) {
    // 空状态必须说「下一步做什么」，而不是只显示「暂无数据」。
    return Semantics(
      label: title,
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Icon(icon, size: 28, color: RdcTokens.textMuted),
          const SizedBox(height: RdcTokens.spaceMd),
          Text(
            title,
            style: const TextStyle(
              fontSize: RdcTokens.fontBody,
              fontWeight: FontWeight.w600,
              color: RdcTokens.textPrimary,
            ),
          ),
          const SizedBox(height: 6),
          Text(
            body,
            textAlign: TextAlign.center,
            style: const TextStyle(
              fontSize: RdcTokens.fontCaption,
              color: RdcTokens.textMuted,
              height: 1.5,
            ),
          ),
          if (action != null) ...[
            const SizedBox(height: RdcTokens.spaceMd),
            action!,
          ],
        ],
      ),
    );
  }
}

/// 诊断折叠区：默认收起，避免把工程细节摆在主界面。
class RdcDiagnostics extends StatelessWidget {
  /// 创建诊断区。
  ///
  /// [child] 诊断内容；[title] 标题。
  const RdcDiagnostics({
    super.key,
    required this.child,
    this.title = AppStrings.diagnostics,
  });

  /// 内容。
  final Widget child;

  /// 标题。
  final String title;

  @override
  Widget build(BuildContext context) {
    return Theme(
      // 去掉 ExpansionTile 默认的分割线，保持卡片内干净的视觉。
      data: Theme.of(context).copyWith(dividerColor: Colors.transparent),
      child: ExpansionTile(
        tilePadding: EdgeInsets.zero,
        childrenPadding: const EdgeInsets.only(bottom: 4),
        iconColor: RdcTokens.textMuted,
        collapsedIconColor: RdcTokens.textMuted,
        title: Text(
          title,
          style: const TextStyle(
            fontSize: RdcTokens.fontCaption,
            color: RdcTokens.textMuted,
            fontWeight: FontWeight.w600,
          ),
        ),
        children: [child],
      ),
    );
  }
}
