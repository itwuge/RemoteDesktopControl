// =====** Annex-B 切分与门控单测（Dart 侧） =====**
//
// 覆盖阶段 3 判据 C3/C4 的纯逻辑部分：
// - 3/4 字节起始码、尾部填充零、多访问单元；
// - SPS/PPS 跨 chunk 缓存；
// - 首个 IDR 之前的非关键帧被门控拒绝。

import 'dart:typed_data';

import 'package:flutter_test/flutter_test.dart';
import 'package:rdc/model/video/annexb.dart';

Uint8List _bytes(List<int> value) => Uint8List.fromList(value);

void main() {
  test('4 字节起始码被识别，尾部零被裁掉', () {
    final parser = AnnexBFrameParser();
    final units = parser.push(
      _bytes(<int>[0, 0, 0, 1, 7, 0x42, 0, 0, 0, 1, 5, 0x88, 0, 0]),
    );

    expect(units, hasLength(1));
    expect(units.single.nalTypes, <int>[7, 5]);
    expect(units.single.keyFrame, isTrue);
    expect(units.single.data.sublist(4), <int>[7, 0x42, 0, 0, 0, 1, 5, 0x88]);
  });

  test('3 字节起始码与多访问单元按 VCL 边界切开', () {
    final parser = AnnexBFrameParser();
    final units = parser.push(
      _bytes(<int>[
        0, 0, 1, 7, 0x42, // SPS
        0, 0, 1, 8, 0xCE, // PPS
        0, 0, 1, 5, 0x88, // IDR
        0, 0, 1, 1, 0x9A, // P
      ]),
    );

    expect(units, hasLength(2));
    expect(units.first.nalTypes, <int>[7, 8, 5]);
    expect(units.last.nalTypes, <int>[1]);
    expect(units.last.keyFrame, isFalse);
  });

  test('SPS/PPS 与 IDR 分两次到达时仍拼进同一访问单元', () {
    final parser = AnnexBFrameParser();
    final first = parser.push(_bytes(<int>[0, 0, 1, 7, 0x42, 0, 0, 1, 8, 0xCE]));
    expect(first, isEmpty);

    final second = parser.push(_bytes(<int>[0, 0, 1, 5, 0x88]));
    expect(second, hasLength(1));
    expect(second.single.nalTypes, <int>[7, 8, 5]);
    expect(second.single.hasParameterSets, isTrue);
  });

  test('IDR 门控拒绝首个关键帧之前的帧，开门后放行', () {
    final parser = AnnexBFrameParser();
    final gate = AnnexBGate();

    final beforeIdr = parser.push(_bytes(<int>[0, 0, 1, 1, 0x9A]));
    expect(beforeIdr, hasLength(1));
    expect(gate.accept(beforeIdr.single), isFalse);

    final idr = parser.push(_bytes(<int>[0, 0, 1, 5, 0x88]));
    expect(gate.accept(idr.single), isTrue);
    expect(gate.waitingForKeyFrame, isFalse);

    final afterIdr = parser.push(_bytes(<int>[0, 0, 1, 1, 0x9A]));
    expect(gate.accept(afterIdr.single), isTrue);
  });

  test('reset 后重新等待关键帧', () {
    final parser = AnnexBFrameParser();
    final gate = AnnexBGate();
    expect(gate.accept(parser.push(_bytes(<int>[0, 0, 1, 5, 0x88])).single), isTrue);

    gate.reset();
    expect(gate.waitingForKeyFrame, isTrue);
    expect(gate.accept(parser.push(_bytes(<int>[0, 0, 1, 1, 0x9A])).single), isFalse);
  });
}
