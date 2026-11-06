// ============================================================
// 文件用途：设置模型单元测试（S1 冒烟，验证序列化与容错）
//
// 层级：test
// 被谁调用：flutter test
// ============================================================

import 'package:flutter_test/flutter_test.dart';
import 'package:rdc/model/settings/app_settings_model.dart';

void main() {
  // =====** 默认值 =====**
  test('默认设置符合严格 P2P 约束', () {
    const settings = AppSettings.defaults();
    expect(settings.allowRelay, isFalse);
    expect(settings.connectionPolicy, ConnectionPolicy.strictP2p);
    expect(settings.fileBandwidthRatio, 0.4);
  });

  // =====** 序列化往返 =====**
  test('序列化后可无损还原', () {
    const original = AppSettings(
      allowRelay: true,
      relayAddress: 'relay.example.com:21117',
      connectionPolicy: ConnectionPolicy.askThenFallback,
      orientation: OrientationPolicy.landscape,
      fileBandwidthRatio: 0.25,
    );
    final restored = AppSettings.fromJson(original.toJson());
    expect(restored.toJson(), original.toJson());
  });

  // =====** 损坏配置容错 =====**
  test('越界带宽比例回落到默认值', () {
    final restored = AppSettings.fromJson({'file_bandwidth_ratio': 0.95});
    expect(restored.fileBandwidthRatio, 0.4);
  });
}
