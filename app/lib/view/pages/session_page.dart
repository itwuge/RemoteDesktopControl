// ============================================================
// 页面用途：远程控制页（视频区 + 底部悬浮工具条）
//
// 层级：view/pages
// 被谁组装：shell_page.dart
// 数据来源：SessionController（角色权限来自 Rust；画面来自 Rust 管线注册的外部纹理）
// 边界：视频必须走外部纹理，不允许逐帧转 Image；纹理未接通时显示明确说明
// ============================================================

import 'package:flutter/material.dart';

import '../../controller/session_controller.dart';
import '../../controller/shell_controller.dart';
import '../../core/l10n/strings.dart';
import '../../core/theme/tokens.dart';
import '../../model/gateway/rust_types.dart';
import '../components/info_row.dart';
import '../components/rdc_button.dart';

/// 远程控制页。
class SessionPage extends StatefulWidget {
  /// 创建控制页。
  ///
  /// [controller] 角色与全屏状态；[shell] 当前设备与断开返回。
  const SessionPage({
    super.key,
    required this.controller,
    required this.shell,
  });

  /// 会话控制器。
  final SessionController controller;

  /// 壳控制器。
  final ShellController shell;

  @override
  State<SessionPage> createState() => _SessionPageState();
}

/// 远程控制页状态。
class _SessionPageState extends State<SessionPage> {
  /// 会话控制器。
  SessionController get controller => widget.controller;

  /// 壳控制器。
  ShellController get shell => widget.shell;

  @override
  void initState() {
    super.initState();
    // 进入控制页即启动真实画面管线（幂等，由控制器内部去重）。
    controller.startPreview();
  }

  @override
  void dispose() {
    // 离开页面时释放纹理与采集管线。
    controller.stopPreview();
    super.dispose();
  }

  /// 提示某功能待接线。
  void _pending(BuildContext context) {
    ScaffoldMessenger.of(context).showSnackBar(
      const SnackBar(content: Text(AppStrings.pendingWiring)),
    );
  }

  // =====** 组件：视频区 + 悬浮工具条 =====**

  /// 真实纹理画面或明确的待接通说明。
  Widget _buildSurface() {
    return ValueListenableBuilder<int?>(
      valueListenable: controller.textureId,
      builder: (context, textureId, _) {
        if (textureId != null) {
          return ValueListenableBuilder<ControlPreviewInfo?>(
            valueListenable: controller.preview,
            builder: (context, info, _) {
              final width = info?.width ?? 1920;
              final height = info?.height ?? 1080;
              return ClipRRect(
                borderRadius: BorderRadius.circular(RdcTokens.radiusCard),
                child: ColoredBox(
                  color: Colors.black,
                  child: FittedBox(
                    fit: BoxFit.contain,
                    child: SizedBox(
                      width: width.toDouble(),
                      height: height.toDouble(),
                      child: Texture(
                        textureId: textureId,
                        filterQuality: FilterQuality.low,
                      ),
                    ),
                  ),
                ),
              );
            },
          );
        }
        return ValueListenableBuilder<String?>(
          valueListenable: controller.previewError,
          builder: (context, error, _) {
            return Container(
              decoration: BoxDecoration(
                color: Colors.black,
                borderRadius: BorderRadius.circular(RdcTokens.radiusCard),
                border: Border.all(color: RdcTokens.border),
              ),
              alignment: Alignment.center,
              child: Padding(
                padding: const EdgeInsets.all(RdcTokens.spaceXl),
                child: Column(
                  mainAxisSize: MainAxisSize.min,
                  children: [
                    Icon(
                      error == null
                          ? Icons.videocam_outlined
                          : Icons.videocam_off_outlined,
                      size: 38,
                      color: RdcTokens.textMuted,
                    ),
                    const SizedBox(height: RdcTokens.spaceMd),
                    Text(
                      error == null
                          ? AppStrings.sessionTextureStarting
                          : '${AppStrings.sessionTextureFailed}$error',
                      textAlign: TextAlign.center,
                      style: const TextStyle(
                        fontSize: RdcTokens.fontBody,
                        color: RdcTokens.textMuted,
                      ),
                    ),
                  ],
                ),
              ),
            );
          },
        );
      },
    );
  }

  /// 画面上的实时指标（fps / 端到端延迟 / 丢帧）。
  Widget _buildStatsPill() {
    return ValueListenableBuilder<ControlPreviewInfo?>(
      valueListenable: controller.preview,
      builder: (context, info, _) {
        if (info == null || !info.running) return const SizedBox.shrink();
        final latencyMs = info.latencyLastUs / 1000.0;
        return Container(
          padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
          decoration: BoxDecoration(
            color: RdcTokens.background.withValues(alpha: 0.72),
            borderRadius: BorderRadius.circular(RdcTokens.radiusPill),
          ),
          child: Text(
            '${info.fps.toStringAsFixed(1)} fps · '
            '${latencyMs.toStringAsFixed(0)} ms · 丢 ${info.droppedFrames}',
            style: const TextStyle(
              fontSize: RdcTokens.fontCaption,
              color: RdcTokens.textPrimary,
            ),
          ),
        );
      },
    );
  }

  // =====** 组件：视频区 + 悬浮工具条 =====**

  Widget _buildVideoArea(BuildContext context) {
    return ValueListenableBuilder(
      valueListenable: controller.fullscreen,
      builder: (context, fullscreen, _) {
        final height = fullscreen
            ? MediaQuery.sizeOf(context).height * 0.72
            : 420.0;
        return SizedBox(
          height: height,
          child: ValueListenableBuilder(
            valueListenable: shell.activeDeviceName,
            builder: (context, deviceName, _) {
              return Stack(
                fit: StackFit.expand,
                children: [
                  // 真实画面：外部纹理直接上屏，不做逐帧 RGBA 拷贝。
                  _buildSurface(),
                  // 右上角：实测帧率 / 端到端延迟 / 丢帧
                  Positioned(
                    right: RdcTokens.spaceLg,
                    top: RdcTokens.spaceLg,
                    child: _buildStatsPill(),
                  ),
                  // 左上角：设备名 + 角色
                  Positioned(
                    left: RdcTokens.spaceLg,
                    top: RdcTokens.spaceLg,
                    child: Row(
                      children: [
                        Container(
                          padding: const EdgeInsets.symmetric(
                            horizontal: 12,
                            vertical: 6,
                          ),
                          decoration: BoxDecoration(
                            color: RdcTokens.background.withValues(alpha: 0.7),
                            borderRadius:
                                BorderRadius.circular(RdcTokens.radiusPill),
                          ),
                          child: Text(
                            deviceName,
                            style: const TextStyle(
                              fontSize: RdcTokens.fontCaption,
                              fontWeight: FontWeight.w600,
                            ),
                          ),
                        ),
                        const SizedBox(width: RdcTokens.spaceSm),
                        ValueListenableBuilder(
                          valueListenable: controller.canInjectInput,
                          builder: (context, allowed, _) => RdcStatusPill(
                            text: allowed
                                ? AppStrings.inputAllowed
                                : AppStrings.inputDenied,
                            color: allowed
                                ? RdcTokens.success
                                : RdcTokens.textMuted,
                          ),
                        ),
                      ],
                    ),
                  ),
                  // 底部悬浮工具条
                  Positioned(
                    left: 0,
                    right: 0,
                    bottom: RdcTokens.spaceLg,
                    child: Center(
                      child: Container(
                        padding: const EdgeInsets.symmetric(
                          horizontal: RdcTokens.spaceMd,
                          vertical: RdcTokens.spaceSm,
                        ),
                        decoration: BoxDecoration(
                          color: RdcTokens.surface.withValues(alpha: 0.92),
                          borderRadius:
                              BorderRadius.circular(RdcTokens.radiusCard),
                          border: Border.all(color: RdcTokens.border),
                          boxShadow: const [
                            BoxShadow(
                              color: Color(0x66000000),
                              blurRadius: 24,
                              offset: Offset(0, 8),
                            ),
                          ],
                        ),
                        child: Row(
                          mainAxisSize: MainAxisSize.min,
                          children: [
                            RdcButton(
                              label: AppStrings.disconnect,
                              icon: Icons.logout,
                              onPressed: shell.disconnectToDevices,
                            ),
                            const SizedBox(width: RdcTokens.spaceSm),
                            RdcButton(
                              label: AppStrings.fullscreen,
                              icon: Icons.fullscreen,
                              onPressed: controller.toggleFullscreen,
                            ),
                            const SizedBox(width: RdcTokens.spaceSm),
                            RdcButton(
                              label: AppStrings.clipboard,
                              icon: Icons.content_paste,
                              onPressed: () => _pending(context),
                            ),
                            const SizedBox(width: RdcTokens.spaceSm),
                            RdcButton(
                              label: AppStrings.files,
                              icon: Icons.folder_open,
                              onPressed: () => _pending(context),
                            ),
                          ],
                        ),
                      ),
                    ),
                  ),
                ],
              );
            },
          ),
        );
      },
    );
  }

  // =====** 组件：会话信息 =====**

  Widget _buildInfo() {
    return ValueListenableBuilder(
      valueListenable: controller.errorMessage,
      builder: (context, error, _) {
        return Container(
          padding: const EdgeInsets.all(RdcTokens.spaceCard),
          decoration: BoxDecoration(
            color: RdcTokens.surface,
            borderRadius: BorderRadius.circular(RdcTokens.radiusCard),
            border: Border.all(color: RdcTokens.border),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              ValueListenableBuilder(
                valueListenable: controller.role,
                builder: (context, role, _) => RdcInfoRow(
                  label: '会话角色',
                  value: role == 'controller'
                      ? AppStrings.roleController
                      : AppStrings.roleViewer,
                ),
              ),
              ValueListenableBuilder(
                valueListenable: controller.canViewWall,
                builder: (context, canView, _) => RdcInfoRow(
                  label: '屏幕墙权限',
                  value: canView ? '允许拉子码流' : '不允许',
                  valueColor: canView ? RdcTokens.success : RdcTokens.textMuted,
                ),
              ),
              ValueListenableBuilder(
                valueListenable: shell.isLocalPreview,
                builder: (context, local, _) => RdcInfoRow(
                  label: '连接目标',
                  value: local ? '本机回环（未接入真实会话）' : '设备会话（传输待接线）',
                ),
              ),
              if (error != null)
                RdcInfoRow(
                  label: '错误',
                  value: error,
                  valueColor: RdcTokens.danger,
                ),
            ],
          ),
        );
      },
    );
  }

  @override
  Widget build(BuildContext context) {
    return SingleChildScrollView(
      padding: const EdgeInsets.all(RdcTokens.spacePage),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          _buildVideoArea(context),
          const SizedBox(height: RdcTokens.spaceLg),
          _buildInfo(),
        ],
      ),
    );
  }
}
