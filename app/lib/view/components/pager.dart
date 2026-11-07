// ============================================================
// 组件用途：分页条（36px 按钮、10px 圆角、蓝底白字页码徽章）
// 由谁组装：view/pages/device_page.dart
// 数据来源：controller 的 page / pageCount 与回调
// ============================================================

import 'package:flutter/material.dart';

import '../../core/theme/tokens.dart';
import 'rdc_button.dart';

/// 卡片式分页条。
class RdcPager extends StatelessWidget {
  /// 创建分页条。
  ///
  /// [page] 当前页码（1 起）；[pageCount] 总页数；[onChanged] 翻页回调。
  const RdcPager({
    super.key,
    required this.page,
    required this.pageCount,
    required this.onChanged,
    this.total = 0,
  });

  /// 当前页。
  final int page;

  /// 总页数。
  final int pageCount;

  /// 翻页回调。
  final ValueChanged<int> onChanged;

  /// 总条目数（显示在左侧）。
  final int total;

  @override
  Widget build(BuildContext context) {
    // 只有一页时隐藏翻页控件与「1 / 1」徽章，避免无意义的高亮数字（清单 B6/B7）。
    if (pageCount <= 1) {
      return Container(
        padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
        decoration: BoxDecoration(
          color: RdcTokens.surface,
          borderRadius: BorderRadius.circular(RdcTokens.radiusCard),
          border: Border.all(color: RdcTokens.border),
        ),
        child: Text(
          '共 $total 台设备',
          style: const TextStyle(
            fontSize: RdcTokens.fontCaption,
            color: RdcTokens.textMuted,
          ),
        ),
      );
    }
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
      decoration: BoxDecoration(
        color: RdcTokens.surface,
        borderRadius: BorderRadius.circular(RdcTokens.radiusCard),
        border: Border.all(color: RdcTokens.border),
      ),
      child: Row(
        children: [
          Text(
            '共 $total 台设备',
            style: const TextStyle(
              fontSize: RdcTokens.fontCaption,
              color: RdcTokens.textMuted,
            ),
          ),
          const Spacer(),
          RdcButton(
            label: '上一页',
            icon: Icons.chevron_left,
            compact: true,
            onPressed: page > 1 ? () => onChanged(page - 1) : null,
          ),
          const SizedBox(width: RdcTokens.spaceSm),
          Container(
            height: RdcTokens.controlHeight,
            constraints: const BoxConstraints(minWidth: 36),
            alignment: Alignment.center,
            padding: const EdgeInsets.symmetric(horizontal: 10),
            decoration: BoxDecoration(
              color: RdcTokens.accent,
              borderRadius: BorderRadius.circular(RdcTokens.radiusControl),
            ),
            child: Text(
              '$page / $pageCount',
              style: const TextStyle(
                fontSize: RdcTokens.fontCaption,
                color: Colors.white,
                fontWeight: FontWeight.w600,
              ),
            ),
          ),
          const SizedBox(width: RdcTokens.spaceSm),
          RdcButton(
            label: '下一页',
            icon: Icons.chevron_right,
            compact: true,
            onPressed: page < pageCount ? () => onChanged(page + 1) : null,
          ),
        ],
      ),
    );
  }
}
