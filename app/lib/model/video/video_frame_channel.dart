// =====** 视频帧通道（Dart 侧接线点） =====**
//
// 层级：model / 视频
// 职责：传输层（未来的 QUIC 视频流）或离线测试源把 Annex-B 字节推进来；
//      `SessionController` 订阅后切分并交给 `AndroidVideoDecoder`。
//
// 为什么单独抽一层：解码器插件只认“一段 Annex-B 字节”，不关心来源；
// 把来源与解码器解耦后，宿主单测可以直接喂内嵌码流，真机再替换为网络源，
// 不需要改控制器与页面。

import 'dart:async';
import 'dart:typed_data';

/// 视频流元数据（来自流的协商/首包，不是从 SPS 解析）。
class VideoStreamInfo {
  /// 创建元数据。
  const VideoStreamInfo({
    required this.width,
    required this.height,
    this.fps = 60,
  });

  /// 编码宽。
  final int width;

  /// 编码高。
  final int height;

  /// 目标帧率。
  final int fps;
}

/// 单生产者、多消费者的视频字节通道。
class VideoFrameChannel {
  final StreamController<Uint8List> _controller =
      StreamController<Uint8List>.broadcast();

  /// 当前流元数据；`null` 表示传输层尚未告知尺寸。
  ///
  /// 解码器 `configure` 用；可在首帧前后任意时刻赋值。
  VideoStreamInfo? info;

  /// 订阅 Annex-B 字节流。
  Stream<Uint8List> get frames => _controller.stream;

  /// 推入一段 Annex-B 字节（可含多个访问单元）。
  void push(Uint8List annexB) {
    if (annexB.isEmpty || _controller.isClosed) {
      return;
    }
    _controller.add(annexB);
  }

  /// 关闭通道（释放订阅者）。
  Future<void> close() async {
    if (!_controller.isClosed) {
      await _controller.close();
    }
  }
}
