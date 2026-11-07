// ============================================================
// 文件用途：触摸手势映射单元测试（宿主可跑，不依赖真机）
//
// 层级：test
// 被谁调用：flutter test
// 覆盖：单击序列、长按右键（含左键未释放）、拖动、滚轮折算、滚动/缩放判定
// ============================================================

import 'package:flutter_test/flutter_test.dart';
import 'package:rdc/model/gateway/rust_types.dart';
import 'package:rdc/model/input/touch_mapper.dart';

/// 断言事件的关键字段（InputEventSpec 未实现值相等，只能逐字段检查）。
void _expectEvent(
  InputEventSpec event, {
  required String kind,
  int? x,
  int? y,
  int? button,
  bool? pressed,
  int? deltaX,
  int? deltaY,
}) {
  expect(event.kind, kind);
  if (x != null) expect(event.x, x);
  if (y != null) expect(event.y, y);
  if (button != null) expect(event.button, button);
  if (pressed != null) expect(event.pressed, pressed);
  if (deltaX != null) expect(event.deltaX, deltaX);
  if (deltaY != null) expect(event.deltaY, deltaY);
}

void main() {
  // =====** 单击 =====**
  test('单击：move → 左键按下 → 左键抬起', () {
    final events = TouchMapper.tap(const OffsetI(640, 360));
    expect(events, hasLength(3));
    _expectEvent(events[0], kind: 'move', x: 640, y: 360);
    _expectEvent(events[1], kind: 'button', button: 1, pressed: true);
    _expectEvent(events[2], kind: 'button', button: 1, pressed: false);
  });

  // =====** 长按右键 =====**
  test('长按：左键已按下时先补抬起，再右键按/抬', () {
    final events = TouchMapper.longPressRight(
      const OffsetI(100, 200),
      leftHeld: true,
    );
    expect(events, hasLength(3));
    _expectEvent(events[0], kind: 'button', button: 1, pressed: false);
    _expectEvent(events[1], kind: 'button', button: 2, pressed: true);
    _expectEvent(events[2], kind: 'button', button: 2, pressed: false);
  });

  test('长按：左键未按下时只发右键序列', () {
    final events = TouchMapper.longPressRight(
      const OffsetI(1, 2),
      leftHeld: false,
    );
    expect(events, hasLength(2));
    _expectEvent(events[0], kind: 'button', button: 2, pressed: true);
    _expectEvent(events[1], kind: 'button', button: 2, pressed: false);
  });

  // =====** 拖动 =====**
  test('拖动：按下 → 移动（带按键）→ 抬起', () {
    _expectEvent(
      TouchMapper.dragStart(const OffsetI(10, 10)).single,
      kind: 'button',
      button: 1,
      pressed: true,
    );
    _expectEvent(
      TouchMapper.dragMove(const OffsetI(30, 50)).single,
      kind: 'button',
      button: 1,
      pressed: true,
    );
    _expectEvent(
      TouchMapper.dragEnd(const OffsetI(30, 50)).single,
      kind: 'button',
      button: 1,
      pressed: false,
    );
  });

  // =====** 滚轮折算 =====**
  test('滚轮：40px = 1 格，符号保留，最小 1 格', () {
    expect(TouchMapper.wheelTicks(0), 0);
    expect(TouchMapper.wheelTicks(20), 1);
    expect(TouchMapper.wheelTicks(-20), -1);
    expect(TouchMapper.wheelTicks(80), 2);
    expect(TouchMapper.wheelTicks(-120), -3);
  });

  test('滚轮：零位移不下发', () {
    expect(TouchMapper.scroll(dx: 0, dy: 0), isEmpty);
  });

  test('滚轮：垂直位移映射为 deltaY', () {
    final events = TouchMapper.scroll(dx: 0, dy: 60);
    expect(events, hasLength(1));
    _expectEvent(events.single, kind: 'wheel', deltaX: 0, deltaY: 2);
  });

  // =====** 滚动/缩放判定 =====**
  test('双指：缩放比例不变且有位移 → 滚动', () {
    expect(
      TouchMapper.isScrollGesture(scale: 1.01, dx: 0, dy: 30),
      isTrue,
    );
  });

  test('双指：缩放比例明显变化 → 不是滚动', () {
    expect(
      TouchMapper.isScrollGesture(scale: 1.3, dx: 0, dy: 30),
      isFalse,
    );
  });

  test('双指：无位移 → 不是滚动', () {
    expect(TouchMapper.isScrollGesture(scale: 1.0, dx: 0, dy: 0), isFalse);
  });
}
