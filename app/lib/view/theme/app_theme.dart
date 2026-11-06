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
      filledButtonTheme: FilledButtonThemeData(
        style: FilledButton.styleFrom(
          backgroundColor: RdcTokens.accent,
          foregroundColor: Colors.white,
          minimumSize: const Size(120, 40),
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
          minimumSize: const Size(100, 40),
          shape: RoundedRectangleBorder(
            borderRadius: BorderRadius.circular(RdcTokens.radiusControl),
          ),
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
