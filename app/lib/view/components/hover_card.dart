// ============================================================
// 组件用途：悬停上浮 + 阴影的卡片容器
// 由谁组装：设备列表卡片、屏幕墙宫格
// 数据来源：调用方传入 child 与可选内边距
// ============================================================

import 'package:flutter/material.dart';

import '../../core/theme/tokens.dart';

/// 悬停时上浮 2px 并加深阴影的卡片。
class RdcHoverCard extends StatefulWidget {
  /// 创建卡片。
  ///
  /// [child] 内容；[padding] 内边距；[onTap] 可选点击；[expand] 是否撑满。
  const RdcHoverCard({
    super.key,
    required this.child,
    this.padding = const EdgeInsets.all(RdcTokens.spaceCard),
    this.onTap,
    this.expand = true,
  });

  /// 内容。
  final Widget child;

  /// 内边距。
  final EdgeInsetsGeometry padding;

  /// 点击回调。
  final VoidCallback? onTap;

  /// 是否撑满宽度。
  final bool expand;

  @override
  State<RdcHoverCard> createState() => _RdcHoverCardState();
}

/// 悬停状态。
class _RdcHoverCardState extends State<RdcHoverCard> {
  /// 鼠标是否在卡片上。
  bool _hovering = false;

  @override
  Widget build(BuildContext context) {
    return MouseRegion(
      cursor: widget.onTap == null
          ? SystemMouseCursors.basic
          : SystemMouseCursors.click,
      onEnter: (_) => setState(() => _hovering = true),
      onExit: (_) => setState(() => _hovering = false),
      child: AnimatedContainer(
        duration: const Duration(milliseconds: 160),
        curve: Curves.easeOut,
        transform: Matrix4.translationValues(0, _hovering ? -2 : 0, 0),
        width: widget.expand ? double.infinity : null,
        decoration: BoxDecoration(
          color: _hovering ? RdcTokens.surfaceRaised : RdcTokens.surface,
          borderRadius: BorderRadius.circular(RdcTokens.radiusCard),
          border: Border.all(color: RdcTokens.border),
          boxShadow: _hovering
              ? const [
                  BoxShadow(
                    color: Color(0x33000000),
                    blurRadius: 18,
                    offset: Offset(0, 6),
                  ),
                ]
              : const [],
        ),
        child: Material(
          color: Colors.transparent,
          child: InkWell(
            borderRadius: BorderRadius.circular(RdcTokens.radiusCard),
            onTap: widget.onTap,
            child: Padding(padding: widget.padding, child: widget.child),
          ),
        ),
      ),
    );
  }
}
