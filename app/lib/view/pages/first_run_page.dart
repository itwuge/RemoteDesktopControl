// ============================================================
// 页面用途：首启「允许被连 / 不允许」引导（清单 P1-17）
// 层级：view（只负责呈现与调用控制器）
// 被谁调用：main.dart 的启动门；完成后进入主界面
// 边界：不直接读写文件；不触碰协议、capabilities 与握手流程
// ============================================================

import 'package:flutter/material.dart';

import '../../controller/settings_controller.dart';
import '../../core/l10n/strings.dart';
import '../../core/theme/tokens.dart';
import '../components/rdc_button.dart';

/// 首启页：默认「暂不允许」，选择后本机设置落盘并进入主界面。
class FirstRunPage extends StatefulWidget {
  /// 创建首启页。
  ///
  /// [controller] 设置控制器（负责本地持久化）；[onDone] 完成后回调进入主界面。
  const FirstRunPage({
    super.key,
    required this.controller,
    required this.onDone,
  });

  /// 设置控制器。
  final SettingsController controller;

  /// 完成回调。
  final VoidCallback onDone;

  @override
  State<FirstRunPage> createState() => _FirstRunPageState();
}

/// 首启页状态：保存中与错误提示。
class _FirstRunPageState extends State<FirstRunPage> {
  /// 是否正在保存（防止连点写出两条记录）。
  bool _saving = false;

  /// 保存失败提示；`null` 表示无错误。
  String? _error;

  /// 保存用户选择并结束引导。
  Future<void> _choose({required bool allowRemoteAccess}) async {
    if (_saving) return;
    setState(() {
      _saving = true;
      _error = null;
    });
    final ok =
        await widget.controller.completeFirstRun(allowRemoteAccess: allowRemoteAccess);
    if (!mounted) return;
    if (ok) {
      widget.onDone();
      return;
    }
    setState(() {
      _saving = false;
      _error = AppStrings.firstRunSaveFailed;
    });
  }

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      backgroundColor: RdcTokens.background,
      body: Center(
        child: SingleChildScrollView(
          padding: const EdgeInsets.all(RdcTokens.spacePage),
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 640),
            child: Semantics(
              scopesRoute: true,
              namesRoute: true,
              label: AppStrings.firstRunTitle,
              explicitChildNodes: true,
              child: Container(
                padding: const EdgeInsets.all(28),
                decoration: BoxDecoration(
                  color: RdcTokens.surface,
                  borderRadius: BorderRadius.circular(RdcTokens.radiusCard),
                  border: Border.all(color: RdcTokens.border),
                ),
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    // 每区一个图标：这里只有安全语义，不放装饰性图标。
                    const Icon(
                      Icons.shield_outlined,
                      size: 24,
                      color: RdcTokens.accent,
                    ),
                    const SizedBox(height: RdcTokens.spaceMd),
                    const Text(
                      AppStrings.firstRunTitle,
                      style: TextStyle(
                        fontSize: RdcTokens.fontPageTitle,
                        fontWeight: FontWeight.w600,
                        color: RdcTokens.textPrimary,
                      ),
                    ),
                    const SizedBox(height: RdcTokens.spaceSm),
                    const Text(
                      AppStrings.firstRunBody,
                      style: TextStyle(
                        fontSize: RdcTokens.fontBody,
                        color: RdcTokens.textMuted,
                        height: 1.6,
                      ),
                    ),
                    const SizedBox(height: RdcTokens.spaceLg),
                    Wrap(
                      spacing: RdcTokens.spaceSm,
                      runSpacing: RdcTokens.spaceSm,
                      children: [
                        RdcButton(
                          label: AppStrings.firstRunAllow,
                          icon: Icons.check,
                          primary: true,
                          onPressed: _saving
                              ? null
                              : () => _choose(allowRemoteAccess: true),
                        ),
                        RdcButton(
                          label: AppStrings.firstRunDeny,
                          icon: Icons.block,
                          onPressed: _saving
                              ? null
                              : () => _choose(allowRemoteAccess: false),
                        ),
                      ],
                    ),
                    const SizedBox(height: RdcTokens.spaceMd),
                    const Text(
                      AppStrings.firstRunDefaultNote,
                      style: TextStyle(
                        fontSize: RdcTokens.fontCaption,
                        color: RdcTokens.textMuted,
                        height: 1.5,
                      ),
                    ),
                    if (_error != null) ...[
                      const SizedBox(height: RdcTokens.spaceSm),
                      Text(
                        _error!,
                        style: const TextStyle(
                          fontSize: RdcTokens.fontCaption,
                          color: RdcTokens.danger,
                        ),
                      ),
                    ],
                    if (_saving) ...[
                      const SizedBox(height: RdcTokens.spaceSm),
                      const Text(
                        '保存中…',
                        style: TextStyle(
                          fontSize: RdcTokens.fontCaption,
                          color: RdcTokens.textMuted,
                        ),
                      ),
                    ],
                  ],
                ),
              ),
            ),
          ),
        ),
      ),
    );
  }
}
