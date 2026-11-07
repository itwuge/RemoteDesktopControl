// ============================================================
// 文件用途：远程控制页控制器（角色权限 + 真实画面管线 + 全屏与工具条状态）
//
// 层级：controller
// 被谁调用：view/pages/session_page.dart
// 边界：画面走外部纹理（Texture），不逐帧转 Image；控制器只透传纹理 ID 与状态
// ============================================================

import 'dart:async';

import 'package:flutter/foundation.dart';

import '../core/utils/logger.dart';
import '../model/gateway/rust_gateway.dart';
import '../model/gateway/rust_types.dart';

/// 远程控制状态与行为。
class SessionController {
  /// 创建控制器。
  ///
  /// [gateway] 由 main.dart 注入。
  SessionController({required this.gateway});

  /// FFI 网关。
  final RustGateway gateway;

  /// 当前角色（controller / viewer）。
  final ValueNotifier<String> role = ValueNotifier('controller');

  /// 当前角色是否可注入输入。
  final ValueNotifier<bool> canInjectInput = ValueNotifier(false);

  /// 当前角色是否可看墙。
  final ValueNotifier<bool> canViewWall = ValueNotifier(false);

  /// 画面区是否全屏。
  final ValueNotifier<bool> fullscreen = ValueNotifier(false);

  /// 错误信息。
  final ValueNotifier<String?> errorMessage = ValueNotifier(null);

  /// Flutter 外部纹理 ID；`null` 表示尚未接通。
  final ValueNotifier<int?> textureId = ValueNotifier(null);

  /// 控制页实测指标（fps / 延迟 / 丢帧）。
  final ValueNotifier<ControlPreviewInfo?> preview = ValueNotifier(null);

  /// 画面启动失败原因。
  final ValueNotifier<String?> previewError = ValueNotifier(null);

  /// 输入注入统计（来自 Rust 的真实计数，每秒刷新）。
  final ValueNotifier<InputStatsInfo> inputStats = ValueNotifier(
    InputStatsInfo.empty(),
  );

  /// 输入通道错误（与画面错误分开，输入失败不中断画面）。
  final ValueNotifier<String?> inputError = ValueNotifier(null);

  /// 会话链路状态（回环端口与 bye 状态）；`null` 表示未建立会话。
  final ValueNotifier<SessionStatusInfo?> session = ValueNotifier(null);

  /// 输入通道是否已启动。
  bool _inputRunning = false;

  /// 输入事件串行链：保证按键/指针按构造顺序抵达 Rust（FFI 调用本身是异步的）。
  Future<void> _inputChain = Future<void>.value();

  /// 状态轮询定时器。
  Timer? _statusTimer;

  /// 是否正在启动（防止重复进入）。
  bool _starting = false;

  /// 查询角色权限（真实调用 Rust 授权模块）。
  ///
  /// 返回：无返回值；结果写入三个角色通知器。
  Future<void> loadRole() async {
    try {
      final info = await gateway.roleInfo(role.value);
      role.value = info.role;
      canInjectInput.value = info.canInjectInput;
      canViewWall.value = info.canViewWall;
      errorMessage.value = null;
    } catch (error, stackTrace) {
      canInjectInput.value = false;
      canViewWall.value = false;
      errorMessage.value = '角色查询失败：$error';
      RdcLogger.log(
        'SessionController',
        '角色查询失败',
        level: LogLevel.error,
        error: error,
        stackTrace: stackTrace,
      );
    }
  }

  /// 启动真实画面管线：Rust 采集被控端桌面 → 编码 → 解码 → 共享纹理，
  /// 再把共享句柄注册为 Flutter 外部纹理。
  ///
  /// 返回：无返回值；成功后 [textureId] 非空，指标每秒刷新。
  Future<void> startPreview() async {
    if (_starting || textureId.value != null) return;
    _starting = true;
    previewError.value = null;
    try {
      final info = await gateway.startControlPreview();
      final id = await gateway.registerControlTexture(info);
      preview.value = info;
      if (id == null) {
        previewError.value = '外部纹理注册失败（非 Windows 或 runner 未挂载通道）';
        await gateway.stopControlPreview();
      } else {
        textureId.value = id;
        // 画面接通后建立会话链路：bye 握手与资源清理都挂在会话生命周期上。
        await _startSession();
        // 画面接通后才启动输入通道：没有画面就没有可靠的坐标映射。
        await _startInput(info);
        _statusTimer?.cancel();
        _statusTimer = Timer.periodic(
          const Duration(seconds: 1),
          (_) => _refreshStatus(),
        );
      }
    } catch (error, stackTrace) {
      previewError.value = '$error';
      RdcLogger.log(
        'SessionController',
        '真实画面启动失败',
        level: LogLevel.error,
        error: error,
        stackTrace: stackTrace,
      );
    } finally {
      _starting = false;
    }
  }

  /// 停止画面管线：先注销纹理，再停止输入与 Rust（避免句柄提前释放）。
  Future<void> stopPreview() async {
    _statusTimer?.cancel();
    _statusTimer = null;
    try {
      await gateway.unregisterControlTexture();
    } catch (error) {
      RdcLogger.log('SessionController', '注销纹理失败: $error');
    }
    await _stopInput();
    try {
      await gateway.stopControlPreview();
    } catch (error) {
      RdcLogger.log('SessionController', '停止画面管线失败: $error');
    }
    textureId.value = null;
    preview.value = null;
  }

  /// 建立会话链路；失败只记录，不阻断画面与输入。
  Future<void> _startSession() async {
    try {
      session.value = await gateway.sessionStart(role.value);
    } catch (error, stackTrace) {
      session.value = null;
      RdcLogger.log(
        'SessionController',
        '会话链路建立失败',
        level: LogLevel.error,
        error: error,
        stackTrace: stackTrace,
      );
    }
  }

  /// 结束会话：先排空输入并注销纹理，再发 bye 完成两端收尾。
  ///
  /// 参数 [reason]：写入 bye 报文，便于对端审计。
  /// 返回：收尾报告（含 bye 往返、服务残留、句柄差值与端口释放）；
  /// 报告为空表示 Rust 侧未返回（调用失败，失败原因已记录日志）。
  Future<SessionTeardownInfo?> endSession({String reason = '用户断开'}) async {
    _statusTimer?.cancel();
    _statusTimer = null;
    // 顺序要求：输入先排空、纹理先注销，Rust 才停采集与解码。
    await _stopInput();
    try {
      await gateway.unregisterControlTexture();
    } catch (error) {
      RdcLogger.log('SessionController', '注销纹理失败: $error');
    }
    SessionTeardownInfo? report;
    try {
      report = await gateway.sessionDisconnect(reason);
    } catch (error, stackTrace) {
      RdcLogger.log(
        'SessionController',
        '会话断开失败',
        level: LogLevel.error,
        error: error,
        stackTrace: stackTrace,
      );
    }
    textureId.value = null;
    preview.value = null;
    session.value = null;
    // 兜底收尾：bye 失败时也不能把服务留在后台。
    await _teardown();
    return report;
  }

  /// 启动输入通道；失败时只记录错误，不影响画面。
  Future<void> _startInput(ControlPreviewInfo info) async {
    try {
      inputStats.value = await gateway.startInput(
        width: info.width,
        height: info.height,
      );
      _inputRunning = true;
      inputError.value = null;
    } catch (error, stackTrace) {
      _inputRunning = false;
      inputError.value = '$error';
      RdcLogger.log(
        'SessionController',
        '输入通道启动失败',
        level: LogLevel.error,
        error: error,
        stackTrace: stackTrace,
      );
    }
  }

  /// 停止输入通道：先排空串行链，再让 Rust 抬起所有键/钮并返回收尾统计。
  Future<void> _stopInput() async {
    if (!_inputRunning) return;
    _inputRunning = false;
    try {
      await _inputChain;
    } catch (_) {
      // 链内已记录，收尾继续。
    }
    try {
      inputStats.value = await gateway.stopInput();
    } catch (error) {
      RdcLogger.log('SessionController', '输入通道停止失败: $error');
    }
  }

  /// 推送一批输入事件（指针/键盘）。
  ///
  /// 参数 [events]：已映射到桌面物理像素的事件；空数组直接忽略。
  /// 顺序保证：所有批次串行提交，不会因 FFI 并发而乱序。
  void sendInputEvents(List<InputEventSpec> events) {
    if (!_inputRunning || events.isEmpty) return;
    final batch = List<InputEventSpec>.unmodifiable(events);
    _inputChain = _inputChain.then((_) async {
      try {
        inputStats.value = await gateway.pushInput(batch);
      } catch (error, stackTrace) {
        inputError.value = '$error';
        RdcLogger.log(
          'SessionController',
          '输入注入失败',
          level: LogLevel.error,
          error: error,
          stackTrace: stackTrace,
        );
      }
    });
  }

  /// 冲刷挂起的移动（指针抬起后调用，避免末点丢失）。
  void flushInputEvents() {
    if (!_inputRunning) return;
    _inputChain = _inputChain.then((_) async {
      try {
        inputStats.value = await gateway.flushInput();
      } catch (error) {
        RdcLogger.log('SessionController', '输入冲刷失败: $error');
      }
    });
  }

  /// 刷新一次指标。
  Future<void> _refreshStatus() async {
    try {
      preview.value = await gateway.controlPreviewStatus();
    } catch (error) {
      RdcLogger.log('SessionController', '读取画面状态失败: $error');
    }
    if (!_inputRunning) return;
    try {
      inputStats.value = await gateway.inputStats();
    } catch (error) {
      RdcLogger.log('SessionController', '读取输入统计失败: $error');
    }
  }

  /// 切换全屏。
  void toggleFullscreen() {
    fullscreen.value = !fullscreen.value;
  }

  /// 启动剪贴板同步（回环两端）。
  Future<ClipboardSyncInfo> startClipboard() => gateway.startClipboard();

  /// 本机剪贴板外发到远端。
  Future<ClipboardSyncInfo> clipboardSend() => gateway.clipboardSend();

  /// 远端内容取回本机剪贴板。
  Future<ClipboardSyncInfo> clipboardPull() => gateway.clipboardPull();

  /// 模拟对端修改剪贴板内容。
  Future<ClipboardSyncInfo> clipboardSetRemote(String text) =>
      gateway.clipboardSetRemote(text);

  /// 远端缓冲区文本（预览）。
  Future<String> clipboardRemoteText() => gateway.clipboardRemoteText();

  /// 本机剪贴板文本（预览）。
  Future<String> clipboardLocalText() => gateway.clipboardLocalText();

  /// 传输文件到接收目录；[interruptAfterChunks] 大于 0 时模拟断线续传。
  Future<FileTransferInfo> sendFile(
    String path, {
    int interruptAfterChunks = 0,
  }) {
    return gateway.transferFile(
      path,
      interruptAfterChunks: interruptAfterChunks,
      // 续传演示用 1 MiB 小块，正常传输走默认 4 MiB。
      chunkSize: interruptAfterChunks > 0 ? 1048576 : 0,
    );
  }

  /// 释放监听器与管线。
  void dispose() {
    _statusTimer?.cancel();
    _statusTimer = null;
    // 只做资源回收，不再写通知器（通知器随后即被销毁）。
    unawaited(_teardown());
    role.dispose();
    canInjectInput.dispose();
    canViewWall.dispose();
    fullscreen.dispose();
    errorMessage.dispose();
    textureId.dispose();
    preview.dispose();
    previewError.dispose();
    inputStats.dispose();
    inputError.dispose();
    session.dispose();
  }

  /// 无通知器写入的管线回收。
  Future<void> _teardown() async {
    try {
      await gateway.unregisterControlTexture();
    } catch (_) {
      // 窗口关闭路径，忽略失败。
    }
    try {
      await gateway.sessionDisconnect('窗口关闭');
    } catch (_) {
      // 窗口关闭路径，忽略失败。
    }
    try {
      await gateway.stopInput();
    } catch (_) {
      // 窗口关闭路径，忽略失败。
    }
    try {
      await gateway.stopClipboard();
    } catch (_) {
      // 窗口关闭路径，忽略失败。
    }
    try {
      await gateway.stopControlPreview();
    } catch (_) {
      // 窗口关闭路径，忽略失败。
    }
  }
}
