// ============================================================
// 组件用途：RGBA 字节 → ui.Image 的预览组件（屏幕墙专用）
// 由谁组装：view/pages/wall_page.dart
// 数据来源：WallController.latest（Rust 解码后的 RGBA）
//
// 说明：屏幕墙是低频预览（约 10fps），允许逐帧转 Flutter Image；
//       远程控制页的实时画面要求外部纹理，见 session_page.dart 的画面占位说明。
// ============================================================

import 'dart:typed_data';
import 'dart:ui' as ui;

import 'package:flutter/material.dart';

import '../../core/theme/tokens.dart';

/// 把 RGBA 帧转换为可绘制的预览图。
class RdcRgbaPreview extends StatefulWidget {
  /// 创建预览组件。
  ///
  /// [bytes] RGBA 紧密排列；[width] / [height] 像素尺寸；[fit] 拉伸方式。
  const RdcRgbaPreview({
    super.key,
    required this.bytes,
    required this.width,
    required this.height,
    this.fit = BoxFit.cover,
  });

  /// RGBA 像素。
  final Uint8List bytes;

  /// 宽。
  final int width;

  /// 高。
  final int height;

  /// 拉伸方式。
  final BoxFit fit;

  @override
  State<RdcRgbaPreview> createState() => _RdcRgbaPreviewState();
}

/// 预览状态：持有当前 ui.Image 与代次，避免旧回调覆盖新帧。
class _RdcRgbaPreviewState extends State<RdcRgbaPreview> {
  /// 当前可绘制图像。
  ui.Image? _image;

  /// 帧代次；回调只在代次未变时生效。
  int _generation = 0;

  @override
  void initState() {
    super.initState();
    _decode();
  }

  @override
  void didUpdateWidget(covariant RdcRgbaPreview oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.bytes != widget.bytes ||
        oldWidget.width != widget.width ||
        oldWidget.height != widget.height) {
      _decode();
    }
  }

  /// 异步解码当前字节。
  ///
  /// 返回：无返回值；解码完成后 setState 并释放旧图像。
  void _decode() {
    final generation = ++_generation;
    final bytes = widget.bytes;
    final width = widget.width;
    final height = widget.height;
    if (width <= 0 || height <= 0 || bytes.length < width * height * 4) return;
    ui.decodeImageFromPixels(
      bytes,
      width,
      height,
      ui.PixelFormat.rgba8888,
      (image) {
        if (!mounted || generation != _generation) {
          image.dispose();
          return;
        }
        final previous = _image;
        setState(() => _image = image);
        previous?.dispose();
      },
    );
  }

  @override
  void dispose() {
    _generation += 1;
    _image?.dispose();
    _image = null;
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final image = _image;
    if (image == null) {
      return const ColoredBox(
        color: RdcTokens.surfaceRaised,
        child: Center(
          child: SizedBox(
            width: 18,
            height: 18,
            child: CircularProgressIndicator(strokeWidth: 2),
          ),
        ),
      );
    }
    return RawImage(image: image, fit: widget.fit, filterQuality: FilterQuality.low);
  }
}
