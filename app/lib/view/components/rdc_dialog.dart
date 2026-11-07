// ============================================================
// 组件用途：统一弹窗（边距 24/28、圆角 16、宽 480/560、按钮 36）
// 由谁组装：设备、设置、权限、会话等页面
// 数据来源：调用方传入标题、内容与操作按钮
// 依据：清单 G2 / G9 / P0-14（去掉 Material 默认浮动标签与超大圆角）
// ============================================================

import 'package:flutter/material.dart';

import '../../core/theme/tokens.dart';
import 'rdc_button.dart';

/// 统一的弹窗容器。
class RdcDialog extends StatelessWidget {
  /// 创建弹窗。
  ///
  /// [title] 标题；[child] 弹窗内容（可用 [RdcField] 排字段）；
  /// [actions] 底部按钮（建议 [RdcButton]）；[width] 弹窗宽度；
  /// [padding] 内边距；[inset] 与其他组件的间距。
  const RdcDialog({
    super.key,
    required this.title,
    required this.child,
    this.actions = const <Widget>[],
    this.width = RdcTokens.dialogWidth,
    this.padding = RdcTokens.dialogPadding,
    this.inset = RdcTokens.dialogPadding,
  });

  /// 标题。
  final String title;

  /// 内容。
  final Widget child;

  /// 底部操作。
  final List<Widget> actions;

  /// 宽度上限。
  final double width;

  /// 内边距。
  final double padding;

  /// 外边距。
  final double inset;

  @override
  Widget build(BuildContext context) {
    final maxHeight = MediaQuery.sizeOf(context).height * 0.82;
    // 操作按钮之间统一 8px 间距，避免各页面各写一套。
    final actionRow = <Widget>[];
    for (var index = 0; index < actions.length; index++) {
      if (index > 0) actionRow.add(const SizedBox(width: 8));
      actionRow.add(actions[index]);
    }

    return Semantics(
      scopesRoute: true,
      namesRoute: true,
      explicitChildNodes: true,
      label: title,
      child: Dialog(
        insetPadding: EdgeInsets.all(inset),
        backgroundColor: RdcTokens.surface,
        surfaceTintColor: Colors.transparent,
        clipBehavior: Clip.antiAlias,
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(RdcTokens.radiusCard),
        ),
        child: ConstrainedBox(
          constraints: BoxConstraints(maxWidth: width, maxHeight: maxHeight),
          child: Padding(
            padding: EdgeInsets.all(padding),
            child: Column(
              mainAxisSize: MainAxisSize.min,
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Text(
                  title,
                  style: const TextStyle(
                    fontSize: RdcTokens.fontPageTitle,
                    fontWeight: FontWeight.w600,
                    color: RdcTokens.textPrimary,
                  ),
                ),
                const SizedBox(height: RdcTokens.spaceLg),
                // 内容过高时可滚动，保证小窗口下按钮不被挤出屏幕。
                Flexible(
                  child: SingleChildScrollView(
                    child: Align(
                      alignment: Alignment.topLeft,
                      child: child,
                    ),
                  ),
                ),
                if (actions.isNotEmpty) ...[
                  const SizedBox(height: RdcTokens.spaceLg),
                  Row(
                    mainAxisAlignment: MainAxisAlignment.end,
                    children: actionRow,
                  ),
                ],
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// 二次确认弹窗：返回是否确认。
///
/// [title] 标题；[body] 说明；[confirmLabel] 确认按钮文案；
/// [danger] 确认动作是否破坏性（红底）。
Future<bool> showRdcConfirmDialog(
  BuildContext context, {
  required String title,
  required String body,
  String confirmLabel = '确认',
  String cancelLabel = '取消',
  bool danger = false,
}) async {
  final result = await showDialog<bool>(
    context: context,
    builder: (dialogContext) {
      return RdcDialog(
        title: title,
        actions: [
          RdcButton(
            label: cancelLabel,
            onPressed: () => Navigator.of(dialogContext).pop(false),
          ),
          RdcButton(
            label: confirmLabel,
            primary: true,
            danger: danger,
            onPressed: () => Navigator.of(dialogContext).pop(true),
          ),
        ],
        // 正文说明用次要色，保持弹窗层次。
        child: Text(
          body,
          style: const TextStyle(
            fontSize: RdcTokens.fontBody,
            color: RdcTokens.textMuted,
            height: 1.6,
          ),
        ),
      );
    },
  );
  return result ?? false;
}
