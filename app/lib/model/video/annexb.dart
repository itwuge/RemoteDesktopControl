// =====** Annex-B 访问单元切分（Dart 侧） =====**
//
// 层级：model / 视频
// 用途：视频通道可能一次送达多帧（含 SPS/PPS/SEI + 多个 VCL 片），
//      这里按 H.264 访问单元边界切开，再逐单元推给平台解码器。
//
// 为什么不依赖 Kotlin 的 `AnnexBParser`：Dart 侧需要在“首帧 IDR 之前”
// 就停止向平台通道投递（避免无意义的跨通道拷贝），并让宿主单测可以
// 用离线码流验证切分与门控；Kotlin 侧仍保留同一套解析作为防御。
//
// 只做切分与关键帧判定，不解析 SPS 像素尺寸（尺寸由流元数据/configure 提供）。

import 'dart:typed_data';

/// 一个 H.264 访问单元（已带 4 字节起始码的 Annex-B 字节）。
class AnnexBUnit {
  /// 创建访问单元。
  const AnnexBUnit({
    required this.data,
    required this.keyFrame,
    required this.hasParameterSets,
    required this.nalTypes,
  });

  /// 可直接推给 `AndroidVideoDecoder.push` 的 Annex-B 字节。
  final Uint8List data;

  /// 是否含 IDR（NAL type 5）。
  final bool keyFrame;

  /// 是否含 SPS/PPS（NAL type 7/8）。
  final bool hasParameterSets;

  /// 本单元包含的 NAL 类型（调试与断言用）。
  final List<int> nalTypes;
}

// =====** 解析器 =====**

/// 有状态的 Annex-B 切分器。
///
/// 状态只保留“尚未等到 VCL 的参数集/SEI”，以便 SPS/PPS 与 IDR 分两次
/// 到达时仍能拼进同一个访问单元；不缓存 VCL，避免内存随码流增长。
class AnnexBFrameParser {
  final List<_Nal> _stash = <_Nal>[];

  /// 推入一段可能的 Annex-B 字节，返回已闭合的访问单元。
  List<AnnexBUnit> push(Uint8List bytes) {
    final units = <AnnexBUnit>[];
    for (final nal in _splitNals(bytes)) {
      final isVcl = nal.type >= 1 && nal.type <= 5;
      if (!isVcl) {
        _stash.add(nal);
        continue;
      }
      final group = <_Nal>[..._stash, nal];
      _stash.clear();
      units.add(_build(group));
    }
    return units;
  }

  /// 清空未配对的参数集（会话重连/暂停时调用）。
  void reset() {
    _stash.clear();
  }

  /// 按起始码切出 NAL，并裁掉尾部零字节。
  List<_Nal> _splitNals(Uint8List bytes) {
    final starts = <int>[];
    var index = 0;
    while (index + 2 < bytes.length) {
      if (bytes[index] == 0 && bytes[index + 1] == 0 && bytes[index + 2] == 1) {
        starts.add(index);
        index += 3;
      } else {
        index += 1;
      }
    }
    final nals = <_Nal>[];
    for (var i = 0; i < starts.length; i++) {
      final payloadStart = starts[i] + 3;
      final payloadEnd = i + 1 < starts.length ? starts[i + 1] : bytes.length;
      var end = payloadEnd;
      // 4 字节起始码的前导 0、以及下一个起始码前的填充 0 都属于分隔区。
      while (end > payloadStart && bytes[end - 1] == 0) {
        end -= 1;
      }
      if (end <= payloadStart) {
        continue;
      }
      nals.add(_Nal(type: bytes[payloadStart] & 0x1F, payload: bytes.sublist(payloadStart, end)));
    }
    return nals;
  }

  /// 把一组 NAL 重新拼成带 4 字节起始码的访问单元。
  AnnexBUnit _build(List<_Nal> group) {
    final builder = BytesBuilder(copy: false);
    for (final nal in group) {
      builder
        ..add(const <int>[0, 0, 0, 1])
        ..add(nal.payload);
    }
    final types = group.map((nal) => nal.type).toList(growable: false);
    return AnnexBUnit(
      data: builder.takeBytes(),
      keyFrame: types.contains(5),
      hasParameterSets: types.contains(7) || types.contains(8),
      nalTypes: types,
    );
  }
}

class _Nal {
  const _Nal({required this.type, required this.payload});

  final int type;
  final Uint8List payload;
}

// =====** IDR 门控 =====**

/// 首个关键帧之前的数据门控。
///
/// 为什么在 Dart 侧也做一次：平台通道每帧都要序列化字节，IDR 之前的
/// 非关键帧既解不出画面，又会占用通道带宽；Kotlin `FrameGate` 是第二道
/// 防御（防传输层绕过分片直接调用插件）。
class AnnexBGate {
  bool _waitingForKeyFrame = true;

  /// 是否仍在等首个关键帧。
  bool get waitingForKeyFrame => _waitingForKeyFrame;

  /// 判定一个访问单元是否放行。
  bool accept(AnnexBUnit unit) {
    if (_waitingForKeyFrame && !unit.keyFrame) {
      return false;
    }
    _waitingForKeyFrame = false;
    return true;
  }

  /// 重新等待关键帧（暂停/切流/重连）。
  void reset() {
    _waitingForKeyFrame = true;
  }
}
