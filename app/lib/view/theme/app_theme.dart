// ============================================================
// 文件用途：应用主题
//
// 层级：view/theme（只读令牌的组合，不含业务）
// 被谁调用：main.dart
// ============================================================

import 'package:flutter/material.dart';

import '../../core/theme/tokens.dart';

/// 全局主题构造。
class AppTheme {
  const AppTheme._();

  /// 构造深色主题；应用固定深色，避免两套配色带来维护成本。
  static ThemeData dark() {
    final base = ThemeData.dark(useMaterial3: true);
    return base.copyWith(
      scaffoldBackgroundColor: RdcTokens.background,
      colorScheme: base.colorScheme.copyWith(
        primary: RdcTokens.accent,
        surface: RdcTokens.surface,
        onSurface: RdcTokens.textPrimary,
        error: RdcTokens.danger,
      ),
      textTheme: base.textTheme.apply(
        bodyColor: RdcTokens.textPrimary,
        displayColor: RdcTokens.textPrimary,
      ),
      dividerColor: RdcTokens.border,
      appBarTheme: const AppBarTheme(
        backgroundColor: RdcTokens.background,
        surfaceTintColor: Colors.transparent,
        elevation: 0,
        centerTitle: false,
        titleTextStyle: TextStyle(
          color: RdcTokens.textPrimary,
          fontSize: 18,
          fontWeight: FontWeight.w600,
        ),
        iconTheme: IconThemeData(color: RdcTokens.textPrimary),
      ),
      // 按钮最小高度统一 36：页面与弹窗同一把尺子（清单 G1 / P1-2）。
      filledButtonTheme: FilledButtonThemeData(
        style: FilledButton.styleFrom(
          backgroundColor: RdcTokens.accent,
          foregroundColor: Colors.white,
          minimumSize: const Size(84, RdcTokens.controlHeight),
          padding: const EdgeInsets.symmetric(horizontal: 12),
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(RdcTokens.radiusControl),
          ),
          textStyle: const TextStyle(fontSize: RdcTokens.fontBody, fontWeight: FontWeight.w600),
        ),
      ),
      outlinedButtonTheme: OutlinedButtonThemeData(
        style: OutlinedButton.styleFrom(
          foregroundColor: RdcTokens.textPrimary,
          side: const BorderSide(color: RdcTokens.border),
          minimumSize: const Size(84, RdcTokens.controlHeight),
          padding: const EdgeInsets.symmetric(horizontal: 12),
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(RdcTokens.radiusControl),
          ),
        ),
      ),
      textButtonTheme: TextButtonThemeData(
        style: TextButton.styleFrom(
          foregroundColor: RdcTokens.textPrimary,
          minimumSize: const Size(64, RdcTokens.controlHeight),
          padding: const EdgeInsets.symmetric(horizontal: 12),
        ),
      ),
      // 弹窗走卡片体系：圆角 16、24 边距，不再使用 M3 默认的 28 圆角与浮标签
      // （清单 G2 / G9 / P0-14）。
      dialogTheme: DialogThemeData(
        backgroundColor: RdcTokens.surface,
        surfaceTintColor: Colors.transparent,
        insetPadding: const EdgeInsets.all(RdcTokens.dialogPadding),
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(RdcTokens.radiusCard),
        ),
        titleTextStyle: const TextStyle(
          color: RdcTokens.textPrimary,
          fontSize: RdcTokens.fontPageTitle,
          fontWeight: FontWeight.w600,
        ),
        contentTextStyle: const TextStyle(
          color: RdcTokens.textPrimary,
          fontSize: RdcTokens.fontBody,
        ),
      ),
      snackBarTheme: SnackBarThemeData(
        backgroundColor: RdcTokens.surfaceRaised,
        contentTextStyle: const TextStyle(color: RdcTokens.textPrimary),
        shape: RoundedRectangleBorder(
          borderRadius: BorderRadius.circular(RdcTokens.radiusControl),
        ),
      ),
    );
  }
}
