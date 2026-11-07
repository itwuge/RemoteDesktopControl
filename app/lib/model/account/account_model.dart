// ============================================================
// 文件用途：账号本地状态模型（登录态 / 同账号设备发现开关）
//
// 层级：model
// 被谁使用：model/account/account_repository.dart、controller/account_controller.dart
//
// 边界：这里只描述「本机记录的账号状态」，不承载任何云端账号能力；
//      账号服务未部署时，本模型仍是完整的本地状态，不影响其他功能。
// ============================================================

/// 账号本地状态。
class AccountState {
  /// 创建账号状态。
  const AccountState({
    required this.email,
    required this.displayName,
    required this.discoveryEnabled,
  });

  /// 空状态（未登录）。
  static const empty = AccountState(
    email: null,
    displayName: null,
    discoveryEnabled: false,
  );

  /// 登录账号（邮箱/账号）；`null` 表示未登录。
  final String? email;

  /// 展示名（取账号 @ 前的部分）。
  final String? displayName;

  /// 同账号设备发现是否开启（仅登录后可用）。
  final bool discoveryEnabled;

  /// 是否已登录（以本地记录为准）。
  bool get signedIn => email != null && email!.isNotEmpty;

  /// 复制并覆盖字段。
  ///
  /// [clearIdentity] 为 `true` 时清空账号与展示名（登出用）。
  AccountState copyWith({
    String? email,
    String? displayName,
    bool? discoveryEnabled,
    bool clearIdentity = false,
  }) {
    return AccountState(
      email: clearIdentity ? null : (email ?? this.email),
      displayName: clearIdentity ? null : (displayName ?? this.displayName),
      discoveryEnabled: clearIdentity
          ? false
          : (discoveryEnabled ?? this.discoveryEnabled),
    );
  }

  /// 从 JSON 还原；字段缺失或类型不符时退回空状态。
  factory AccountState.fromJson(Map<String, dynamic> json) {
    final email = json['email'];
    final displayName = json['displayName'];
    final discovery = json['discoveryEnabled'];
    return AccountState(
      email: email is String && email.isNotEmpty ? email : null,
      displayName: displayName is String ? displayName : null,
      discoveryEnabled: discovery is bool ? discovery : false,
    );
  }

  /// 序列化为 JSON。
  Map<String, dynamic> toJson() => {
        'email': email,
        'displayName': displayName,
        'discoveryEnabled': discoveryEnabled,
      };
}
