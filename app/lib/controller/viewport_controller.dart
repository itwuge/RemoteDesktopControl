// ============================================================
// 文件用途：视口控制器（方向切换的唯一协调者 + 唯一坐标矩阵）
//
// 层级：controller（纯视图态，不进 Model、不持久化）
// 被谁调用：view/pages/session_page.dart（画面与手势）、wall/device 页读宽窄断点
// 依据：docs/08 §4（旋转只换皮不换核；M = T(offset) * S(scale) * F(fit) * R(rotation)）
// 边界：只改渲染层与坐标系；绝不触碰解码器、网络连接与纹理注册（textureId）
// ============================================================

import 'dart:math' as math;

import 'package:flutter/material.dart';

/// 视口状态：方向切换期间唯一持有「实际宽窄、缩放、平移、坐标矩阵」的对象。
///
/// 旋转事件只进 [updateViewport]，其他控制器不监听 `onMetricsChanged`，
/// 避免多处各算一套矩阵导致画面与输入错位。
class ViewportController extends ChangeNotifier {
  /// 宽布局断点（08 号文档 §4.5：`maxWidth > 900dp` 视为横向宽布局）。
  static const double wideBreakpoint = 900.0;

  /// 手机横屏的额外宽判定：宽度 ≥ 600dp 且明显宽于高（不依赖 `Orientation`，
  /// 平板分屏时 `Orientation` 会把竖向窗口报成横屏）。
  static const double phoneWideMinWidth = 600.0;

  /// 用户缩放下限。
  static const double minScale = 0.5;

  /// 用户缩放上限。
  static const double maxScale = 4.0;

  Size _viewport = Size.zero;
  Size _remote = const Size(1920, 1080);
  bool _wide = false;
  double _scale = 1.0;
  Offset _offset = Offset.zero;
  bool _matrixDirty = false;
  Matrix4 _matrix = Matrix4.identity();

  /// 本地视口尺寸（画面区的可用像素）。
  Size get viewportSize => _viewport;

  /// 被控端画面尺寸（来自解码帧）。
  Size get remoteSize => _remote;

  /// 是否宽布局（横屏全屏画面 + 顶部悬浮工具条）。
  bool get isWideLayout => _wide;

  /// 用户缩放。
  double get scale => _scale;

  /// 用户平移（本地像素）。
  Offset get offset => _offset;

  /// 矩阵是否尚未与最新布局同步；为真期间输入事件应丢弃。
  bool get matrixDirty => _matrixDirty;

  /// 远端归一化坐标（0..1）→ 本地像素的唯一变换矩阵。
  Matrix4 get matrix => _matrix;

  /// 远端像素坐标 → 本地像素的渲染矩阵（同一矩阵的像素空间形式）。
  Matrix4 get renderMatrix {
    final rect = displayRect;
    final sx = rect.width / _remote.width;
    final sy = rect.height / _remote.height;
    // `Matrix4` 构造参数是列主序（storage[i] == 第 i 个实参），平移在 12/13 位。
    return Matrix4(
      sx, 0, 0, 0,
      0, sy, 0, 0,
      0, 0, 1, 0,
      rect.left, rect.top, 0, 1,
    );
  }

  /// 等比适配后的画面矩形（scale=1、offset=0，含 letterbox 居中）。
  Rect get fittedRect {
    final fit = _fitFactor;
    final width = _remote.width * fit;
    final height = _remote.height * fit;
    return Rect.fromCenter(
      center: Offset(_viewport.width / 2, _viewport.height / 2),
      width: width,
      height: height,
    );
  }

  /// 叠加用户缩放/平移后的实际显示矩形。
  Rect get displayRect {
    final s = _fitFactor * _scale;
    final width = _remote.width * s;
    final height = _remote.height * s;
    final center = Offset(_viewport.width / 2, _viewport.height / 2) + _offset;
    return Rect.fromCenter(center: center, width: width, height: height);
  }

  /// 等比适配系数。
  double get _fitFactor {
    if (_viewport.isEmpty || _remote.isEmpty) return 1.0;
    return math.min(
      _viewport.width / _remote.width,
      _viewport.height / _remote.height,
    );
  }

  /// 单入口：视口（窗口尺寸、旋转、分屏）变化只从这里进来。
  ///
  /// 返回：无返回值；尺寸未变时不通知，避免 post-frame 回调自激。
  void updateViewport(Size size) {
    if (size.isEmpty || size == _viewport) return;
    _viewport = size;
    _wide = resolveWide(size);
    _matrixDirty = true;
    _refreshMatrix();
    notifyListeners();
  }

  /// 远端解码帧尺寸变化（只重算矩阵，不重建解码器）。
  void updateRemoteSize(Size size) {
    if (size.isEmpty || size == _remote) return;
    _remote = size;
    // 分辨率变化时复位视图态，避免沿用旧缩放落在画面外。
    _scale = 1.0;
    _offset = Offset.zero;
    _matrixDirty = true;
    _refreshMatrix();
    notifyListeners();
  }

  /// 布局（旋转动画/首帧）已完成：清脏标志，放行输入。
  void onLayoutDone() {
    if (!_matrixDirty) return;
    _matrixDirty = false;
    notifyListeners();
  }

  /// 手势缩放：[focal] 为本地像素焦点，缩放后该点下的远端内容保持不动。
  void zoom(double factor, {Offset? focal}) {
    if (factor == 1.0) return;
    final anchor = focal ?? Offset(_viewport.width / 2, _viewport.height / 2);
    final before = screenToRemote(anchor);
    final next = (_scale * factor).clamp(minScale, maxScale);
    if (next == _scale) return;
    _scale = next;
    _refreshMatrix();
    final after = remoteToScreen(before);
    _offset += anchor - after;
    _refreshMatrix();
    notifyListeners();
  }

  /// 手势平移（本地像素增量）。
  void pan(Offset delta) {
    if (delta == Offset.zero) return;
    _offset += delta;
    _refreshMatrix();
    notifyListeners();
  }

  /// 双击复位。
  void reset() {
    _scale = 1.0;
    _offset = Offset.zero;
    _refreshMatrix();
    notifyListeners();
  }

  /// 远端归一化坐标 → 本地像素。
  Offset remoteToScreen(Offset normalized) {
    final m = _matrix.storage;
    return Offset(
      m[0] * normalized.dx + m[4] * normalized.dy + m[12],
      m[1] * normalized.dx + m[5] * normalized.dy + m[13],
    );
  }

  /// 本地像素 → 远端归一化坐标。
  Offset screenToRemote(Offset local) {
    final m = _matrix.storage;
    final det = m[0] * m[5] - m[4] * m[1];
    if (det == 0.0) return Offset.zero;
    final dx = local.dx - m[12];
    final dy = local.dy - m[13];
    return Offset(
      (m[5] * dx - m[4] * dy) / det,
      (m[0] * dy - m[1] * dx) / det,
    );
  }

  /// 宽窄判定：纯几何（不读 `Orientation`）。
  ///
  /// 返回：`true` 表示宽布局。
  static bool resolveWide(Size size) {
    if (size.width > wideBreakpoint) return true;
    return size.width >= phoneWideMinWidth && size.width > size.height;
  }

  /// 重算唯一矩阵：归一化坐标 → 本地像素。
  ///
  /// `Matrix4` 构造参数是列主序：平移必须落在 storage[12]/[13]。
  /// 曾把 `rect.left/top` 写进第 4/8 位，导致 letterbox 的横向偏移丢失
  /// （画面画在 `displayRect`，点击却按无偏移的矩阵换算，横向必错位）。
  void _refreshMatrix() {
    final rect = displayRect;
    _matrix = Matrix4(
      rect.width, 0, 0, 0,
      0, rect.height, 0, 0,
      0, 0, 1, 0,
      rect.left, rect.top, 0, 1,
    );
  }
}
