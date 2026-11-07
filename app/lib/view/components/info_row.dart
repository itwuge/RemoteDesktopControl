// ============================================================
// 组件用途：键值信息行 + 状态胶囊
// 由谁组装：view/pages/home_page.dart
// 数据来源：调用方传入 label/value/状态色
// ======================================================== */

import 'package:flutter/material.dart';

import '../../core/theme/tokens.dart';

/// 一行「标签 → 值」信息。
class RdcInfoRow extends StatelessWidget {
  /// 创建信息行。
  ///
  /// [label] 为左侧标签；[value] 为右侧值；[valueColor] 可选值颜色。
  const RdcInfoRow({
    super.key,
    required this.label,
    required this.value,
    this.valueColor = RdcTokens.textPrimary,
  });

  /// 左侧标签。
  final String label;

  /// 右侧值。
  final String value;

  /// 值颜色（如成功态用绿色）。
  final Color valueColor;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 6),
      child: Row(
        children: [
          SizedBox(
            width: 110,
            child: Text(
              label,
              style: const TextStyle(fontSize: RdcTokens.fontBody, color: RdcTokens.textMuted),
            ),
          ),
          Expanded(
            child: Text(
              value,
              style: TextStyle(
                fontSize: RdcTokens.fontBody,
                color: valueColor,
                fontWeight: FontWeight.w500,
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// 状态胶囊（如「已连接」「S1 空壳」）。
class RdcStatusPill extends StatelessWidget {
  /// 创建胶囊。
  ///
  /// [text] 为显示文字；[color] 为前景/边框色。
  const RdcStatusPill({super.key, required this.text, required this.color});

  /// 显示文字。
  final String text;

  /// 主题色。
  final Color color;

  @override
  Widget build(BuildContext context) {
    // 胶囊只读，读屏按文字播报即可。
    return Semantics(
      label: text,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 4),
        decoration: BoxDecoration(
          color: color.withValues(alpha: 0.12),
          borderRadius: BorderRadius.circular(RdcTokens.radiusPill),
          border: Border.all(color: color.withValues(alpha: 0.4)),
        ),
        child: Text(
          text,
          style: TextStyle(fontSize: RdcTokens.fontCaption, color: color, fontWeight: FontWeight.w600),
        ),
      ),
    );
  }
}
