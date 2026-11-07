// ============================================================
// 文件用途：第一批 UI 整改的 widget 回归测试
//
// 覆盖点：禁用态配色与语义、空状态文案、弹窗边距与圆角、导航选中语义、
// 分页单页收敛（对应清单 F1 / A3 / G9 / A4 / B6）。
// 数据来源：全部为纯 widget 组件，不触碰 Rust FFI，可在无会话环境运行。
// ============================================================

import 'dart:ui' show Tristate;

import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rdc/core/l10n/strings.dart';
import 'package:rdc/core/theme/tokens.dart';
import 'package:rdc/view/components/fields.dart';
import 'package:rdc/view/components/home_access_card.dart';
import 'package:rdc/view/components/info_row.dart';
import 'package:rdc/view/components/pager.dart';
import 'package:rdc/view/components/rdc_button.dart';
import 'package:rdc/view/components/rdc_dialog.dart';
import 'package:rdc/view/theme/app_theme.dart';

/// 测试宿主：统一使用应用主题，避免默认 M3 配色干扰断言。
Widget _host(Widget child) {
  return MaterialApp(
    theme: AppTheme.dark(),
    home: Scaffold(body: Center(child: child)),
  );
}

void main() {
  // =====** 禁用态：配色弱化 + 语义标注不可用 =====**

  testWidgets('状态按钮禁用时不再沿用强调色，且读屏可识别为不可用', (tester) async {
    final handle = tester.ensureSemantics();
    await tester.pumpWidget(
      _host(
        const RdcStateButton(
          label: '允许中继',
          active: true,
          onPressed: null,
        ),
      ),
    );

    final button = tester.widget<OutlinedButton>(find.byType(OutlinedButton));
    final background = button.style!.backgroundColor!
        .resolve(<WidgetState>{WidgetState.disabled})!;
    final foreground = button.style!.foregroundColor!
        .resolve(<WidgetState>{WidgetState.disabled})!;

    // 禁用态必须透明/弱化，不能仍是强调蓝。
    expect(background.a, lessThan(0.5));
    expect(foreground, isNot(RdcTokens.accent));
    expect(foreground.a, lessThan(1.0));

    // 读屏标签必须包含「不可用」，而不是只报一个按钮名。
    expect(find.bySemanticsLabel(RegExp('不可用')), findsOneWidget);
    handle.dispose();
  });

  // =====** 空状态：说明里必须给出下一步 =====**

  testWidgets('设备空状态说明给出下一步动作', (tester) async {
    await tester.pumpWidget(
      _host(
        const RdcEmptyState(
          icon: Icons.devices_other,
          title: AppStrings.deviceEmptyTitle,
          body: AppStrings.deviceEmptyBody,
        ),
      ),
    );
    expect(find.text(AppStrings.deviceEmptyBody), findsOneWidget);
    expect(
      find.text('还没有设备。点「新增设备」，输入对方的设备 ID。'),
      findsOneWidget,
    );
  });

  testWidgets('远程状态不可读时显示明确空状态与重试，而不是假开关', (tester) async {
    await tester.pumpWidget(
      _host(
        RdcAccessCard(
          enabled: false,
          onlineLabel: '',
          unavailable: true,
          onRetry: () {},
        ),
      ),
    );
    expect(find.text(AppStrings.homeAccessUnknown), findsOneWidget);
    expect(find.text(AppStrings.retry), findsOneWidget);
    // 不可读时不允许出现「已允许/已停止」的猜测文案。
    expect(find.text(AppStrings.homeAccessAllowed), findsNothing);
    expect(find.text(AppStrings.homeAccessStopped), findsNothing);
  });

  // =====** 弹窗：边距 24 / 圆角 16 / 按钮 36 =====**

  testWidgets('弹窗使用 24 边距与 16 圆角，按钮走统一 36 高', (tester) async {
    await tester.pumpWidget(
      MaterialApp(
        theme: AppTheme.dark(),
        home: Builder(
          builder: (context) => Scaffold(
            body: Center(
              child: RdcButton(
                label: '打开',
                onPressed: () => showDialog<void>(
                  context: context,
                  builder: (_) => RdcDialog(
                    title: '标题',
                    actions: [
                      RdcButton(
                        label: '取消',
                        onPressed: () => Navigator.of(context).pop(),
                      ),
                    ],
                    child: const Text('正文'),
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
    );
    await tester.tap(find.text('打开'));
    await tester.pumpAndSettle();

    final dialog = tester.widget<Dialog>(find.byType(Dialog));
    expect(dialog.insetPadding, const EdgeInsets.all(24));
    final shape = dialog.shape! as RoundedRectangleBorder;
    expect(shape.borderRadius, BorderRadius.circular(RdcTokens.radiusCard));

    // 外层 inset 与内容区都必须是 24px（清单 G9：24–28）。
    final padding24 = find.byWidgetPredicate(
      (widget) => widget is Padding &&
          widget.padding == const EdgeInsets.all(RdcTokens.dialogPadding),
    );
    expect(padding24, findsNWidgets(2));

    // 弹窗按钮最小高度 36（清单 G1）。
    final button = tester.widget<OutlinedButton>(
      find.descendant(
        of: find.byType(RdcDialog),
        matching: find.byType(OutlinedButton),
      ),
    );
    final size = button.style!.minimumSize!.resolve(<WidgetState>{})!;
    expect(size.height, RdcTokens.controlHeight);
    expect(RdcTokens.controlHeight, 36);
  });

  // =====** 导航：选中态用 selected 语义，而不是“开关已开” =====**

  testWidgets('导航标签暴露选中语义', (tester) async {
    final handle = tester.ensureSemantics();
    await tester.pumpWidget(
      _host(
        const RdcNavTab(label: '设备', selected: true, onPressed: null),
      ),
    );
    final node = tester.getSemantics(find.byType(RdcNavTab));
    expect(node.flagsCollection.isSelected, Tristate.isTrue);
    expect(node.flagsCollection.isButton, isTrue);
    handle.dispose();
  });

  // =====** 分页：单页只显示总数 =====**

  testWidgets('只有一页时隐藏翻页按钮与 1/1 徽章', (tester) async {
    await tester.pumpWidget(
      _host(
        RdcPager(page: 1, pageCount: 1, total: 3, onChanged: (_) {}),
      ),
    );
    expect(find.text('共 3 台设备'), findsOneWidget);
    expect(find.text('上一页'), findsNothing);
    expect(find.text('下一页'), findsNothing);
    expect(find.text('1 / 1'), findsNothing);
  });

  // =====** 第二批：菜单按钮 / 紧凑胶囊 / 密码框 =====**

  testWidgets('「更多」菜单按钮与普通按钮同高，展开后可选中低频动作', (tester) async {
    String? selected;
    await tester.pumpWidget(
      _host(
        RdcMenuButton(
          label: AppStrings.more,
          onSelected: (value) => selected = value,
          itemBuilder: (context) => const [
            PopupMenuItem(value: 'clipboard', child: Text(AppStrings.clipboard)),
            PopupMenuItem(value: 'files', child: Text(AppStrings.files)),
          ],
        ),
      ),
    );

    final size = tester.getSize(find.byType(RdcMenuButton));
    expect(size.height, RdcTokens.controlHeight);

    await tester.tap(find.text(AppStrings.more));
    await tester.pumpAndSettle();
    expect(find.text(AppStrings.clipboard), findsOneWidget);
    expect(find.text(AppStrings.files), findsOneWidget);

    await tester.tap(find.text(AppStrings.clipboard));
    await tester.pumpAndSettle();
    expect(selected, 'clipboard');
  });

  testWidgets('窄屏紧凑胶囊高度不超过 20px', (tester) async {
    await tester.pumpWidget(
      _host(
        const RdcStatusPill(
          text: AppStrings.wallNotConnected,
          color: RdcTokens.textMuted,
          compact: true,
        ),
      ),
    );
    expect(
      tester.getSize(find.byType(RdcStatusPill)).height,
      lessThanOrEqualTo(20),
    );
  });

  testWidgets('密码框默认遮挡，眼睛按钮可切换明文', (tester) async {
    final controller = TextEditingController(text: 'secret');
    await tester.pumpWidget(
      _host(
        SizedBox(
          width: 240,
          child: RdcTextField(controller: controller, obscure: true),
        ),
      ),
    );

    expect(tester.widget<TextField>(find.byType(TextField)).obscureText, isTrue);
    expect(find.byIcon(Icons.visibility_off), findsOneWidget);

    await tester.tap(find.byIcon(Icons.visibility_off));
    await tester.pumpAndSettle();
    expect(tester.widget<TextField>(find.byType(TextField)).obscureText, isFalse);
    expect(find.byIcon(Icons.visibility), findsOneWidget);
    controller.dispose();
  });

  test('启动与托盘只给空状态说明，不承诺未提供的能力', () {
    expect(AppStrings.startupUnavailable.contains('尚未提供'), isTrue);
    expect(AppStrings.startupUnavailable.contains('已开启'), isFalse);
    expect(AppStrings.checkUpdateUnavailable.contains('不提供在线更新'), isTrue);
    expect(AppStrings.capabilitiesLines, isNotEmpty);
  });

  // =====** 文案：开发痕迹不得出现在面向用户的字符串里 =====**

  test('用户可见文案不含内部实现术语', () {
    final userFacing = <String>[
      AppStrings.appTagline,
      AppStrings.wallNote,
      AppStrings.sessionTexturePending,
      AppStrings.sessionTextureStarting,
      AppStrings.sessionTextureFailed,
      AppStrings.deviceEmptyBody,
      AppStrings.accountNote,
      AppStrings.accountLoginBody,
      AppStrings.trustHint,
      AppStrings.aboutHint,
    ];
    const forbidden = ['Rust', 'Flutter', 'DXGI', '待接线', '未接入', '工程量'];
    for (final text in userFacing) {
      for (final word in forbidden) {
        expect(
          text.contains(word),
          isFalse,
          reason: '文案「$text」仍包含工程词「$word」',
        );
      }
    }
  });
}
