// =====** 阶段 3：SessionController Android 接线单测 =====**
//
// 判据（见 logs/TESTPLAN-s9-android.md C2–C4b）：
// 1. 创建 textureId 并写入控制器；
// 2. 离线 Annex-B 码流经视频通道切分后按序推给解码器；
// 3. 首个 IDR 之前的非关键帧不投递（IDR 前不启动解码）；
// 4. 能力位为 0 时输入法编辑不进入输入链，置位后才发 kind=text。
//
// 这些用例全在宿主（Windows/macOS/Linux + flutter test）跑，不需要真机；
// MediaCodec 的实际出帧、延迟与花屏仍属设备未验证项。

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rdc/controller/session_controller.dart';
import 'package:rdc/model/gateway/rust_gateway.dart';
import 'package:rdc/model/gateway/rust_types.dart';
import 'package:rdc/model/input/text_input_bridge.dart';
import 'package:rdc/model/video/android_video_decoder.dart';
import 'package:rdc/model/video/annexb.dart';
import 'package:rdc/model/video/video_frame_channel.dart';

/// 只替身会话/输入接口的网关；不触达 FFI。
class _FakeGateway extends RustGateway {
  final List<List<InputEventSpec>> pushes = <List<InputEventSpec>>[];
  int startInputCalls = 0;

  @override
  Future<SessionStatusInfo> sessionStart(String role) async {
    return SessionStatusInfo(
      running: true,
      sessionId: 'test-session',
      role: role,
      localPort: 1,
      peerPort: 2,
      elapsedMs: 0,
      byeSent: false,
      byeAcked: false,
    );
  }

  @override
  Future<InputStatsInfo> startInput({
    required int width,
    required int height,
  }) async {
    startInputCalls += 1;
    return const InputStatsInfo.empty();
  }

  @override
  Future<InputStatsInfo> pushInput(List<InputEventSpec> events) async {
    pushes.add(List<InputEventSpec>.of(events));
    return const InputStatsInfo.empty();
  }

  @override
  Future<InputStatsInfo> flushInput() async => const InputStatsInfo.empty();

  @override
  Future<InputStatsInfo> stopInput() async => const InputStatsInfo.empty();

  @override
  Future<InputStatsInfo> inputStats() async => const InputStatsInfo.empty();
}

/// 一个 NAL（4 字节起始码 + 类型 + 负载）。
Uint8List _nal(int type, [List<int> payload = const <int>[]]) {
  return Uint8List.fromList(<int>[0, 0, 0, 1, type, ...payload]);
}

/// 非关键帧（P 片，type 1）。
Uint8List _pFrame() => _nal(1, const <int>[0x9A, 0x22]);

/// SPS + PPS + IDR 组成的访问单元。
Uint8List _idrAccessUnit() {
  final sps = _nal(7, const <int>[0x42, 0x00, 0x1E]);
  final pps = _nal(8, const <int>[0xCE, 0x3C, 0x80]);
  final idr = _nal(5, const <int>[0x88, 0x84]);
  return Uint8List.fromList(<int>[...sps, ...pps, ...idr]);
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  const channel = MethodChannel(AndroidVideoDecoder.channelName);
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  final pushed = <Uint8List>[];

  setUp(() {
    pushed.clear();
    messenger.setMockMethodCallHandler(channel, (call) async {
      switch (call.method) {
        case 'create':
          return <String, dynamic>{'textureId': 7};
        case 'configure':
          return null;
        case 'push':
          final args = call.arguments as Map<dynamic, dynamic>;
          pushed.add(args['data'] as Uint8List);
          return null;
        case 'stats':
          return <String, dynamic>{
            'textureId': 7,
            'decoded': 12,
            'dropped': 1,
            'latencyP50Us': 800,
            'latencyP99Us': 2400,
            'fps': 30.0,
            'width': 640,
            'height': 360,
            'codec': 'c2.android.avc.decoder',
            'paused': false,
            'surfaceRebuilds': 0,
            'lastError': '',
          };
        case 'pause':
        case 'resume':
        case 'dispose':
          return null;
        default:
          throw MissingPluginException('未实现的测试方法 ${call.method}');
      }
    });
  });

  tearDown(() {
    messenger.setMockMethodCallHandler(channel, null);
  });

  SessionController buildController(_FakeGateway gateway) {
    final video = VideoFrameChannel()
      ..info = const VideoStreamInfo(width: 640, height: 360, fps: 30);
    return SessionController(
      gateway: gateway,
      androidMode: true,
      videoChannel: video,
      decoderFactory: () => AndroidVideoDecoder(channel: channel),
    );
  }

  test('startPreview 创建外部纹理并在尺寸就绪后启动输入', () async {
    final gateway = _FakeGateway();
    final controller = buildController(gateway);

    await controller.startPreview();

    expect(controller.textureId.value, 7);
    expect(controller.preview.value?.width, 640);
    expect(controller.preview.value?.height, 360);
    expect(controller.preview.value?.texturePath, 'surface-producer');
    expect(controller.textInput, isNotNull);
    expect(gateway.startInputCalls, 1);

    await controller.stopPreview();
    controller.dispose();
  });

  test('离线码流：切分正确，IDR 前的非关键帧不推给解码器', () async {
    final gateway = _FakeGateway();
    final controller = buildController(gateway);
    await controller.startPreview();

    controller.videoChannel.push(_pFrame()); // IDR 之前 → 丢弃
    controller.videoChannel.push(_idrAccessUnit()); // 开门
    controller.videoChannel.push(_pFrame()); // IDR 之后 → 放行
    await Future<void>.delayed(Duration.zero);
    await controller.videoIdle;

    expect(pushed, hasLength(2));
    final first = AnnexBFrameParser().push(pushed.first).single;
    final last = AnnexBFrameParser().push(pushed.last).single;
    expect(first.nalTypes, <int>[7, 8, 5]);
    expect(first.keyFrame, isTrue);
    expect(first.hasParameterSets, isTrue);
    expect(last.nalTypes, <int>[1]);

    await controller.stopPreview();
    controller.dispose();
  });

  test('能力位为 0 时输入法编辑不进入输入链', () async {
    final gateway = _FakeGateway();
    final controller = buildController(gateway);
    await controller.startPreview();

    // 默认未协商：文本客户端已接线，但编辑必须被门控挡住。
    controller.textInput!.submit('你好');
    await controller.inputIdle;

    expect(gateway.pushes, isEmpty);

    await controller.stopPreview();
    controller.dispose();
  });

  test('能力位置位后文本以 kind=text 进入输入链', () async {
    final gateway = _FakeGateway();
    final controller = buildController(gateway);
    await controller.startPreview();

    controller.setNegotiatedCapabilities(kTextInputCapabilityBit);
    controller.textInput!.submit('你好');
    await controller.inputIdle;

    expect(gateway.pushes, hasLength(1));
    final event = gateway.pushes.single.single;
    expect(event.kind, 'text');
    expect(event.text, '你好');
    expect(event.deleteUnits, 0);

    await controller.stopPreview();
    controller.dispose();
  });

  test('协商后被撤销时立即停发文本', () async {
    final gateway = _FakeGateway();
    final controller = buildController(gateway);
    await controller.startPreview();

    final input = controller.textInput!;
    controller.setNegotiatedCapabilities(kTextInputCapabilityBit);
    controller.setNegotiatedCapabilities(0);
    input.submit('late');
    await controller.inputIdle;

    expect(gateway.pushes, isEmpty);

    await controller.stopPreview();
    controller.dispose();
  });
}
