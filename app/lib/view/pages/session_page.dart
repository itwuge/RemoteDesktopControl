// ============================================================
// 页面用途：远程控制页（视频区 + 工具条；横竖屏两种布局）
//
// 层级：view/pages
// 被谁组装：shell_page.dart
// 数据来源：SessionController（角色权限来自 Rust；画面来自 Rust 管线注册的外部纹理）
// 边界：Windows 走外部纹理零拷贝；Linux 无外部纹理，按 RGBA 帧降级（同一 model/controller）
// 布局：宽（横屏/宽屏）→ 全出血画面 + 顶部悬浮工具条（3 秒淡出）；
//       窄（竖屏）→ 画面上、统计与工具条下的上下布局
// 旋转：只经 ViewportController 重算渲染矩阵；不重建 textureId、不重启采集管线
// ============================================================

import 'dart:async';

import 'package:flutter/gestures.dart';
import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../../controller/key_scan_map.dart';
import '../../controller/session_controller.dart';
import '../../controller/shell_controller.dart';
import '../../controller/viewport_controller.dart';
import '../../core/l10n/strings.dart';
import '../../core/theme/tokens.dart';
import '../../model/gateway/rust_types.dart';
import '../components/info_row.dart';
import '../components/rdc_button.dart';
import '../components/rgba_preview.dart';
import '../components/session_transfer_dialogs.dart';

/// 远程控制页。
class SessionPage extends StatefulWidget {
  /// 创建控制页。
  ///
  /// [controller] 角色与全屏状态；[shell] 当前设备与断开返回。
  const SessionPage({super.key, required this.controller, required this.shell});

  /// 会话控制器。
  final SessionController controller;

  /// 壳控制器。
  final ShellController shell;

  @override
  State<SessionPage> createState() => _SessionPageState();
}

/// 远程控制页状态。
class _SessionPageState extends State<SessionPage> {
  /// 视口控制器：方向切换与坐标矩阵的唯一入口（纯视图态）。
  final ViewportController _viewport = ViewportController();

  /// 宽布局工具条自动淡出定时器。
  Timer? _hideTimer;

  /// 宽布局工具条是否可见。
  bool _toolbarVisible = true;

  /// 上一次的宽布局判定，用于在方向切换时重置工具条。
  bool _lastWide = false;

  /// 上一次手势的累计缩放（ScaleUpdateDetails.scale 是累计值）。
  double _gestureScale = 1.0;

  /// 键盘注入的焦点节点：画面区获得焦点后才把按键送往被控端。
  final FocusNode _focusNode = FocusNode(debugLabel: 'rdc-session-input');

  /// 当前按下的鼠标键（`0` 无、`1` 左、`2` 右、`3` 中）；拖动时随移动下发。
  int _activeButton = 0;

  /// 注入回声抑制窗口（毫秒）：XTest 把真实指针移到远端坐标后，系统会把这次
  /// 移动作为 pointer move 回传进来；不忽略就会被当成新的用户移动再次下发，
  /// 形成「本地→远端」放大反馈（本机回环实测指针会一路跑飞）。
  static const int _echoWindowMs = 120;

  /// 距最近一次下发输入的单调计时器。
  final Stopwatch _injectClock = Stopwatch();

  /// 最近一次「用户意图」的远端坐标；抬手必须用它，否则 press/release 落两处。
  Offset? _lastRemotePoint;

  /// 会话控制器。
  SessionController get controller => widget.controller;

  /// 壳控制器。
  ShellController get shell => widget.shell;

  @override
  void initState() {
    super.initState();
    // 进入控制页即启动真实画面管线（幂等，由控制器内部去重）。
    controller.startPreview();
    controller.preview.addListener(_syncRemoteSize);
    _syncRemoteSize();
  }

  @override
  void dispose() {
    // 只释放本页新增的视图态；管线由控制器停止（旋转不会走到这里）。
    controller.preview.removeListener(_syncRemoteSize);
    _hideTimer?.cancel();
    _viewport.dispose();
    _focusNode.dispose();
    // 离开页面时释放纹理与采集管线。
    controller.stopPreview();
    super.dispose();
  }

  /// 把解码帧尺寸同步给视口控制器（只重算矩阵，不碰解码器）。
  void _syncRemoteSize() {
    final info = controller.preview.value;
    if (info == null) return;
    _viewport.updateRemoteSize(
      Size(info.width.toDouble(), info.height.toDouble()),
    );
  }

  /// 视口变化的唯一入口：post-frame 即「布局完成」，清脏标志放行输入。
  ///
  /// 返回：无返回值；尺寸未变时 [ViewportController.updateViewport] 自行去重。
  void _syncViewport(Size size) {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted) return;
      _viewport.updateViewport(size);
      _viewport.onLayoutDone();
      final wide = _viewport.isWideLayout;
      if (wide != _lastWide) {
        _lastWide = wide;
        if (wide) {
          _showToolbar(autoHide: true);
        } else {
          _hideTimer?.cancel();
          if (!_toolbarVisible) setState(() => _toolbarVisible = true);
        }
      }
    });
  }

  /// 显示工具条；[autoHide] 为真时 3 秒无操作淡出（仅宽布局使用）。
  void _showToolbar({required bool autoHide}) {
    _hideTimer?.cancel();
    if (!_toolbarVisible && mounted) {
      setState(() => _toolbarVisible = true);
    }
    if (autoHide) {
      _hideTimer = Timer(const Duration(seconds: 3), () {
        if (mounted) setState(() => _toolbarVisible = false);
      });
    }
  }

  /// 点击画面：宽布局唤回/收起工具条。
  void _onSurfaceTap(bool wide) {
    if (!wide) return;
    if (_toolbarVisible) {
      _hideTimer?.cancel();
      setState(() => _toolbarVisible = false);
    } else {
      _showToolbar(autoHide: true);
    }
  }

  /// 手势：双指缩放围绕焦点、单指拖动平移；矩阵未同步时丢弃输入。
  void _handleScaleUpdate(ScaleUpdateDetails details) {
    if (_viewport.matrixDirty || _inEchoWindow) return;
    if (details.pointerCount > 1) {
      final factor = details.scale / _gestureScale;
      _gestureScale = details.scale;
      _viewport.zoom(factor, focal: details.localFocalPoint);
    } else if (details.focalPointDelta != Offset.zero) {
      _viewport.pan(details.focalPointDelta);
    }
  }

  // =====** 输入注入：指针与键盘 =====**

  /// 是否处于注入回声窗口（窗口内的 pointer move 一律忽略，不下发也不平移）。
  bool get _inEchoWindow =>
      _injectClock.isRunning && _injectClock.elapsedMilliseconds < _echoWindowMs;

  /// 记录一次下发，开启回声抑制窗口。
  void _markInjected() {
    _injectClock
      ..reset()
      ..start();
  }

  /// 是否可以注入：有角色权限且画面未中断（断线时不允许再把事件打到被控端）。
  bool get _inputReady =>
      controller.canInjectInput.value && controller.previewError.value == null;

  /// 本地像素 → 被控端桌面物理像素（唯一坐标入口，走 ViewportController 矩阵）。
  ///
  /// [local] 为画面区内的本地坐标；[clamp] 为真时把越界点钳到画面边缘
  /// （抬手/取消必须能复位按键，即使指针已移出内容区）。
  Offset? _desktopPoint(Offset local, {bool clamp = false}) {
    if (_viewport.matrixDirty) return null;
    final remote = _viewport.remoteSize;
    if (remote.isEmpty) return null;
    final normalized = _viewport.screenToRemote(local);
    if (!clamp &&
        (normalized.dx < 0 ||
            normalized.dx > 1 ||
            normalized.dy < 0 ||
            normalized.dy > 1)) {
      return null;
    }
    final nx = normalized.dx.clamp(0.0, 1.0).toDouble();
    final ny = normalized.dy.clamp(0.0, 1.0).toDouble();
    final x = (nx * remote.width).round().clamp(0, remote.width.floor() - 1);
    final y = (ny * remote.height).round().clamp(0, remote.height.floor() - 1);
    return Offset(x.toDouble(), y.toDouble());
  }

  /// 指针按键编码：`1` 左、`2` 右、`3` 中（与 S4 线格式一致）。
  int _buttonCode(int buttons) {
    if (buttons & kPrimaryButton != 0) return 1;
    if (buttons & kSecondaryButton != 0) return 2;
    if (buttons & kMiddleMouseButton != 0) return 3;
    return 0;
  }

  /// 指针按下：请求键盘焦点并下发按键按下事件。
  void _onPointerDown(PointerDownEvent event) {
    if (!_inputReady) return;
    _focusNode.requestFocus();
    final code = _buttonCode(event.buttons);
    if (code == 0) return;
    final point = _desktopPoint(event.localPosition);
    if (point == null) return;
    _activeButton = code;
    _lastRemotePoint = point;
    controller.sendInputEvents([
      InputEventSpec.button(
        x: point.dx.toInt(),
        y: point.dy.toInt(),
        button: code,
        pressed: true,
      ),
    ]);
    _markInjected();
  }

  /// 指针移动：无按键时是纯移动；按住时带上按键，交由合并器按拖动限频。
  ///
  /// 处于回声窗口的移动来自我们自己的注入（XTest 移动了真实指针），必须丢弃：
  /// 否则会被当成新的用户移动再次下发，形成放大反馈。
  void _onPointerMove(PointerMoveEvent event) {
    if (!_inputReady || _inEchoWindow) return;
    final point = _desktopPoint(event.localPosition);
    if (point == null) return;
    _lastRemotePoint = point;
    final x = point.dx.toInt();
    final y = point.dy.toInt();
    if (_activeButton == 0) {
      controller.sendInputEvents([InputEventSpec.move(x: x, y: y)]);
    } else {
      controller.sendInputEvents([
        InputEventSpec.button(x: x, y: y, button: _activeButton, pressed: true),
      ]);
    }
    _markInjected();
  }

  /// 指针抬起/取消：先抬起按键，再冲刷挂起移动（末点不丢）。
  ///
  /// 抬手用 [_lastRemotePoint]（用户意图点），不用 `localPosition`：
  /// 注入已把真实指针挪走，按回传位置抬起会 press/release 落两处。
  void _onPointerRelease(Offset local) {
    if (!_inputReady) return;
    final code = _activeButton;
    _activeButton = 0;
    if (code == 0) {
      controller.flushInputEvents();
      return;
    }
    final point = _lastRemotePoint ?? _desktopPoint(local, clamp: true);
    _lastRemotePoint = null;
    if (point == null) {
      controller.flushInputEvents();
      return;
    }
    controller.sendInputEvents([
      InputEventSpec.button(
        x: point.dx.toInt(),
        y: point.dy.toInt(),
        button: code,
        pressed: false,
      ),
    ]);
    _markInjected();
    controller.flushInputEvents();
  }

  /// 滚轮：Flutter 的像素增量换算为刻度（100 px ≈ 1 格）；方向与被控端一致。
  void _onPointerSignal(PointerSignalEvent event) {
    if (!_inputReady) return;
    if (event is! PointerScrollEvent) return;
    if (_desktopPoint(event.localPosition) == null) return;
    var dx = (event.scrollDelta.dx / 100).round();
    var dy = (event.scrollDelta.dy / 100).round();
    if (dx == 0 && event.scrollDelta.dx != 0) {
      dx = event.scrollDelta.dx > 0 ? 1 : -1;
    }
    if (dy == 0 && event.scrollDelta.dy != 0) {
      dy = event.scrollDelta.dy > 0 ? 1 : -1;
    }
    if (dx == 0 && dy == 0) return;
    controller.sendInputEvents([InputEventSpec.wheel(deltaX: dx, deltaY: dy)]);
    _markInjected();
    controller.flushInputEvents();
  }

  /// 键盘：物理键 → 扫描码，按下/抬起原序下发；修饰位随事件携带。
  KeyEventResult _handleKeyEvent(FocusNode node, KeyEvent event) {
    if (!_inputReady) return KeyEventResult.ignored;
    final scanCode = scanCodeFor(event.physicalKey);
    if (scanCode == null) return KeyEventResult.ignored;
    final bool? pressed;
    if (event is KeyDownEvent || event is KeyRepeatEvent) {
      pressed = true;
    } else if (event is KeyUpEvent) {
      pressed = false;
    } else {
      pressed = null;
    }
    if (pressed == null) return KeyEventResult.ignored;
    controller.sendInputEvents([
      InputEventSpec.key(
        scanCode: scanCode,
        pressed: pressed,
        modifiers: currentModifierMask(),
      ),
    ]);
    return KeyEventResult.handled;
  }

  // =====** 组件：视频区 =====**

  /// 画面区：外部纹理 + ViewportController 的渲染矩阵（不逐帧拷贝）。
  ///
  /// 只调整 `Transform` 与 `Texture` 的显示尺寸，绝不重建 `textureId`。
  Widget _buildSurface(bool wide) {
    return LayoutBuilder(
      builder: (context, constraints) {
        _syncViewport(constraints.biggest);
        return Listener(
          behavior: HitTestBehavior.opaque,
          onPointerDown: _onPointerDown,
          onPointerMove: _onPointerMove,
          onPointerUp: (event) => _onPointerRelease(event.localPosition),
          onPointerCancel: (event) => _onPointerRelease(event.localPosition),
          onPointerSignal: _onPointerSignal,
          child: Stack(
            fit: StackFit.expand,
            children: [
              GestureDetector(
                behavior: HitTestBehavior.opaque,
                onTap: () => _onSurfaceTap(wide),
                onDoubleTap: _viewport.reset,
                onScaleStart: (_) => _gestureScale = 1.0,
                onScaleUpdate: _handleScaleUpdate,
                child: ValueListenableBuilder<int?>(
                  valueListenable: controller.textureId,
                  builder: (context, textureId, _) {
                    return ValueListenableBuilder<ControlFrameInfo?>(
                      valueListenable: controller.frame,
                      builder: (context, frame, _) {
                        if (textureId == null && frame == null) {
                          return _buildPlaceholder();
                        }
                        return ListenableBuilder(
                          listenable: _viewport,
                          builder: (context, _) {
                            // 显示矩形完全由 ViewportController 的同一矩阵推出；
                            // 用定位矩形而不是 Transform，避免外部纹身在变换下漏绘。
                            final rect = _viewport.displayRect;
                            return ClipRect(
                              child: ColoredBox(
                                color: Colors.black,
                                child: Stack(
                                  children: [
                                    Positioned(
                                      left: rect.left,
                                      top: rect.top,
                                      width: rect.width,
                                      height: rect.height,
                                      // Windows：外部纹理零拷贝上屏；
                                      // Linux：RGBA 帧降级通道（逐帧转换，见 SessionController）。
                                      child: textureId != null
                                          ? Texture(
                                              textureId: textureId,
                                              filterQuality: FilterQuality.low,
                                            )
                                          : RdcRgbaPreview(
                                              bytes: frame!.bytes,
                                              width: frame.width,
                                              height: frame.height,
                                              fit: BoxFit.fill,
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
                  },
                ),
              ),
              _buildDisconnectBanner(),
            ],
          ),
        );
      },
    );
  }

  /// 纹理尚未接通时的明确说明（不显示假画面）。
  Widget _buildPlaceholder() {
    return ValueListenableBuilder<String?>(
      valueListenable: controller.previewError,
      builder: (context, error, _) {
        return Container(
          color: Colors.black,
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
                if (error != null) ...[
                  const SizedBox(height: RdcTokens.spaceLg),
                  RdcButton(
                    label: AppStrings.reconnect,
                    icon: Icons.refresh,
                    primary: true,
                    onPressed: _reconnect,
                  ),
                ],
              ],
            ),
          ),
        );
      },
    );
  }

  /// 画面仍有纹理但解码中断时的横幅 + 重连入口（不静默卡成黑屏）。
  Widget _buildDisconnectBanner() {
    return ValueListenableBuilder<String?>(
      valueListenable: controller.previewError,
      builder: (context, error, _) {
        if (error == null ||
            (controller.textureId.value == null &&
                controller.frame.value == null)) {
          return const SizedBox.shrink();
        }
        return Positioned(
          left: RdcTokens.spaceLg,
          right: RdcTokens.spaceLg,
          bottom: RdcTokens.spaceLg + 44,
          child: Center(
            child: Container(
              padding: const EdgeInsets.symmetric(
                horizontal: RdcTokens.spaceMd,
                vertical: RdcTokens.spaceSm,
              ),
              decoration: BoxDecoration(
                color: RdcTokens.surface.withValues(alpha: 0.94),
                borderRadius: BorderRadius.circular(RdcTokens.radiusCard),
                border: Border.all(color: RdcTokens.danger),
              ),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  const Icon(
                    Icons.link_off,
                    size: 16,
                    color: RdcTokens.danger,
                  ),
                  const SizedBox(width: RdcTokens.spaceSm),
                  Flexible(
                    child: Text(
                      '${AppStrings.sessionDisconnected}$error',
                      style: const TextStyle(
                        fontSize: RdcTokens.fontCaption,
                        color: RdcTokens.textPrimary,
                      ),
                    ),
                  ),
                  const SizedBox(width: RdcTokens.spaceSm),
                  RdcButton(
                    label: AppStrings.reconnect,
                    icon: Icons.refresh,
                    compact: true,
                    onPressed: _reconnect,
                  ),
                ],
              ),
            ),
          ),
        );
      },
    );
  }

  // =====** 组件：指标与设备标识 =====**

  /// 画面上的连接质量角标（fps / 端到端延迟 / 码率 / 丢帧），全部来自 Rust 真实统计。
  Widget _buildStatsPill() {
    return ValueListenableBuilder<ControlPreviewInfo?>(
      valueListenable: controller.preview,
      builder: (context, info, _) {
        if (info == null || !info.running) return const SizedBox.shrink();
        final latencyMs = info.latencyLastUs / 1000.0;
        final kbps = info.bitrateKbps;
        final bitrate = kbps >= 1000
            ? '${(kbps / 1000).toStringAsFixed(1)} Mbps'
            : '$kbps kbps';
        return Container(
          padding: const EdgeInsets.symmetric(horizontal: 10, vertical: 5),
          decoration: BoxDecoration(
            color: RdcTokens.background.withValues(alpha: 0.72),
            borderRadius: BorderRadius.circular(RdcTokens.radiusPill),
          ),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            mainAxisSize: MainAxisSize.min,
            children: [
              Text(
                '${info.fps.toStringAsFixed(1)} fps · '
                '${latencyMs.toStringAsFixed(0)} ms · '
                '$bitrate · 丢 ${info.droppedFrames}'
                // 编码看门狗重建次数：>0 时必须让用户看到（截图反馈用）。
                '${info.encoderResets > 0 ? ' · 重编 ${info.encoderResets}' : ''}',
                style: const TextStyle(
                  fontSize: RdcTokens.fontCaption,
                  color: RdcTokens.textPrimary,
                ),
              ),
              // 稳定错误码（如 RDC-ENC-001）不静默：直接显示在角标里。
              if (info.lastError != null && info.lastError!.isNotEmpty)
                Text(
                  info.lastError!,
                  style: const TextStyle(
                    fontSize: RdcTokens.fontCaption,
                    color: RdcTokens.danger,
                  ),
                ),
            ],
          ),
        );
      },
    );
  }

  /// 设备名 + 输入权限胶囊。
  Widget _buildDeviceChips() {
    return ValueListenableBuilder(
      valueListenable: shell.activeDeviceName,
      builder: (context, deviceName, _) {
        return Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Container(
              padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 6),
              decoration: BoxDecoration(
                color: RdcTokens.background.withValues(alpha: 0.7),
                borderRadius: BorderRadius.circular(RdcTokens.radiusPill),
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
                color: allowed ? RdcTokens.success : RdcTokens.textMuted,
              ),
            ),
          ],
        );
      },
    );
  }

  /// 重连：先停掉残留管线，再重新启动采集与外部纹理。
  Future<void> _reconnect() async {
    await controller.stopPreview();
    await controller.startPreview();
  }

  /// 断开：先走 bye 收尾（报告写进提示），再返回设备列表。
  Future<void> _disconnect() async {
    final report = await controller.endSession();
    if (!mounted) return;
    final detail = report == null
        ? '收尾报告缺失（详见日志）'
        : 'bye ${report.byeAcked ? "已确认" : "未确认"}'
            ' · 服务残留 ${report.servicesRunningAfter}'
            ' · 句柄 ${report.handlesDelta >= 0 ? "+" : ""}${report.handlesDelta}'
            ' · 端口${report.portsReleased ? "已释放" : "未释放"}';
    ScaffoldMessenger.of(context).showSnackBar(
      SnackBar(content: Text('会话已断开：$detail')),
    );
    shell.disconnectToDevices();
  }

  // =====** 组件：工具条（两个方向复用同一组操作，仅布局不同） =====**

  /// 工具条；[floating] 为真时是宽布局的顶部悬浮样式。
  Widget _buildToolbar({required bool floating, required bool wide}) {
    final actions = <Widget>[
      RdcButton(
        label: AppStrings.disconnect,
        icon: Icons.logout,
        onPressed: _disconnect,
      ),
      if (wide)
        RdcButton(
          label: AppStrings.resetView,
          icon: Icons.center_focus_strong,
          onPressed: _viewport.reset,
        )
      else
        RdcButton(
          label: AppStrings.fullscreen,
          icon: Icons.fullscreen,
          onPressed: controller.toggleFullscreen,
        ),
      RdcButton(
        label: AppStrings.clipboard,
        icon: Icons.content_paste,
        onPressed: () => showClipboardDialog(context, controller),
      ),
      RdcButton(
        label: AppStrings.files,
        icon: Icons.folder_open,
        onPressed: () => showFileTransferDialog(context, controller),
      ),
    ];
    return Container(
      padding: EdgeInsets.symmetric(
        horizontal: RdcTokens.spaceMd,
        vertical: floating ? RdcTokens.spaceSm : RdcTokens.spaceMd,
      ),
      decoration: BoxDecoration(
        color: RdcTokens.surface.withValues(alpha: floating ? 0.92 : 1.0),
        borderRadius: BorderRadius.circular(RdcTokens.radiusCard),
        border: Border.all(color: RdcTokens.border),
        boxShadow: floating
            ? const [
                BoxShadow(
                  color: Color(0x66000000),
                  blurRadius: 24,
                  offset: Offset(0, 8),
                ),
              ]
            : null,
      ),
      child: Wrap(
        spacing: RdcTokens.spaceSm,
        runSpacing: RdcTokens.spaceSm,
        alignment: WrapAlignment.center,
        crossAxisAlignment: WrapCrossAlignment.center,
        children: actions,
      ),
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

  // =====** 组件：两种方向布局 =====**

  /// 宽布局：全出血画面 + 顶部悬浮工具条（3 秒淡出）+ 底部两侧信息胶囊。
  Widget _buildWideLayout() {
    return Stack(
      fit: StackFit.expand,
      children: [
        Positioned.fill(child: _buildSurface(true)),
        Positioned(
          top: RdcTokens.spaceMd,
          left: 0,
          right: 0,
          child: IgnorePointer(
            ignoring: !_toolbarVisible,
            child: AnimatedOpacity(
              opacity: _toolbarVisible ? 1.0 : 0.0,
              duration: const Duration(milliseconds: 220),
              child: Center(child: _buildToolbar(floating: true, wide: true)),
            ),
          ),
        ),
        Positioned.fill(
          child: Padding(
            padding: const EdgeInsets.all(RdcTokens.spaceLg),
            child: Align(
              alignment: Alignment.bottomLeft,
              child: _buildDeviceChips(),
            ),
          ),
        ),
        Positioned.fill(
          child: Padding(
            padding: const EdgeInsets.all(RdcTokens.spaceLg),
            child: Align(
              alignment: Alignment.bottomRight,
              child: _buildStatsPill(),
            ),
          ),
        ),
      ],
    );
  }

  /// 窄布局（竖屏）：画面在上，统计与工具条在下，最后是会话信息。
  Widget _buildNarrowLayout(BoxConstraints constraints) {
    final ratio = _viewport.remoteSize.aspectRatio;
    return SingleChildScrollView(
      padding: const EdgeInsets.all(RdcTokens.spacePage),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          ValueListenableBuilder(
            valueListenable: controller.fullscreen,
            builder: (context, fullscreen, _) {
              if (fullscreen) {
                return SizedBox(
                  height: constraints.maxHeight * 0.55,
                  child: ClipRRect(
                    borderRadius: BorderRadius.circular(RdcTokens.radiusCard),
                    child: _buildSurface(false),
                  ),
                );
              }
              return ClipRRect(
                borderRadius: BorderRadius.circular(RdcTokens.radiusCard),
                child: AspectRatio(
                  aspectRatio: ratio,
                  child: _buildSurface(false),
                ),
              );
            },
          ),
          const SizedBox(height: RdcTokens.spaceMd),
          Row(
            children: [
              Expanded(child: _buildDeviceChips()),
              const SizedBox(width: RdcTokens.spaceSm),
              _buildStatsPill(),
            ],
          ),
          const SizedBox(height: RdcTokens.spaceMd),
          _buildToolbar(floating: false, wide: false),
          const SizedBox(height: RdcTokens.spaceLg),
          _buildInfo(),
        ],
      ),
    );
  }

  @override
  Widget build(BuildContext context) {
    // 焦点收口在页面根节点：画面区按下即获焦，键盘事件统一走 _handleKeyEvent。
    return Focus(
      focusNode: _focusNode,
      autofocus: true,
      onKeyEvent: _handleKeyEvent,
      child: LayoutBuilder(
        builder: (context, constraints) {
          // 断点用约束几何，不用 Orientation（平板分屏时 Orientation 会撒谎）。
          final wide = ViewportController.resolveWide(constraints.biggest);
          return wide ? _buildWideLayout() : _buildNarrowLayout(constraints);
        },
      ),
    );
  }
}
