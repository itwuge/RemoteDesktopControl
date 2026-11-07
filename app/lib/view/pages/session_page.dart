// ============================================================
// 页面用途：远程控制页（视频区 + 底部悬浮工具条）
//
// 层级：view/pages
// 被谁组装：shell_page.dart
// 数据来源：SessionController（角色权限来自 Rust；画面纹理待接线）
// 边界：视频必须走外部纹理，不允许逐帧转 Image；纹理未接通时显示明确待接线说明
// ============================================================

import 'package:flutter/material.dart';

import '../../controller/session_controller.dart';
import '../../controller/shell_controller.dart';
import '../../core/l10n/strings.dart';
import '../../core/theme/tokens.dart';
import '../components/info_row.dart';
import '../components/rdc_button.dart';

/// 远程控制页。
class SessionPage extends StatelessWidget {
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

  /// 提示某功能待接线。
  void _pending(BuildContext context) {
    ScaffoldMessenger.of(context).showSnackBar(
      const SnackBar(content: Text(AppStrings.pendingWiring)),
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
                  // 视频占位：明确说明外部纹理未接通，不画假画面。
                  Container(
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
                          const Icon(
                            Icons.videocam_off_outlined,
                            size: 38,
                            color: RdcTokens.textMuted,
                          ),
                          const SizedBox(height: RdcTokens.spaceMd),
                          const Text(
                            AppStrings.sessionTexturePending,
                            textAlign: TextAlign.center,
                            style: TextStyle(
                              fontSize: RdcTokens.fontBody,
                              color: RdcTokens.textMuted,
                            ),
                          ),
                          const SizedBox(height: RdcTokens.spaceSm),
                          RdcStatusPill(
                            text: AppStrings.pendingBadge,
                            color: RdcTokens.danger,
                          ),
                        ],
                      ),
                    ),
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
