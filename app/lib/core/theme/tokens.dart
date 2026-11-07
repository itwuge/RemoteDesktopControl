// ============================================================
// 文件用途：全局设计令牌（颜色/间距/圆角/字号）
//
// 层级：core（与业务无关的纯常量）
// 被谁调用：view/theme、view/pages、view/components
// 数据来源：docs/11（设计令牌真源）；此处只做 Dart 侧映射，禁止在页面里写魔法值
// ============================================================

import 'package:flutter/material.dart';

/// 设计令牌：所有页面与组件只能引用这里的值，保证视觉规范一致。
class RdcTokens {
  const RdcTokens._();

  // =====** 颜色 =====**

  /// 页面背景（深色单栏）。
  static const background = Color(0xFF0B1220);

  /// 卡片/分组框背景。
  static const surface = Color(0xFF121C2E);

  /// 卡片悬浮/次级背景。
  static const surfaceRaised = Color(0xFF17233A);

  /// 1px 细边框。
  static const border = Color(0xFF223049);

  /// 主强调色（主按钮、选中态）。
  static const accent = Color(0xFF3B82F6);

  /// 成功色（自检通过、桥接正常）。
  static const success = Color(0xFF22C55E);

  /// 警告/危险色（失败、关闭中继提示）。
  static const danger = Color(0xFFEF4444);

  /// 主文字色。
  static const textPrimary = Color(0xFFE5EAF3);

  /// 次文字色。
  static const textMuted = Color(0xFF8A98AD);

  // =====** 间距 =====**

  /// 页面外侧边距 24。
  static const spacePage = 24.0;

  /// 卡片内边距 20。
  static const spaceCard = 20.0;

  /// 常规元素间距 12。
  static const spaceMd = 12.0;

  /// 小间距 8。
  static const spaceSm = 8.0;

  /// 大区块间距 28。
  static const spaceXl = 28.0;

  /// 中段间距 16。
  static const spaceLg = 16.0;

  // =====** 控件尺寸 =====**

  /// 列表/工具栏按钮统一高度 36（S5-2 界面规矩）。
  static const controlHeight = 36.0;

  /// 页面内容最大宽度（上下单栏，不做左右分栏）。
  static const pageMaxWidth = 1100.0;

  // =====** 圆角 =====**

  /// 卡片圆角 16。
  static const radiusCard = 16.0;

  /// 控件圆角 10。
  static const radiusControl = 10.0;

  /// 胶囊圆角 999。
  static const radiusPill = 999.0;

  // =====** 字号 =====**

  /// 页面标题 24。
  static const fontTitle = 24.0;

  /// 分组标题 16。
  static const fontSection = 16.0;

  /// 正文 14。
  static const fontBody = 14.0;

  /// 辅助文字 12。
  static const fontCaption = 12.0;
}
