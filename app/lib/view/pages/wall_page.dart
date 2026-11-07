// ============================================================
// 页面用途：屏幕墙页（真实缩略流预览 + 1/2/4/6 宫格切换）
//
// 层级：view/pages
// 被谁组装：shell_page.dart
// 数据来源：WallController（Rust 本机回环缩略流；跨机会话待接线）
// ============================================================

import 'package:flutter/material.dart';

import '../../controller/shell_controller.dart';
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

  Widget _buildHeader() {
    return Row(
      children: [
        const Text(
          AppStrings.wallGridLabel,
          style: TextStyle(
            fontSize: RdcTokens.fontCaption,
            color: RdcTokens.textMuted,
          ),
        ),
        const SizedBox(width: RdcTokens.spaceSm),
        ValueListenableBuilder(
          valueListenable: widget.controller.gridIndex,
          builder: (context, selected, _) {
            return Row(
              children: [
                for (var index = 0;
                    index < WallController.gridOptions.length;
                    index++) ...[
                  if (index != 0) const SizedBox(width: RdcTokens.spaceSm),
                  RdcStateButton(
                    label: '${WallController.gridOptions[index]} 宫格',
                    icon: Icons.grid_view,
                    active: selected == index,
                    onPressed: () => widget.controller.setGrid(index),
                  ),
                ],
              ],
            );
          },
        ),
        const Spacer(),
        ValueListenableBuilder(
          valueListenable: widget.controller.status,
          builder: (context, status, _) {
            if (status == null || !status.running) {
              return const RdcStatusPill(
                text: '未启动',
                color: RdcTokens.danger,
              );
            }
            return Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                RdcStatusPill(
                  text: '${status.fps.toStringAsFixed(1)} fps',
                  color: RdcTokens.success,
                ),
                const SizedBox(width: RdcTokens.spaceSm),
                RdcStatusPill(
                  text: '${(status.latencyUs / 1000).round()} ms',
                  color: RdcTokens.accent,
                ),
              ],
            );
          },
        ),
      ],
    );
  }

  // =====** 组件：单格 =====**

  Widget _buildTile(int index, int columns, {required String label, Widget? preview, bool live = false, String? hint}) {
    return RdcHoverCard(
      padding: EdgeInsets.zero,
      onTap: live
          ? () => widget.shell.openControl(deviceName: label)
          : () => ScaffoldMessenger.of(context).showSnackBar(
                const SnackBar(content: Text(AppStrings.pendingWiring)),
              ),
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
                      text: live ? '在线' : '待连接',
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

  Widget _buildGrid() {
    return ValueListenableBuilder(
      valueListenable: widget.controller.gridIndex,
      builder: (context, gridIndex, _) {
        final count = WallController.gridOptions[gridIndex];
        final columns = count == 1 ? 1 : (count == 6 ? 3 : 2);
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
                childAspectRatio: 16 / 10,
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
                          hint: '等待首帧…',
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
                  hint: name == null ? AppStrings.wallWaiting : '该设备尚未建立预览流',
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
    return SingleChildScrollView(
      padding: const EdgeInsets.all(RdcTokens.spacePage),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          _buildHeader(),
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
          _buildGrid(),
          const SizedBox(height: RdcTokens.spaceMd),
          ValueListenableBuilder(
            valueListenable: widget.controller.status,
            builder: (context, status, _) {
              if (status == null) return const SizedBox.shrink();
              return Text(
                '采集 ${status.captureBackend} · 编码 ${status.backend} · '
                '${status.encodedFrames} 帧 · 解码 ${status.decodedFrames} 帧 · '
                '丢弃 ${status.droppedFrames}',
                style: const TextStyle(
                  fontSize: RdcTokens.fontCaption,
                  color: RdcTokens.textMuted,
                ),
              );
            },
          ),
        ],
      ),
    );
  }
}
