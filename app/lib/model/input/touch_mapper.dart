// ============================================================
// 文件用途：移动端触摸手势 → 远端鼠标事件映射（纯函数，无 UI 依赖）
//
// 层级：model（输入映射，被动接收调用，不监听手势流）
// 被谁调用：view/pages/session_page.dart（仅 Android 触摸分支）
// 依据：docs/08 §3.1「单指移动/点击，双指滚动/缩放，长按右键」
// 边界：只产出 InputEventSpec 列表；不发送网络、不持有会话状态
//
// 为什么单独抽出来：
//   安卓触摸语义（长按右键、双指滚动）与桌面指针不同，
//   把映射写成纯函数后可在宿主上用 flutter test 验证序列，不依赖真机。
// ============================================================

import 'dart:math' as math;

import '../gateway/rust_types.dart';

/// 触摸手势到鼠标事件的映射器。
class TouchMapper {
  const TouchMapper._();

  /// 滚轮：每多少本地像素折算 1 格（触摸没有系统滚轮增量，只能按位移折算）。
  static const double pixelsPerWheelTick = 40.0;

  /// 双指手势判定为滚动的最大缩放偏差；超过即视为缩放（viewport 处理）。
  static const double scrollScaleTolerance = 0.06;

  /// 单击：移动到位后左键按下 + 抬起。
  ///
  /// [point] 为远端桌面物理像素坐标。
  static List<InputEventSpec> tap(OffsetI point) {
    return <InputEventSpec>[
      InputEventSpec.move(x: point.x, y: point.y),
      InputEventSpec.button(x: point.x, y: point.y, button: 1, pressed: true),
      InputEventSpec.button(x: point.x, y: point.y, button: 1, pressed: false),
    ];
  }

  /// 长按：先释放可能已按下的左键，再在当前位置点右键。
  ///
  /// [leftHeld] 为真表示触摸按下时已经下发过左键按下（当前实现的行为），
  /// 必须先补一次左键抬起，否则远端会一直保持左键按下状态。
  static List<InputEventSpec> longPressRight(
    OffsetI point, {
    required bool leftHeld,
  }) {
    return <InputEventSpec>[
      if (leftHeld)
        InputEventSpec.button(
          x: point.x,
          y: point.y,
          button: 1,
          pressed: false,
        ),
      InputEventSpec.button(x: point.x, y: point.y, button: 2, pressed: true),
      InputEventSpec.button(x: point.x, y: point.y, button: 2, pressed: false),
    ];
  }

  /// 拖动开始：左键按下。
  static List<InputEventSpec> dragStart(OffsetI point) {
    return <InputEventSpec>[
      InputEventSpec.button(x: point.x, y: point.y, button: 1, pressed: true),
    ];
  }

  /// 拖动过程：带按键的移动（交给 Rust 侧合并器限频）。
  static List<InputEventSpec> dragMove(OffsetI point) {
    return <InputEventSpec>[
      InputEventSpec.button(x: point.x, y: point.y, button: 1, pressed: true),
    ];
  }

  /// 拖动结束：左键抬起。
  static List<InputEventSpec> dragEnd(OffsetI point) {
    return <InputEventSpec>[
      InputEventSpec.button(x: point.x, y: point.y, button: 1, pressed: false),
    ];
  }

  /// 双指滚动：把累计像素位移折算成滚轮刻度（正值为向下/向右）。
  ///
  /// [dx]/[dy] 为本次手势的累计像素位移（本地像素）。
  /// 返回：空列表表示位移不足一格，调用方不必下发。
  static List<InputEventSpec> scroll({required double dx, required double dy}) {
    final ticksX = wheelTicks(dx);
    final ticksY = wheelTicks(dy);
    if (ticksX == 0 && ticksY == 0) return const <InputEventSpec>[];
    return <InputEventSpec>[InputEventSpec.wheel(deltaX: ticksX, deltaY: ticksY)];
  }

  /// 像素位移 → 滚轮刻度（保留符号，至少 1 格）。
  static int wheelTicks(double pixels) {
    if (pixels == 0) return 0;
    final ticks = (pixels.abs() / pixelsPerWheelTick).round();
    final value = ticks == 0 ? 1 : ticks;
    return pixels > 0 ? value : -value;
  }

  /// 双指手势是否应判为滚动而不是缩放。
  ///
  /// 规则：缩放比例几乎不变（±6%），且存在有效位移。
  static bool isScrollGesture({
    required double scale,
    required double dx,
    required double dy,
  }) {
    if ((scale - 1.0).abs() > scrollScaleTolerance) return false;
    return math.sqrt(dx * dx + dy * dy) > 0.0;
  }
}

/// 远端桌面像素坐标（避免在映射层依赖 Flutter 的 Offset 语义）。
class OffsetI {
  /// 创建整数坐标。
  const OffsetI(this.x, this.y);

  /// 横坐标（远端物理像素）。
  final int x;

  /// 纵坐标（远端物理像素）。
  final int y;
}
