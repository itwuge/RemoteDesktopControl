// =====** Android H.264 解码器（Dart 侧） =====**
//
// 层级：Flutter 模型层 / 视频
// 边界：只负责 MethodChannel 契约与状态缓存；像素在 Kotlin 侧直接渲染到
//       SurfaceTexture，Dart 只拿到 textureId 交给 `Texture` 组件。
//
// 为什么用外部纹理而不是 ImageReader：ImageReader 要把解码结果回读到 CPU
// （YUV 缓冲），每帧都要跨 surface 复制，1080p@30 会额外占用带宽与内存；
// SurfaceTexture 走 GPU 采样，Flutter 合成器直接消费，零回读。
//
// 通道契约与 Kotlin `VideoDecoderPlugin` 一一对应；改任一侧都要同步改另一侧
// 与 test/android_video_decoder_test.dart 的契约测试。

import 'package:flutter/services.dart';

/// 解码统计快照（Kotlin → Dart）。
///
/// 延迟分位是本地“入队 → 渲染”的耗时，不含网络传输；样本窗口约 8 秒 @30fps。
class VideoDecodeStats {
  /// 创建统计快照。
  const VideoDecodeStats({
    required this.textureId,
    required this.decoded,
    required this.dropped,
    required this.latencyP50Us,
    required this.latencyP99Us,
    required this.fps,
    required this.width,
    required this.height,
    required this.codec,
    required this.paused,
    required this.surfaceRebuilds,
    required this.lastError,
  });

  /// 从通道返回值解析；缺失字段用 0/空串兜底（旧插件版本不会崩客户端）。
  factory VideoDecodeStats.fromMap(Map<String, dynamic> map) => VideoDecodeStats(
    textureId: (map['textureId'] as num?)?.toInt() ?? -1,
    decoded: (map['decoded'] as num?)?.toInt() ?? 0,
    dropped: (map['dropped'] as num?)?.toInt() ?? 0,
    latencyP50Us: (map['latencyP50Us'] as num?)?.toInt() ?? 0,
    latencyP99Us: (map['latencyP99Us'] as num?)?.toInt() ?? 0,
    fps: (map['fps'] as num?)?.toDouble() ?? 0,
    width: (map['width'] as num?)?.toInt() ?? 0,
    height: (map['height'] as num?)?.toInt() ?? 0,
    codec: map['codec'] as String? ?? '',
    paused: map['paused'] as bool? ?? false,
    surfaceRebuilds: (map['surfaceRebuilds'] as num?)?.toInt() ?? 0,
    lastError: map['lastError'] as String? ?? '',
  );

  /// Flutter 外部纹理编号。
  final int textureId;

  /// 成功渲染的帧数。
  final int decoded;

  /// 因等待 IDR/队列过深而丢弃的帧数。
  final int dropped;

  /// 本地解码+渲染延迟 p50（微秒）。
  final int latencyP50Us;

  /// 本地解码+渲染延迟 p99（微秒）。
  final int latencyP99Us;

  /// 最近一秒的渲染帧率。
  final double fps;

  /// 当前视频宽（像素）。
  final int width;

  /// 当前视频高（像素）。
  final int height;

  /// 实际解码器名称；未启动时为 `none`。
  final String codec;

  /// 是否处于后台暂停。
  final bool paused;

  /// Surface/MediaCodec 重建次数（前后台切换、分辨率变化）。
  final int surfaceRebuilds;

  /// 最近一次错误（空串表示无）。
  final String lastError;

  /// 延迟（毫秒，便于日志）。
  double get latencyP50Ms => latencyP50Us / 1000.0;

  /// 延迟（毫秒，便于日志）。
  double get latencyP99Ms => latencyP99Us / 1000.0;
}

/// Android 视频解码器通道客户端。
///
/// 一个实例对应一个外部纹理；生命周期由调用方（会话控制器）管理。
class AndroidVideoDecoder {
  /// 创建客户端。
  ///
  /// # Arguments
  /// * `channel` - 可注入的通道（单测用 mock）；默认 `rdc/video_decoder`
  AndroidVideoDecoder({MethodChannel? channel})
    : _channel = channel ?? const MethodChannel(channelName);

  /// 与 Kotlin `VideoDecoderPlugin.CHANNEL` 必须一致。
  static const String channelName = 'rdc/video_decoder';

  final MethodChannel _channel;
  int? _textureId;

  /// 已创建的纹理编号；未创建时为 `null`。
  int? get textureId => _textureId;

  /// 创建外部纹理。
  ///
  /// # Arguments
  /// * `width`/`height` - 初始视频尺寸（未知时传 0，等首帧 SPS 后再 `configure`）
  ///
  /// # Returns
  /// Flutter `Texture` 组件要用的 textureId。
  ///
  /// # Errors
  /// 插件未注册或参数非法时抛出 [PlatformException]。
  Future<int> create({int width = 0, int height = 0}) async {
    final reply = await _channel.invokeMapMethod<String, dynamic>('create', {
      'width': width,
      'height': height,
    });
    final id = (reply?['textureId'] as num?)?.toInt();
    if (id == null || id < 0) {
      throw PlatformException(
        code: 'bad_texture',
        message: '解码插件未返回 textureId: $reply',
      );
    }
    _textureId = id;
    return id;
  }

  /// 更新视频尺寸/帧率（远端分辨率变化时调用，不重建 textureId）。
  ///
  /// 尺寸为 0 时忽略，避免把有效尺寸覆盖成 0。
  Future<void> configure({required int width, required int height, int fps = 60}) async {
    if (width <= 0 || height <= 0) {
      return;
    }
    await _channel.invokeMethod<void>('configure', {
      'width': width,
      'height': height,
      'fps': fps,
    });
  }

  /// 推入一段 Annex-B H.264 码流。
  ///
  /// 注意：字节经由平台通道传给 Kotlin，有一次内存拷贝（不属于 GPU 回读），
  /// 1080p 码流每帧只有几十 KB，实测影响远小于 ImageReader 的整帧 YUV 复制。
  Future<void> push(Uint8List data) async {
    await _channel.invokeMethod<void>('push', {'data': data});
  }

  /// 读取统计快照。
  Future<VideoDecodeStats> stats() async {
    final reply = await _channel.invokeMapMethod<String, dynamic>('stats');
    return VideoDecodeStats.fromMap(reply ?? const <String, dynamic>{});
  }

  /// 暂停解码（后台）。
  Future<void> pause() => _channel.invokeMethod<void>('pause');

  /// 恢复解码（前台，重建 Surface，等待下一个 IDR）。
  Future<void> resume() => _channel.invokeMethod<void>('resume');

  /// 释放纹理与解码器；重复调用安全。
  Future<void> dispose() async {
    if (_textureId == null) {
      return;
    }
    try {
      await _channel.invokeMethod<void>('dispose');
    } finally {
      _textureId = null;
    }
  }
}
