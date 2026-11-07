// ============================================================
// 组件用途：表单字段容器（标签在上、控件在下）
// 由谁组装：设备编辑弹窗、设置页、权限页
// 数据来源：调用方传入 label 与 child
// ============================================================

import 'package:flutter/material.dart';

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
class RdcTextField extends StatelessWidget {
  /// 创建输入框。
  ///
  /// [controller] 输入控制器；[hint] 占位文案；[onChanged] 变更回调。
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
  Widget build(BuildContext context) {
    final border = OutlineInputBorder(
      borderRadius: BorderRadius.circular(RdcTokens.radiusControl),
      borderSide: const BorderSide(color: RdcTokens.border),
    );
    return TextField(
      controller: controller,
      obscureText: obscure,
      onChanged: onChanged,
      style: const TextStyle(fontSize: RdcTokens.fontBody),
      decoration: InputDecoration(
        hintText: hint,
        filled: true,
        fillColor: RdcTokens.surfaceRaised,
        isDense: true,
        contentPadding: const EdgeInsets.symmetric(horizontal: 12, vertical: 10),
        border: border,
        enabledBorder: border,
        focusedBorder: border.copyWith(
          borderSide: const BorderSide(color: RdcTokens.accent),
        ),
      ),
    );
  }
}
