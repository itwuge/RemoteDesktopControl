// 解码器通道契约测试。
//
// 不启动 Android 解码器，只验证 Dart 侧与 Kotlin `VideoDecoderPlugin` 的
// 方法名、参数结构与返回值解析；任何一侧改字段都会让这里失败。

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:rdc/model/video/android_video_decoder.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  const channel = MethodChannel(AndroidVideoDecoder.channelName);
  final calls = <MethodCall>[];
  late AndroidVideoDecoder decoder;

  setUp(() {
    calls.clear();
    decoder = AndroidVideoDecoder();
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, (call) async {
          calls.add(call);
          switch (call.method) {
            case 'create':
              return {'textureId': 7, 'width': 1280, 'height': 720};
            case 'stats':
              return {
                'textureId': 7,
                'decoded': 120,
                'dropped': 3,
                'latencyP50Us': 9000,
                'latencyP99Us': 21000,
                'fps': 29.5,
                'width': 1280,
                'height': 720,
                'codec': 'c2.qti.avc.decoder',
                'paused': false,
                'surfaceRebuilds': 1,
                'lastError': '',
              };
            default:
              return null;
          }
        });
  });

  tearDown(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, null);
  });

  test('create 传递尺寸并返回 textureId', () async {
    final id = await decoder.create(width: 1280, height: 720);
    expect(id, 7);
    expect(decoder.textureId, 7);
    final call = calls.single;
    expect(call.method, 'create');
    expect(call.arguments, {'width': 1280, 'height': 720});
  });

  test('create 返回缺失 textureId 时抛出明确错误', () async {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, (call) async => <String, dynamic>{});
    expect(
      () => decoder.create(width: 100, height: 100),
      throwsA(isA<PlatformException>()),
    );
  });

  test('configure 忽略 0 尺寸，正常尺寸按帧率下发', () async {
    await decoder.configure(width: 0, height: 0);
    expect(calls, isEmpty, reason: '0 尺寸不应覆盖已协商的尺寸');

    await decoder.configure(width: 1920, height: 1080, fps: 60);
    expect(calls.single.method, 'configure');
    expect(calls.single.arguments, {'width': 1920, 'height': 1080, 'fps': 60});
  });

  test('push 以 Uint8List 传输码流', () async {
    final data = Uint8List.fromList([0, 0, 0, 1, 0x67, 0x42]);
    await decoder.push(data);
    final call = calls.single;
    expect(call.method, 'push');
    expect((call.arguments as Map)['data'], isA<Uint8List>());
    expect((call.arguments as Map)['data'], data);
  });

  test('stats 解析全部字段与毫秒换算', () async {
    final stats = await decoder.stats();
    expect(stats.textureId, 7);
    expect(stats.decoded, 120);
    expect(stats.dropped, 3);
    expect(stats.latencyP50Ms, 9.0);
    expect(stats.latencyP99Ms, 21.0);
    expect(stats.fps, 29.5);
    expect(stats.codec, 'c2.qti.avc.decoder');
    expect(stats.surfaceRebuilds, 1);
    expect(stats.lastError, '');
  });

  test('pause/resume 透传到通道', () async {
    await decoder.pause();
    await decoder.resume();
    expect(calls.map((call) => call.method), ['pause', 'resume']);
  });

  test('dispose 只下发一次，未创建时不调用', () async {
    await decoder.dispose();
    expect(calls, isEmpty);

    await decoder.create();
    await decoder.dispose();
    await decoder.dispose();
    expect(calls.where((call) => call.method == 'dispose').length, 1);
    expect(decoder.textureId, isNull);
  });
}
