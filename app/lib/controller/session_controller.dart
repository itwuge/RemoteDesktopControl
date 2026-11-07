// ============================================================
// 文件用途：远程控制页控制器（角色权限 + 真实画面管线 + 全屏与工具条状态）
//
// 层级：controller
// 被谁调用：view/pages/session_page.dart
// 边界：画面走外部纹理（Texture），不逐帧转 Image；控制器只透传纹理 ID 与状态
// ============================================================

import 'dart:async';
import 'dart:io' show Platform;

import 'package:flutter/foundation.dart';

import '../core/utils/logger.dart';
import '../model/gateway/rust_gateway.dart';
import '../model/gateway/rust_types.dart';
import '../model/input/text_input_bridge.dart';
import '../model/video/android_video_decoder.dart';
import '../model/video/annexb.dart';
import '../model/video/video_frame_channel.dart';

/// 远程控制状态与行为。
///
/// 平台分叉原则：会话/输入/收尾骨架只有一份；Android 只替换「画面来源」
/// （MediaCodec 外部纹理 + Annex-B 通道），Windows/Linux 继续走 Rust 回环管线。
class SessionController {
  /// 创建控制器。
  ///
  /// [gateway] 由 main.dart 注入。
  /// [decoderFactory] / [videoChannel] / [androidMode] 只用于宿主单测注入；
  /// 生产环境保持 `null`，由平台与默认实现决定。
  SessionController({
    required this.gateway,
    AndroidVideoDecoder Function()? decoderFactory,
    VideoFrameChannel? videoChannel,
    bool? androidMode,
  }) : _decoderFactory = decoderFactory ?? AndroidVideoDecoder.new,
       videoChannel = videoChannel ?? VideoFrameChannel(),
       _android = androidMode ?? Platform.isAndroid;

  /// FFI 网关。
  final RustGateway gateway;

  /// 视频字节通道：传输层或离线测试源推入 Annex-B。
  final VideoFrameChannel videoChannel;

  /// 是否运行 Android 画面分支。
  final bool _android;

  /// 解码器工厂（每个会话一个实例）。
  final AndroidVideoDecoder Function() _decoderFactory;

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

  /// 最新一帧 RGBA 画面（Linux 降级通道）。
  ///
  /// Windows 外部纹理路径下恒为 `null`，由 `Texture` 直接消费共享句柄。
  final ValueNotifier<ControlFrameInfo?> frame = ValueNotifier(null);

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

  /// RGBA 帧轮询定时器（仅 Linux 降级通道启用）。
  Timer? _frameTimer;

  /// 是否有一次帧请求在途（避免慢帧叠加成队列）。
  bool _frameInflight = false;

  /// 已显示的帧序号（同帧不重复解码）。
  int _lastFrameId = 0;

  /// 是否正在启动（防止重复进入）。
  bool _starting = false;

  // =====** Android 视频分支状态 =====**

  /// 当前会话的 Android 解码器；`null` 表示未接通。
  AndroidVideoDecoder? _decoder;

  /// 视频通道订阅。
  StreamSubscription<Uint8List>? _videoSubscription;

  /// 视频推入串行链：保证 Annex-B 单元按切分顺序抵达平台通道。
  Future<void> _videoChain = Future<void>.value();

  /// Annex-B 访问单元切分器（状态：等待配对的 SPS/PPS）。
  final AnnexBFrameParser _annexBParser = AnnexBFrameParser();

  /// 首个关键帧门控（IDR 前不向解码器投递）。
  final AnnexBGate _annexBGate = AnnexBGate();

  /// 已下发给解码器的尺寸，避免每秒重复 `configure`。
  int _configuredWidth = 0;

  /// 已下发的视频高。
  int _configuredHeight = 0;

  /// 已下发的视频帧率。
  int _configuredFps = 60;

  /// 是否已尝试启动输入（Android 尺寸未就绪时延后，避免每秒重试）。
  bool _inputStartAttempted = false;

  /// 远端视频轮询定时器（8 ms，约一帧一次）。
  Timer? _videoPollTimer;

  /// 防止上一轮 `pollVideoFrame` 未返回时重入。
  bool _videoPollInflight = false;

  /// 已应用到解码器的配置版本；元数据变化时重建。
  int _appliedConfigId = -1;

  /// 远端文本输入客户端；Android 输入法经它进入统一输入链。
  RemoteTextInput? _textInput;

  /// 协商结果是否含文本能力位（`TEXT_INPUT = bit13`）。
  bool _textInputEnabled = false;

  /// 当前可用的远端文本输入客户端（单测与页面入口使用）。
  RemoteTextInput? get textInput => _textInput;

  /// 是否允许发送文本事件（能力位已协商且输入通道已运行）。
  bool get canSendText => _textInputEnabled && _inputRunning;

  /// 等待视频推入链排空（暂停/收尾与单测使用）。
  Future<void> get videoIdle => _videoChain;

  /// 等待输入串行链排空（收尾与单测使用）。
  Future<void> get inputIdle => _inputChain;

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
    if (_starting || textureId.value != null || frame.value != null) return;
    _starting = true;
    previewError.value = null;
    try {
      // 平台分叉只发生在「画面来源」这一层；会话、输入与轮询骨架共用。
      if (_android) {
        await _startAndroidVideo();
      } else {
        await _startDesktopPreview();
      }
      if (previewError.value != null &&
          textureId.value == null &&
          frame.value == null) {
        return;
      }
      // 画面接通后建立会话链路：bye 握手与资源清理都挂在会话生命周期上。
      await _startSession();
      final info = preview.value;
      // 画面接通后才启动输入通道：没有画面就没有可靠的坐标映射。
      // Android 首帧前的尺寸可能为 0，交给 `_refreshStatus` 在拿到尺寸后再启动。
      if (info != null) {
        await _startInput(info);
      }
      if (_android) {
        _ensureTextInput();
      }
      _statusTimer?.cancel();
      _statusTimer = Timer.periodic(
        const Duration(seconds: 1),
        (_) => _refreshStatus(),
      );
    } catch (error, stackTrace) {
      previewError.value = '$error';
      if (_android) {
        await _stopAndroidVideo();
      }
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

  /// Windows/Linux：Rust 采集编码 → 共享纹理（Windows）或 RGBA 帧（Linux）。
  Future<void> _startDesktopPreview() async {
    final info = await gateway.startControlPreview();
    final id = await gateway.registerControlTexture(info);
    preview.value = info;
    // Linux 无外部纹理，Rust 端以 `rgba-frame` 标明走降级通道。
    final rgbaFallback = id == null && info.texturePath == 'rgba-frame';
    RdcLogger.log(
      'SessionController',
      '画面通道 textureId=$id path=${info.texturePath} '
          '${info.width}x${info.height} backend=${info.backend}',
    );
    if (id != null) {
      textureId.value = id;
    } else if (rgbaFallback) {
      _lastFrameId = 0;
      _startFramePolling();
    } else {
      previewError.value = '外部纹理注册失败（非 Windows 或 runner 未挂载通道）';
      await gateway.stopControlPreview();
    }
  }

  /// Android：创建 MediaCodec 外部纹理，并订阅 Annex-B 视频通道。
  ///
  /// 返回：无返回值；`textureId` 非空后页面用现有 `Texture` 组件消费。
  Future<void> _startAndroidVideo() async {
    final decoder = _decoderFactory();
    _decoder = decoder;
    final id = await decoder.create();
    textureId.value = id;
    final stream = videoChannel.info;
    if (stream != null) {
      await _configureAndroidDecoder(stream);
    }
    final stats = await decoder.stats();
    _updateAndroidPreview(stats);
    _annexBParser.reset();
    _annexBGate.reset();
    _videoSubscription = videoChannel.frames.listen(
      _onVideoBytes,
      onError: (Object error, StackTrace stackTrace) {
        RdcLogger.log(
          'SessionController',
          '视频通道错误',
          level: LogLevel.error,
          error: error,
          stackTrace: stackTrace,
        );
      },
    );
    // 桥接层把网络视频消息重组成帧，Flutter 侧按帧轮询拉取。
    _startVideoPolling();
    RdcLogger.log(
      'SessionController',
      'Android 画面通道 textureId=$id '
          '${stream?.width ?? 0}x${stream?.height ?? 0}',
    );
  }

  /// 仅在流元数据变化时下发 `configure`（不重建 textureId）。
  ///
  /// 参数 [force]：配置版本变化但尺寸相同时（如色彩空间变化）仍要重配。
  Future<void> _configureAndroidDecoder(
    VideoStreamInfo stream, {
    bool force = false,
  }) async {
    final decoder = _decoder;
    if (decoder == null || stream.width <= 0 || stream.height <= 0) {
      return;
    }
    if (!force &&
        stream.width == _configuredWidth &&
        stream.height == _configuredHeight &&
        stream.fps == _configuredFps) {
      return;
    }
    await decoder.configure(
      width: stream.width,
      height: stream.height,
      fps: stream.fps,
    );
    _configuredWidth = stream.width;
    _configuredHeight = stream.height;
    _configuredFps = stream.fps;
  }

  /// 启动远端视频轮询（8 ms）；桥接层没有新帧时返回 `null`，不占 CPU。
  void _startVideoPolling() {
    _videoPollTimer?.cancel();
    _videoPollTimer = Timer.periodic(
      const Duration(milliseconds: 8),
      (_) => unawaited(pollRemoteVideoNow()),
    );
  }

  /// 从 Rust 桥拉取一帧并送入既有视频通道。
  ///
  /// 元数据 `configId` 变化时重建解码器、重置切分与 IDR 门控；
  /// 队列为空时直接返回，不产生额外 FFI 压力（8 ms 一次）。
  Future<void> pollRemoteVideoNow() async {
    if (_videoPollInflight || _decoder == null) {
      return;
    }
    _videoPollInflight = true;
    try {
      final frame = await gateway.pollVideoFrame();
      if (frame == null) {
        return;
      }
      if (frame.config.configId != _appliedConfigId) {
        await _applyRemoteConfig(frame.config);
      }
      videoChannel.push(frame.data);
    } catch (error) {
      RdcLogger.log('SessionController', '拉取远端视频帧失败: $error');
    } finally {
      _videoPollInflight = false;
    }
  }

  /// 应用新的远端元数据：更新流信息、重建解码器并等待新的 IDR。
  Future<void> _applyRemoteConfig(VideoConfigInfo config) async {
    _appliedConfigId = config.configId;
    videoChannel.info = VideoStreamInfo(
      width: config.width,
      height: config.height,
      fps: config.fps,
    );
    _annexBParser.reset();
    _annexBGate.reset();
    final info = VideoStreamInfo(
      width: config.width,
      height: config.height,
      fps: config.fps,
    );
    // 分辨率/色彩变化必须重新 configure；同配置重复到达直接跳过。
    await _stopInput();
    await _configureAndroidDecoder(info, force: true);
    preview.value = ControlPreviewInfo(
      running: true,
      backend: 'android',
      encoder: 'remote',
      decoder: config.codec,
      texturePath: 'surface-producer',
      decodePath: 'mediacodec-surface',
      width: config.width,
      height: config.height,
      fps: config.fps.toDouble(),
      capturedFrames: 0,
      encodedFrames: 0,
      decodedFrames: 0,
      droppedFrames: 0,
      encodedBytes: 0,
      bitrateKbps: 0,
      latencyLastUs: 0,
      latencyP50Us: 0,
      latencyP99Us: 0,
      readbackCount: 0,
      shareHandleA: 0,
      shareHandleB: 0,
      eventA: 0,
      eventB: 0,
      lastError: null,
    );
    _inputStartAttempted = false;
    await _startInput(preview.value!);
    _ensureTextInput();
  }

  /// 收到一段 Annex-B 字节：切分 → IDR 门控 → 串行推给平台解码器。
  void _onVideoBytes(Uint8List bytes) {
    final decoder = _decoder;
    if (decoder == null) {
      return;
    }
    for (final unit in _annexBParser.push(bytes)) {
      if (!_annexBGate.accept(unit)) {
        continue;
      }
      _videoChain = _videoChain.then((_) async {
        try {
          await decoder.push(unit.data);
        } catch (error, stackTrace) {
          RdcLogger.log(
            'SessionController',
            '视频帧推入失败',
            level: LogLevel.error,
            error: error,
            stackTrace: stackTrace,
          );
        }
      });
    }
  }

  /// 把解码器统计映射成页面需要的画面快照。
  void _updateAndroidPreview(VideoDecodeStats stats) {
    preview.value = ControlPreviewInfo(
      running: stats.textureId >= 0,
      backend: 'android',
      encoder: 'remote',
      decoder: stats.codec.isEmpty ? 'mediacodec' : stats.codec,
      texturePath: 'surface-producer',
      decodePath: 'mediacodec-surface',
      width: stats.width,
      height: stats.height,
      fps: stats.fps,
      capturedFrames: 0,
      encodedFrames: 0,
      decodedFrames: stats.decoded,
      droppedFrames: stats.dropped,
      encodedBytes: 0,
      bitrateKbps: 0,
      latencyLastUs: stats.latencyP50Us,
      latencyP50Us: stats.latencyP50Us,
      latencyP99Us: stats.latencyP99Us,
      readbackCount: 0,
      shareHandleA: 0,
      shareHandleB: 0,
      eventA: 0,
      eventB: 0,
      lastError: stats.lastError.isEmpty ? null : stats.lastError,
    );
  }

  /// 启动 RGBA 帧轮询（约 30Hz，按需丢弃重复帧）。
  ///
  /// 降级说明：Linux 无外部纹理，逐帧 `decodeImageFromPixels` 的 CPU 成本与帧率成正比，
  /// 软件渲染下实际帧率由 Rust 解码端决定，这里不做插帧。
  void _startFramePolling() {
    _frameTimer?.cancel();
    _frameInflight = false;
    _frameTimer = Timer.periodic(
      const Duration(milliseconds: 33),
      (_) => unawaited(_pollFrame()),
    );
  }

  /// 拉取最新一帧；沿用上一帧直到出现新帧号。
  Future<void> _pollFrame() async {
    if (_frameInflight) return;
    _frameInflight = true;
    try {
      final next = await gateway.controlFrame();
      if (next == null || next.frameId <= _lastFrameId) return;
      final first = _lastFrameId == 0;
      _lastFrameId = next.frameId;
      frame.value = next;
      if (first) {
        RdcLogger.log(
          'SessionController',
          '首帧 #${next.frameId} ${next.width}x${next.height} '
              '${next.bytes.length}B 延迟 ${next.latencyUs}us',
        );
      }
    } catch (error) {
      RdcLogger.log('SessionController', '读取 RGBA 帧失败: $error');
    } finally {
      _frameInflight = false;
    }
  }

  /// 停止 RGBA 帧轮询并清空当前帧。
  void _stopFramePolling() {
    _frameTimer?.cancel();
    _frameTimer = null;
    _frameInflight = false;
    _lastFrameId = 0;
    frame.value = null;
  }

  /// 停止画面管线：先注销纹理，再停止输入与 Rust（避免句柄提前释放）。
  Future<void> stopPreview() async {
    _statusTimer?.cancel();
    _statusTimer = null;
    _stopFramePolling();
    _detachTextInput();
    if (_android) {
      await _stopAndroidVideo();
    } else {
      try {
        await gateway.unregisterControlTexture();
      } catch (error) {
        RdcLogger.log('SessionController', '注销纹理失败: $error');
      }
    }
    await _stopInput();
    if (!_android) {
      try {
        await gateway.stopControlPreview();
      } catch (error) {
        RdcLogger.log('SessionController', '停止画面管线失败: $error');
      }
    }
    textureId.value = null;
    preview.value = null;
  }

  /// 释放 Android 解码器、视频订阅与切分状态（幂等）。
  Future<void> _stopAndroidVideo() async {
    _videoPollTimer?.cancel();
    _videoPollTimer = null;
    _videoPollInflight = false;
    _appliedConfigId = -1;
    await _videoSubscription?.cancel();
    _videoSubscription = null;
    _annexBParser.reset();
    _annexBGate.reset();
    try {
      await _videoChain;
    } catch (_) {
      // 链内已记录，收尾继续。
    }
    final decoder = _decoder;
    _decoder = null;
    _configuredWidth = 0;
    _configuredHeight = 0;
    _configuredFps = 60;
    if (decoder != null) {
      try {
        await decoder.dispose();
      } catch (error) {
        RdcLogger.log('SessionController', '释放 Android 解码器失败: $error');
      }
    }
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
      return;
    }
    // 握手完成后回填协商位；未握手时 Rust 侧返回 0，文本保持关闭。
    await _refreshNegotiatedCapabilities();
  }

  /// 从 Rust 桥读取协商结果并写入文本门控。
  Future<void> _refreshNegotiatedCapabilities() async {
    try {
      setNegotiatedCapabilities(await gateway.sessionNegotiatedCapabilities());
    } catch (error) {
      RdcLogger.log('SessionController', '读取协商能力位失败: $error');
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
    _stopFramePolling();
    _detachTextInput();
    // 顺序要求：输入先排空、纹理先注销，Rust 才停采集与解码。
    await _stopInput();
    if (_android) {
      await _stopAndroidVideo();
    } else {
      try {
        await gateway.unregisterControlTexture();
      } catch (error) {
        RdcLogger.log('SessionController', '注销纹理失败: $error');
      }
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
    // Android 首帧前尺寸可能为 0：等 `_refreshStatus` 拿到有效尺寸后再启动。
    if (_android && (info.width <= 0 || info.height <= 0)) {
      return;
    }
    _inputStartAttempted = true;
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
    _inputStartAttempted = false;
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

  // =====** 文本/IME 接线 =====**

  /// 写入协商结果的能力位；bit13 未置位时立即停发文本。
  ///
  /// 参数 [bits]：双方 `Capabilities::negotiate` 后的位图（Dart 只关心 bit13）。
  void setNegotiatedCapabilities(int bits) {
    final enabled = (bits & kTextInputCapabilityBit) != 0;
    if (!enabled) {
      _detachTextInput();
    }
    _textInputEnabled = enabled;
    RdcLogger.log(
      'SessionController',
      '文本能力位 ${enabled ? '已协商' : '未协商'}（bits=0x${bits.toRadixString(16)}）',
    );
  }

  /// 创建远端文本框（幂等）；只接线，不自动弹软键盘。
  void _ensureTextInput() {
    if (!_android) {
      return;
    }
    _textInput ??= RemoteTextInput(onEdit: _onTextEdit);
  }

  /// 显示软键盘（页面入口调用）；能力位未协商时静默不弹。
  void showTextInput() {
    if (!canSendText) {
      return;
    }
    _ensureTextInput();
    _textInput?.attach();
  }

  /// 收起软键盘（不发编辑）。
  void hideTextInput() {
    _textInput?.detach();
  }

  /// 清空本地编辑缓存（远端窗口切换时调用）。
  void resetTextInput() {
    _textInput?.reset();
  }

  /// 断开输入法并丢弃客户端；会话结束时调用。
  void _detachTextInput() {
    _textInput?.detach();
    _textInput = null;
  }

  /// 输入法分段事件 → 统一输入链。
  ///
  /// 能力位为 0 时**不发送**：这时保留本地组合/候选，远端退回扫描码键盘。
  void _onTextEdit(TextEdit edit) {
    if (edit.isNoop || !canSendText) {
      return;
    }
    sendInputEvents([
      InputEventSpec.text(text: edit.insert, deleteUnits: edit.deleteUnits),
    ]);
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
    if (_android) {
      await _refreshAndroidVideo();
    } else {
      try {
        preview.value = await gateway.controlPreviewStatus();
      } catch (error) {
        RdcLogger.log('SessionController', '读取画面状态失败: $error');
      }
    }
    // 握手可能晚于会话建立：每秒回填一次，直到能力位稳定。
    await _refreshNegotiatedCapabilities();
    if (!_inputRunning) return;
    try {
      inputStats.value = await gateway.inputStats();
    } catch (error) {
      RdcLogger.log('SessionController', '读取输入统计失败: $error');
    }
  }

  /// Android 指标来自解码器，而不是 Rust 回环采集端。
  Future<void> _refreshAndroidVideo() async {
    final decoder = _decoder;
    if (decoder == null) {
      return;
    }
    try {
      final stats = await decoder.stats();
      _updateAndroidPreview(stats);
      // 首帧 SPS 解出前宽度为 0；拿到有效尺寸后补启动输入与文本接线。
      if (!_inputRunning &&
          !_inputStartAttempted &&
          stats.width > 0 &&
          stats.height > 0) {
        final info = preview.value;
        if (info != null) {
          await _startInput(info);
          _ensureTextInput();
        }
      }
    } catch (error) {
      RdcLogger.log('SessionController', '读取解码统计失败: $error');
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
    _frameTimer?.cancel();
    _frameTimer = null;
    // 只做资源回收，不再写通知器（通知器随后即被销毁）。
    unawaited(_teardown());
    role.dispose();
    canInjectInput.dispose();
    canViewWall.dispose();
    fullscreen.dispose();
    errorMessage.dispose();
    textureId.dispose();
    preview.dispose();
    frame.dispose();
    previewError.dispose();
    inputStats.dispose();
    inputError.dispose();
    session.dispose();
  }

  /// 无通知器写入的管线回收。
  Future<void> _teardown() async {
    _textInput?.detach();
    _textInput = null;
    if (_android) {
      await _stopAndroidVideo();
    } else {
      try {
        await gateway.unregisterControlTexture();
      } catch (_) {
        // 窗口关闭路径，忽略失败。
      }
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
    if (!_android) {
      try {
        await gateway.stopControlPreview();
      } catch (_) {
        // 窗口关闭路径，忽略失败。
      }
    }
  }
}
