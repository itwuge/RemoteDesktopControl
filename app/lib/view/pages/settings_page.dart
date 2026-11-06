// ============================================================
// 页面用途：设置页（中继 / 方向 / 文件传输）
//
// 层级：view/pages
// 被谁组装：home_page.dart 通过 Navigator 打开
// 数据来源：SettingsController（草稿设置、保存结果）
// ============================================================

import 'package:flutter/material.dart';

import '../../controller/settings_controller.dart';
import '../../core/l10n/strings.dart';
import '../../core/theme/tokens.dart';
import '../../core/utils/formatters.dart';
import '../components/info_row.dart';
import '../components/section_card.dart';

/// 设置页。
class SettingsPage extends StatefulWidget {
  /// 创建设置页。
  ///
  /// [controller] 为设置状态与保存行为。
  const SettingsPage({super.key, required this.controller});

  /// 设置控制器。
  final SettingsController controller;

  @override
  State<SettingsPage> createState() => _SettingsPageState();
}

/// 设置页状态：持有输入框控制器与保存反馈。
class _SettingsPageState extends State<SettingsPage> {
  /// 中继地址输入框控制器；初值在 initState 中从草稿设置填充。
  late final TextEditingController _relayAddressController;

  @override
  void initState() {
    super.initState();
    _relayAddressController = TextEditingController(
      text: widget.controller.settings.value.relayAddress,
    );
  }

  @override
  void dispose() {
    // 先移除监听再释放，避免析构顺序导致 setState after dispose。
    _relayAddressController.dispose();
    super.dispose();
  }

  // =====** 组件：页面骨架 =====**

  @override
  Widget build(BuildContext context) {
    return Scaffold(
      appBar: AppBar(title: const Text(AppStrings.settingsTitle)),
      body: SafeArea(
        child: ValueListenableBuilder(
          valueListenable: widget.controller.settings,
          builder: (context, settings, _) {
            return SingleChildScrollView(
              padding: const EdgeInsets.all(RdcTokens.spacePage),
              child: Center(
                child: ConstrainedBox(
                  constraints: const BoxConstraints(maxWidth: 920),
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.start,
                    children: [
                      _buildRelaySection(settings.allowRelay),
                      const SizedBox(height: RdcTokens.spaceXl),
                      _buildDisplaySection(),
                      const SizedBox(height: RdcTokens.spaceXl),
                      _buildTransferSection(settings.fileBandwidthRatio),
                      const SizedBox(height: RdcTokens.spaceXl),
                      _buildSaveButton(),
                    ],
                  ),
                ),
              ),
            );
          },
        ),
      ),
    );
  }

  // =====** 组件：中继分组 =====**

  Widget _buildRelaySection(bool allowRelay) {
    return RdcSectionCard(
      title: AppStrings.relaySection,
      trailing: RdcStatusPill(
        text: allowRelay ? '已开启' : '严格 P2P',
        color: allowRelay ? RdcTokens.danger : RdcTokens.success,
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              const Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Text(
                      AppStrings.relayEnable,
                      style: TextStyle(fontSize: RdcTokens.fontBody, color: RdcTokens.textPrimary),
                    ),
                    SizedBox(height: 2),
                    Text(
                      AppStrings.relayHint,
                      style: TextStyle(fontSize: RdcTokens.fontCaption, color: RdcTokens.textMuted),
                    ),
                  ],
                ),
              ),
              Switch(
                value: allowRelay,
                onChanged: (value) => _onRelayChanged(value),
              ),
            ],
          ),
          if (allowRelay) ...[
            const SizedBox(height: RdcTokens.spaceMd),
            TextField(
              controller: _relayAddressController,
              onChanged: widget.controller.setRelayAddress,
              decoration: InputDecoration(
                labelText: AppStrings.relayAddress,
                hintText: 'relay.example.com:21117',
                filled: true,
                fillColor: RdcTokens.surfaceRaised,
                border: OutlineInputBorder(
                  borderRadius: BorderRadius.circular(RdcTokens.radiusControl),
                  borderSide: const BorderSide(color: RdcTokens.border),
                ),
                enabledBorder: OutlineInputBorder(
                  borderRadius: BorderRadius.circular(RdcTokens.radiusControl),
                  borderSide: const BorderSide(color: RdcTokens.border),
                ),
              ),
            ),
            const SizedBox(height: RdcTokens.spaceMd),
            const Text(
              AppStrings.policyLabel,
              style: TextStyle(fontSize: RdcTokens.fontBody, color: RdcTokens.textPrimary),
            ),
            const SizedBox(height: RdcTokens.spaceSm),
            SegmentedButton<int>(
              segments: const [
                ButtonSegment(value: 0, label: Text(AppStrings.policyStrict)),
                ButtonSegment(value: 1, label: Text(AppStrings.policyAsk)),
                ButtonSegment(value: 2, label: Text(AppStrings.policyAuto)),
              ],
              selected: {widget.controller.policyIndex},
              onSelectionChanged: (selection) =>
                  widget.controller.setConnectionPolicyByIndex(selection.first),
            ),
          ],
        ],
      ),
    );
  }

  // =====** 组件：显示与方向分组 =====**

  Widget _buildDisplaySection() {
    return RdcSectionCard(
      title: AppStrings.displaySection,
      child: DropdownButtonFormField<int>(
        initialValue: widget.controller.orientationIndex,
        decoration: InputDecoration(
          labelText: AppStrings.orientationLabel,
          filled: true,
          fillColor: RdcTokens.surfaceRaised,
          border: OutlineInputBorder(
            borderRadius: BorderRadius.circular(RdcTokens.radiusControl),
            borderSide: const BorderSide(color: RdcTokens.border),
          ),
        ),
        items: const [
          DropdownMenuItem(value: 0, child: Text(AppStrings.orientationFollow)),
          DropdownMenuItem(value: 1, child: Text(AppStrings.orientationLandscape)),
          DropdownMenuItem(value: 2, child: Text(AppStrings.orientationPortrait)),
        ],
        onChanged: (value) {
          if (value != null) widget.controller.setOrientationByIndex(value);
        },
      ),
    );
  }

  // =====** 组件：文件传输分组 =====**

  Widget _buildTransferSection(double ratio) {
    return RdcSectionCard(
      title: AppStrings.transferSection,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              const Expanded(
                child: Text(
                  AppStrings.bandwidthLabel,
                  style: TextStyle(fontSize: RdcTokens.fontBody, color: RdcTokens.textPrimary),
                ),
              ),
              Text(
                Formatters.percent(ratio),
                style: const TextStyle(
                  fontSize: RdcTokens.fontBody,
                  color: RdcTokens.accent,
                  fontWeight: FontWeight.w600,
                ),
              ),
            ],
          ),
          Slider(
            value: ratio,
            min: 0.1,
            max: 0.7,
            divisions: 6,
            label: Formatters.percent(ratio),
            onChanged: widget.controller.setFileBandwidthRatio,
          ),
          const Text(
            AppStrings.bandwidthHint,
            style: TextStyle(fontSize: RdcTokens.fontCaption, color: RdcTokens.textMuted),
          ),
        ],
      ),
    );
  }

  // =====** 组件：保存按钮 =====**

  Widget _buildSaveButton() {
    return ValueListenableBuilder(
      valueListenable: widget.controller.isSaving,
      builder: (context, saving, _) {
        return SizedBox(
          width: double.infinity,
          child: FilledButton(
            onPressed: saving ? null : _save,
            child: Text(saving ? '保存中…' : AppStrings.saveButton),
          ),
        );
      },
    );
  }

  // =====** 行为：中继开关确认 =====**

  Future<void> _onRelayChanged(bool value) async {
    if (!value) {
      widget.controller.setAllowRelay(false);
      return;
    }
    // 开启中继属于高风险设置：必须显式确认，避免用户误触后流量走服务器。
    final confirmed = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => AlertDialog(
        title: const Text(AppStrings.relayConfirmTitle),
        content: const Text(AppStrings.relayConfirmBody),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(dialogContext).pop(false),
            child: const Text(AppStrings.cancel),
          ),
          FilledButton(
            onPressed: () => Navigator.of(dialogContext).pop(true),
            child: const Text(AppStrings.confirm),
          ),
        ],
      ),
    );
    if (confirmed == true) {
      widget.controller.setAllowRelay(true);
    }
  }

  // =====** 行为：保存 =====**

  Future<void> _save() async {
    await widget.controller.save();
    if (!mounted) return;
    final message = widget.controller.message.value;
    if (message == null) return;
    ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(message)));
  }
}
