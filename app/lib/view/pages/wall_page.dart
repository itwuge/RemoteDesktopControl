// ============================================================
// 页面用途：屏幕墙页（真实缩略流预览 + 1/2/4/6 宫格切换）
//
// 层级：view/pages
// 被谁组装：shell_page.dart
// 数据来源：WallController（Rust 本机回环缩略流；跨机会话待接线）
// ============================================================

import 'package:flutter/material.dart';

import '../../controller/shell_controller.dart';
import '../../controller/viewport_controller.dart';
import '../../controller/wall_controller.dart';
import '../../core/l10n/strings.dart';
import '../../core/theme/tokens.dart';
import '../components/hover_card.dart';
import '../components/info_row.dart';
import '../components/rdc_button.dart';
import '../components/rgba_preview.dart';

/// 屏幕墙页。
class WallPage extends StatefulWidget {
  /// 创建屏幕墙页。
  ///
  /// [controller] 预览状态；[shell] 点击宫格进入控制。
  const WallPage({
    super.key,
    required this.controller,
    required this.shell,
  });

  /// 墙控制器。
  final WallController controller;

  /// 壳控制器。
  final ShellController shell;

  @override
  State<WallPage> createState() => _WallPageState();
}

/// 屏幕墙页状态。
class _WallPageState extends State<WallPage> {
  @override
  void initState() {
    super.initState();
    widget.controller.start();
  }

  @override
  void dispose() {
    // 离开页面即停流，避免后台持续占用编码资源。
    widget.controller.stop();
    super.dispose();
  }

  // =====** 组件：页头（宫格切换 + 实测状态） =====**

  /// 宫格切换按钮组。
  Widget _buildGridButtons() {
    return ValueListenableBuilder(
      valueListenable: widget.controller.gridIndex,
      builder: (context, selected, _) {
        return Wrap(
          spacing: RdcTokens.spaceSm,
          runSpacing: RdcTokens.spaceSm,
          children: [
            for (var index = 0;
                index < WallController.gridOptions.length;
                index++)
              // 宫格按钮只保留文字：同一个图标重复 4 次会变成视觉噪音（清单 G7 / P1-7）。
              RdcStateButton(
                label: '${WallController.gridOptions[index]} 宫格',
                active: selected == index,
                onPressed: () => widget.controller.setGrid(index),
              ),
          ],
        );
      },
    );
  }

  /// 实测状态胶囊组（fps + 延迟）。
  Widget _buildStatusPills() {
    return ValueListenableBuilder(
      valueListenable: widget.controller.status,
      builder: (context, status, _) {
        if (status == null || !status.running) {
          // 「未启动」不是故障，用中性色，避免首屏一片红（清单 G6）。
          return const RdcStatusPill(
            text: '未启动',
            color: RdcTokens.textMuted,
          );
        }
        return Wrap(
          spacing: RdcTokens.spaceSm,
          runSpacing: RdcTokens.spaceSm,
          children: [
            RdcStatusPill(
              text: '${status.fps.toStringAsFixed(1)} fps',
              color: RdcTokens.success,
            ),
            RdcStatusPill(
              text: '${(status.latencyUs / 1000).round()} ms',
              color: RdcTokens.accent,
            ),
          ],
        );
      },
    );
  }

  /// 页头：宽布局一行、窄布局分行（不是整页缩放）。
  Widget _buildHeader(bool wide) {
    const label = Text(
      AppStrings.wallGridLabel,
      style: TextStyle(
        fontSize: RdcTokens.fontCaption,
        color: RdcTokens.textMuted,
      ),
    );
    if (wide) {
      return Row(
        children: [
          label,
          const SizedBox(width: RdcTokens.spaceSm),
          Expanded(child: _buildGridButtons()),
          const SizedBox(width: RdcTokens.spaceSm),
          _buildStatusPills(),
        ],
      );
    }
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        label,
        const SizedBox(height: RdcTokens.spaceSm),
        _buildGridButtons(),
        const SizedBox(height: RdcTokens.spaceSm),
        _buildStatusPills(),
      ],
    );
  }

  // =====** 组件：单格 =====**

  Widget _buildTile(int index, int columns, {required String label, Widget? preview, bool live = false, String? hint}) {
    return RdcHoverCard(
      padding: EdgeInsets.zero,
      // 未连接的宫格不绑定点击：不弹「待接线」，不用假入口（清单 C6）。
      onTap: live ? () => widget.shell.openControl(deviceName: label) : null,
      child: ClipRRect(
        borderRadius: BorderRadius.circular(RdcTokens.radiusCard),
        child: Stack(
          fit: StackFit.expand,
          children: [
            if (preview != null)
              preview
            else
              Container(
                color: RdcTokens.surfaceRaised,
                alignment: Alignment.center,
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    const Icon(
                      Icons.monitor,
                      size: 28,
                      color: RdcTokens.textMuted,
                    ),
                    const SizedBox(height: RdcTokens.spaceSm),
                    Text(
                      hint ?? AppStrings.wallWaiting,
                      style: const TextStyle(
                        fontSize: RdcTokens.fontCaption,
                        color: RdcTokens.textMuted,
                      ),
                    ),
                  ],
                ),
              ),
            // 底部标签条：设备名 + 实时状态
            Positioned(
              left: 0,
              right: 0,
              bottom: 0,
              child: Container(
                padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
                color: RdcTokens.background.withValues(alpha: 0.72),
                child: Row(
                  children: [
                    Expanded(
                      child: Text(
                        label,
                        overflow: TextOverflow.ellipsis,
                        style: const TextStyle(
                          fontSize: RdcTokens.fontCaption,
                          fontWeight: FontWeight.w600,
                        ),
                      ),
                    ),
                    RdcStatusPill(
                      text: live ? '在线' : AppStrings.wallNotConnected,
                      color: live ? RdcTokens.success : RdcTokens.textMuted,
                    ),
                  ],
                ),
              ),
            ),
          ],
        ),
      ),
    );
  }

  // =====** 组件：网格 =====**

  /// 宫格列数：随方向变化（窄屏单列/两列，宽屏两列/三列），不是把整页缩小。
  int _columnsFor(int count, bool wide) {
    if (count == 1) return 1;
    if (count == 2) return wide ? 2 : 1;
    if (count == 6) return wide ? 3 : 2;
    return 2;
  }

  Widget _buildGrid(bool wide) {
    return ValueListenableBuilder(
      valueListenable: widget.controller.gridIndex,
      builder: (context, gridIndex, _) {
        final count = WallController.gridOptions[gridIndex];
        final columns = _columnsFor(count, wide);
        // 窄屏卡片更高（4:3），避免缩略图被压成一条线。
        final aspect = wide ? 16 / 10 : 4 / 3;
        return ValueListenableBuilder(
          valueListenable: widget.controller.deviceNames,
          builder: (context, names, _) {
            return GridView.builder(
              shrinkWrap: true,
              physics: const NeverScrollableScrollPhysics(),
              itemCount: count,
              gridDelegate: SliverGridDelegateWithFixedCrossAxisCount(
                crossAxisCount: columns,
                mainAxisSpacing: RdcTokens.spaceMd,
                crossAxisSpacing: RdcTokens.spaceMd,
                childAspectRatio: aspect,
              ),
              itemBuilder: (context, index) {
                if (index == 0) {
                  return ValueListenableBuilder(
                    valueListenable: widget.controller.latest,
                    builder: (context, frame, _) {
                      if (frame == null) {
                        return _buildTile(
                          index,
                          columns,
                          label: AppStrings.wallLocalPreview,
                          hint: '正在准备预览…',
                        );
                      }
                      return _buildTile(
                        index,
                        columns,
                        label: AppStrings.wallLocalPreview,
                        live: true,
                        preview: RdcRgbaPreview(
                          bytes: frame.bytes,
                          width: frame.width,
                          height: frame.height,
                        ),
                      );
                    },
                  );
                }
                final name = index - 1 < names.length ? names[index - 1] : null;
                return _buildTile(
                  index,
                  columns,
                  label: name ?? AppStrings.wallWaiting,
                  hint: name == null ? AppStrings.wallWaiting : AppStrings.wallNotConnected,
                );
              },
            );
          },
        );
      },
    );
  }

  // =====** 组件：页面骨架 =====**

  @override
  Widget build(BuildContext context) {
    // 方向只决定布局（列数/卡片比例/页头分行），不缩放置换整页。
    return LayoutBuilder(
      builder: (context, constraints) {
        final wide = ViewportController.resolveWide(constraints.biggest);
        return SingleChildScrollView(
          padding: const EdgeInsets.all(RdcTokens.spacePage),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              _buildHeader(wide),
              const SizedBox(height: RdcTokens.spaceMd),
              const Text(
                AppStrings.wallNote,
                style: TextStyle(
                  fontSize: RdcTokens.fontCaption,
                  color: RdcTokens.textMuted,
                ),
              ),
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
              _buildGrid(wide),
            ],
          ),
        );
      },
    );
  }
}
