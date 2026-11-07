// ============================================================
// 组件用途：统一弹窗（边距 24/28、圆角 16、宽 480/560、按钮 36）
// 由谁组装：设备、设置、权限、会话等页面
// 数据来源：调用方传入标题、内容与操作按钮
// 依据：清单 G2 / G9 / P0-14（去掉 Material 默认浮动标签与超大圆角）
// ============================================================

import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../core/theme/tokens.dart';
import 'rdc_button.dart';

/// 统一的弹窗容器。
class RdcDialog extends StatelessWidget {
  /// 创建弹窗。
  ///
  /// [title] 标题；[child] 弹窗内容（可用 [RdcField] 排字段）；
  /// [actions] 底部按钮（建议 [RdcButton]）；[width] 弹窗宽度；
  /// [padding] 内边距；[inset] 与其他组件的间距；
  /// [onSubmit] 非破坏性弹窗的主动作：按 Enter 触发。
  ///
  /// **Enter 约定**：只有明确传入 [onSubmit] 才绑定 Enter；危险操作
  /// （吊销、移除、恢复默认等）不传，默认焦点落在「取消」上，
  /// 避免一路 Enter 误确认（清单 P2-6）。
  const RdcDialog({
    super.key,
    required this.title,
    required this.child,
    this.actions = const <Widget>[],
    this.width = RdcTokens.dialogWidth,
    this.padding = RdcTokens.dialogPadding,
    this.inset = RdcTokens.dialogPadding,
    this.onSubmit,
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

  /// Enter 触发的主动作；`null` 表示不绑定（危险操作必须传 `null`）。
  final VoidCallback? onSubmit;

  @override
  Widget build(BuildContext context) {
    final maxHeight = MediaQuery.sizeOf(context).height * 0.82;
    // 操作按钮之间统一 8px 间距，避免各页面各写一套。
    final actionRow = <Widget>[];
    for (var index = 0; index < actions.length; index++) {
      if (index > 0) actionRow.add(const SizedBox(width: 8));
      actionRow.add(actions[index]);
    }

    final dialog = Semantics(
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

    // 弹窗打开即把键盘焦点收入内部，Tab 按「取消 → 确认」的视觉顺序遍历；
    // 只有调用方传入 onSubmit 才绑定 Enter（危险操作不传，见类注释）。
    final shortcuts = onSubmit == null
        ? const <ShortcutActivator, VoidCallback>{}
        : <ShortcutActivator, VoidCallback>{
            const SingleActivator(LogicalKeyboardKey.enter): onSubmit!,
            const SingleActivator(LogicalKeyboardKey.numpadEnter): onSubmit!,
          };
    return CallbackShortcuts(
      bindings: shortcuts,
      child: Focus(autofocus: true, child: dialog),
    );
  }
}

/// 二次确认弹窗：返回是否确认。
///
/// [title] 标题；[body] 说明；[confirmLabel] 确认按钮文案；
/// [danger] 确认动作是否破坏性（红底）；[safeDefault] 是否把默认焦点放到「取消」
/// 且不绑定 Enter（用于「恢复默认」这类不希望一路 Enter 通过的确认）。
///
/// 键盘约定（清单 P2-6）：非破坏性弹窗默认聚焦「确认」且 Enter 直接确认；
/// 破坏性或 [safeDefault] 弹窗默认聚焦「取消」且**不绑定 Enter**，
/// 必须显式 Tab/点击确认。
Future<bool> showRdcConfirmDialog(
  BuildContext context, {
  required String title,
  required String body,
  String confirmLabel = '确认',
  String cancelLabel = '取消',
  bool danger = false,
  bool safeDefault = false,
}) async {
  final result = await showDialog<bool>(
    context: context,
    builder: (dialogContext) {
      void confirm() => Navigator.of(dialogContext).pop(true);
      final guardEnter = danger || safeDefault;
      final focusCancel = danger || safeDefault;
      return RdcDialog(
        title: title,
        // 危险/需谨慎的操作不绑定 Enter：避免连按 Enter 误删/误吊销/误重置。
        onSubmit: guardEnter ? null : confirm,
        actions: [
          RdcButton(
            label: cancelLabel,
            autofocus: focusCancel,
            onPressed: () => Navigator.of(dialogContext).pop(false),
          ),
          RdcButton(
            label: confirmLabel,
            primary: true,
            danger: danger,
            autofocus: !focusCancel,
            onPressed: confirm,
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
