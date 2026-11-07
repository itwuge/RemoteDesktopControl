// ============================================================
// 页面用途：设备列表页（卡片列表 + 行内操作 + 空状态 + 搜索）
//
// 层级：view/pages
// 被谁组装：shell_page.dart
// 数据来源：DeviceController（Rust 设备列表文件，真实数据）
// 布局规矩：搜索区一排；操作按钮在条目后；不跳新页面（编辑用弹窗）
// ============================================================

import 'package:flutter/material.dart';

import '../../controller/device_controller.dart';
import '../../controller/shell_controller.dart';
import '../../controller/viewport_controller.dart';
import '../../core/l10n/strings.dart';
import '../../core/theme/tokens.dart';
import '../../core/utils/formatters.dart';
import '../components/fields.dart';
import '../components/hover_card.dart';
import '../components/info_row.dart';
import '../components/pager.dart';
import '../components/rdc_button.dart';
import '../components/rdc_dialog.dart';

/// 设备列表页。
class DevicePage extends StatefulWidget {
  /// 创建设备页。
  ///
  /// [controller] 设备状态；[shell] 用于「连接」跳转远程控制。
  const DevicePage({
    super.key,
    required this.controller,
    required this.shell,
  });

  /// 设备控制器。
  final DeviceController controller;

  /// 壳控制器。
  final ShellController shell;

  @override
  State<DevicePage> createState() => _DevicePageState();
}

/// 设备页状态。
class _DevicePageState extends State<DevicePage> {
  /// 搜索输入框。
  late final TextEditingController _search = TextEditingController();

  @override
  void initState() {
    super.initState();
    if (widget.controller.allDevices.value.isEmpty) {
      widget.controller.load();
    }
  }

  @override
  void dispose() {
    _search.dispose();
    super.dispose();
  }

  // =====** 组件：搜索区（一排 / 窄屏分行） =====**

  /// 筛选与新增按钮组。
  Widget _buildFilterButtons() {
    return ValueListenableBuilder(
      valueListenable: widget.controller.filterIndex,
      builder: (context, filter, _) {
        return Wrap(
          spacing: RdcTokens.spaceSm,
          runSpacing: RdcTokens.spaceSm,
          crossAxisAlignment: WrapCrossAlignment.center,
          children: [
            RdcStateButton(
              label: AppStrings.filterAll,
              active: filter == 0,
              icon: Icons.inbox,
              onPressed: () => widget.controller.setFilter(0),
            ),
            RdcStateButton(
              label: AppStrings.filterTrusted,
              active: filter == 1,
              icon: Icons.verified_user,
              onPressed: () => widget.controller.setFilter(1),
            ),
            RdcButton(
              label: AppStrings.deviceAdd,
              icon: Icons.add,
              primary: true,
              onPressed: () => _showDeviceDialog(),
            ),
          ],
        );
      },
    );
  }

  /// 搜索字段。
  Widget _buildSearchField({required bool expand}) {
    return RdcField(
      label: AppStrings.deviceSearchLabel,
      expand: expand,
      child: RdcTextField(
        controller: _search,
        hint: AppStrings.deviceSearchHint,
        onChanged: widget.controller.setQuery,
      ),
    );
  }

  Widget _buildSearchRow(bool wide) {
    if (wide) {
      // 底部对齐：标签在上时，输入框与按钮同一条基线（清单 B2 / E2）。
      return Row(
        crossAxisAlignment: CrossAxisAlignment.end,
        children: [
          _buildSearchField(expand: true),
          const SizedBox(width: RdcTokens.spaceMd),
          _buildFilterButtons(),
        ],
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        _buildSearchField(expand: false),
        const SizedBox(height: RdcTokens.spaceMd),
        _buildFilterButtons(),
      ],
    );
  }

  // =====** 组件：设备卡片（宽屏一行 / 窄屏上下两段） =====**

  Widget _buildDeviceCard(DeviceRow device, bool wide) {
    final trusted = device.trusted;
    final iconBox = Container(
      width: 40,
      height: 40,
      decoration: BoxDecoration(
        color: RdcTokens.surfaceRaised,
        borderRadius: BorderRadius.circular(RdcTokens.radiusControl),
      ),
      child: Icon(
        Icons.desktop_windows,
        size: 20,
        color: trusted ? RdcTokens.accent : RdcTokens.textMuted,
      ),
    );
    final nameColumn = Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Row(
          children: [
            Flexible(
              child: Text(
                device.name,
                overflow: TextOverflow.ellipsis,
                style: const TextStyle(
                  fontSize: RdcTokens.fontBody,
                  fontWeight: FontWeight.w600,
                  color: RdcTokens.textPrimary,
                ),
              ),
            ),
            // 信任不再是独立胶囊：用小锁放在名称行，避免与在线状态抢位置（清单 P1-5）。
            if (trusted) ...[
              const SizedBox(width: 6),
              const Tooltip(
                message: AppStrings.trustedTooltip,
                child: Icon(
                  Icons.lock,
                  size: 14,
                  color: RdcTokens.success,
                ),
              ),
            ],
          ],
        ),
        const SizedBox(height: 2),
        Text(
          device.id,
          overflow: TextOverflow.ellipsis,
          style: const TextStyle(
            fontSize: RdcTokens.fontCaption,
            color: RdcTokens.textMuted,
          ),
        ),
      ],
    );
    // 没有在线心跳接口：只如实显示「从未在线 / 上次在线」，不伪造在线绿点。
    final lastOnline = Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        Icon(
          device.lastOnline == null ? Icons.radio_button_unchecked : Icons.history,
          size: 12,
          color: RdcTokens.textMuted,
        ),
        const SizedBox(width: 4),
        Flexible(
          child: Text(
            device.lastOnline == null
                ? AppStrings.neverOnline
                : Formatters.lastOnline(device.lastOnline),
            overflow: TextOverflow.ellipsis,
            style: const TextStyle(
              fontSize: RdcTokens.fontCaption,
              color: RdcTokens.textMuted,
            ),
          ),
        ),
      ],
    );
    final connectButton = RdcButton(
      label: AppStrings.connect,
      icon: Icons.link,
      primary: true,
      onPressed: () => widget.shell.openControl(
        deviceId: device.id,
        deviceName: device.name,
      ),
    );
    final editButton = RdcButton(
      label: AppStrings.edit,
      icon: Icons.edit,
      onPressed: () => _showDeviceDialog(
        deviceId: device.id,
        name: device.name,
        trusted: trusted,
        sameAccount: device.sameAccount,
      ),
    );
    // 破坏性动作使用 danger 变体，避免与「连接」同样的视觉权重（清单 B5）。
    final deleteButton = RdcButton(
      label: AppStrings.delete,
      icon: Icons.delete_outline,
      danger: true,
      onPressed: () => _confirmDelete(device.id, device.name),
    );
    return Padding(
      padding: const EdgeInsets.only(bottom: RdcTokens.spaceMd),
      child: RdcHoverCard(
        // 整卡可点：点卡片任意空白处进入编辑，按钮仍走各自的回调（清单 P2-8）。
        onTap: () => _showDeviceDialog(
          deviceId: device.id,
          name: device.name,
          trusted: trusted,
          sameAccount: device.sameAccount,
        ),
        padding: EdgeInsets.symmetric(
          horizontal: wide ? 18 : 14,
          vertical: wide ? 14 : RdcTokens.spaceMd,
        ),
        child: wide
            ? Row(
                children: [
                  iconBox,
                  const SizedBox(width: 14),
                  Expanded(flex: 3, child: nameColumn),
                  Expanded(flex: 2, child: lastOnline),
                  const SizedBox(width: RdcTokens.spaceLg),
                  connectButton,
                  const SizedBox(width: RdcTokens.spaceSm),
                  editButton,
                  const SizedBox(width: RdcTokens.spaceSm),
                  deleteButton,
                ],
              )
            : Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Row(
                    children: [
                      iconBox,
                      const SizedBox(width: RdcTokens.spaceMd),
                      Expanded(child: nameColumn),
                    ],
                  ),
                  const SizedBox(height: RdcTokens.spaceSm),
                  lastOnline,
                  const SizedBox(height: RdcTokens.spaceMd),
                  Wrap(
                    spacing: RdcTokens.spaceSm,
                    runSpacing: RdcTokens.spaceSm,
                    children: [connectButton, editButton, deleteButton],
                  ),
                ],
              ),
      ),
    );
  }

  // =====** 组件：空状态 =====**

  Widget _buildEmptyState() {
    // 复用统一空状态组件：图标 + 标题 + 下一步动作（清单 B3 / G-high）。
    return RdcHoverCard(
      child: RdcEmptyState(
        icon: Icons.devices_other,
        title: AppStrings.deviceEmptyTitle,
        body: AppStrings.deviceEmptyBody,
        action: RdcButton(
          label: AppStrings.deviceAdd,
          icon: Icons.add,
          primary: true,
          onPressed: () => _showDeviceDialog(),
        ),
      ),
    );
  }

  // =====** 行为：新增/编辑弹窗 =====**

  Future<void> _showDeviceDialog({
    String? deviceId,
    String? name,
    bool trusted = false,
    bool sameAccount = false,
  }) async {
    final editing = deviceId != null;
    final idController = TextEditingController(text: deviceId ?? '');
    final nameController = TextEditingController(text: name ?? '');
    var trust = trusted;
    var same = sameAccount;
    // 弹窗统一走 RdcDialog：24 边距、16 圆角、标签在上、按钮 36（清单 G9 / P0-14）。
    final result = await showDialog<bool>(
      context: context,
      builder: (dialogContext) => StatefulBuilder(
        builder: (context, setDialogState) => RdcDialog(
          title: editing ? AppStrings.edit : AppStrings.deviceAdd,
          actions: [
            RdcButton(
              label: AppStrings.cancel,
              onPressed: () => Navigator.of(dialogContext).pop(false),
            ),
            RdcButton(
              label: AppStrings.save,
              primary: true,
              onPressed: () => Navigator.of(dialogContext).pop(true),
            ),
          ],
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              RdcField(
                label: AppStrings.deviceIdLabel,
                expand: false,
                child: RdcTextField(
                  controller: idController,
                  hint: 'rdc-xxxx',
                  onChanged: editing ? null : (_) {},
                ),
              ),
              const SizedBox(height: RdcTokens.spaceLg),
              RdcField(
                label: AppStrings.deviceNameLabel,
                expand: false,
                child: RdcTextField(
                  controller: nameController,
                  hint: '书房的台式机',
                ),
              ),
              const SizedBox(height: RdcTokens.spaceLg),
              Wrap(
                spacing: RdcTokens.spaceMd,
                runSpacing: RdcTokens.spaceSm,
                children: [
                  RdcStateButton(
                    label: AppStrings.trustDeviceLabel,
                    active: trust,
                    icon: Icons.verified_user,
                    onPressed: () {
                      if (!editing) {
                        setDialogState(() => trust = !trust);
                      }
                    },
                  ),
                  RdcStateButton(
                    label: AppStrings.sameAccountLabel,
                    active: same,
                    icon: Icons.person,
                    onPressed: () {
                      if (!editing) {
                        setDialogState(() => same = !same);
                      }
                    },
                  ),
                ],
              ),
            ],
          ),
        ),
      ),
    );
    final id = idController.text;
    final deviceName = nameController.text;
    idController.dispose();
    nameController.dispose();
    if (result != true || !mounted) return;
    final message = editing
        ? await widget.controller.updateDevice(
            deviceId: id,
            name: deviceName,
            trusted: trust,
            sameAccount: same,
          )
        : await widget.controller.addDevice(
            deviceId: id,
            name: deviceName,
            trusted: trust,
          );
    if (!mounted) return;
    if (message != null) {
      ScaffoldMessenger.of(context).showSnackBar(SnackBar(content: Text(message)));
    }
  }

  // =====** 行为：删除确认 =====**

  Future<void> _confirmDelete(String deviceId, String name) async {
    final confirmed = await showRdcConfirmDialog(
      context,
      title: AppStrings.deleteConfirmTitle,
      body: '将移除「$name」的本机记录，不影响对方设备。',
      confirmLabel: AppStrings.delete,
      danger: true,
    );
    if (!confirmed || !mounted) return;
    final message = await widget.controller.removeDevice(deviceId);
    if (!mounted) return;
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(content: Text(message ?? '设备已删除')),
    );
  }

  // =====** 组件：页面骨架 =====**

  @override
  Widget build(BuildContext context) {
    // 方向只决定布局形态（搜索区/卡片分行），不缩放内容。
    return LayoutBuilder(
      builder: (context, constraints) {
        final wide = ViewportController.resolveWide(constraints.biggest);
        return SingleChildScrollView(
          padding: const EdgeInsets.all(RdcTokens.spacePage),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              _buildSearchRow(wide),
              const SizedBox(height: RdcTokens.spaceLg),
              ValueListenableBuilder(
                valueListenable: widget.controller.errorMessage,
                builder: (context, error, _) {
                  if (error == null) return const SizedBox.shrink();
                  return Padding(
                    padding: const EdgeInsets.only(bottom: RdcTokens.spaceMd),
                    child: RdcStatusPill(text: error, color: RdcTokens.danger),
                  );
                },
              ),
              ValueListenableBuilder(
                valueListenable: widget.controller.visibleDevices,
                builder: (context, devices, _) {
                  if (devices.isEmpty) {
                    final hasAny = widget.controller.allDevices.value.isNotEmpty;
                    if (hasAny) {
                      return const Padding(
                        padding: EdgeInsets.symmetric(
                          vertical: RdcTokens.spaceXl,
                        ),
                        child: Center(
                          child: Text(
                            '没有匹配的设备',
                            style: TextStyle(color: RdcTokens.textMuted),
                          ),
                        ),
                      );
                    }
                    return _buildEmptyState();
                  }
                  return Column(
                    children: [
                      for (final device in devices)
                        _buildDeviceCard(device, wide),
                      const SizedBox(height: RdcTokens.spaceSm),
                      ValueListenableBuilder(
                        valueListenable: widget.controller.page,
                        builder: (context, page, _) => ValueListenableBuilder(
                          valueListenable: widget.controller.pageCount,
                          builder: (context, pageCount, _) => RdcPager(
                            page: page,
                            pageCount: pageCount,
                            total: widget.controller.allDevices.value.length,
                            onChanged: widget.controller.goToPage,
                          ),
                        ),
                      ),
                    ],
                  );
                },
              ),
            ],
          ),
        );
      },
    );
  }
}
