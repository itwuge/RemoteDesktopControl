// ============================================================
// 文件用途：中文文案常量
//
// 层级：core（文案与逻辑分离，便于后续做多语言）
// 被谁调用：view/pages、view/components、controller（仅错误提示）
// 数据来源：docs/11 文案表
// ============================================================

/// 应用内全部面向用户的文案。
class AppStrings {
  const AppStrings._();

  // =====** 应用信息 =====**

  /// 应用名（窗口标题同名）。
  static const appName = 'RDC';

  /// 首页副标题。
  static const appTagline = '远程桌面控制 · Rust 核心 + Flutter 界面';

  /// 版本号前缀。
  static const versionLabel = '核心版本';

  /// 平台前缀。
  static const platformLabel = '构建平台';

  /// 桥接状态前缀。
  static const bridgeLabel = 'FFI 桥接';

  // =====** 首页 =====**

  /// 自检按钮。
  static const selfCheckButton = '运行自检';

  /// 自检进行中。
  static const selfChecking = '自检进行中…';

  /// 自检标题。
  static const selfCheckTitle = '环境自检';

  /// 自检未运行提示。
  static const selfCheckIdle = '点击「运行自检」查看 5 项环境检查结果。';

  // =====** 设置页 =====**

  /// 设置页标题。
  static const settingsTitle = '设置';

  /// 中继分组标题。
  static const relaySection = '中继';

  /// 中继总开关。
  static const relayEnable = '允许使用中继';

  /// 中继说明（强调默认不开启）。
  static const relayHint = '默认严格 P2P；只有你主动开启后，打洞失败才会连接中继服务器。';

  /// 中继地址标签。
  static const relayAddress = '中继地址';

  /// 打洞失败策略标签。
  static const policyLabel = '打洞失败策略';

  /// 策略：严格 P2P。
  static const policyStrict = '严格 P2P';

  /// 策略：询问后降级。
  static const policyAsk = '询问后降级';

  /// 策略：自动降级。
  static const policyAuto = '自动降级';

  /// 显示分组标题。
  static const displaySection = '显示与方向';

  /// 方向策略标签。
  static const orientationLabel = '方向策略';

  /// 方向：跟随主控端。
  static const orientationFollow = '跟随主控端';

  /// 方向：锁定横屏。
  static const orientationLandscape = '锁定横屏';

  /// 方向：锁定竖屏。
  static const orientationPortrait = '锁定竖屏';

  /// 文件传输分组标题。
  static const transferSection = '文件传输';

  /// 带宽上限标签。
  static const bandwidthLabel = '占用带宽上限';

  /// 带宽说明。
  static const bandwidthHint = '限制文件传输可使用的上行带宽，避免影响实时画面。';

  /// 保存按钮。
  static const saveButton = '保存设置';

  /// 保存成功提示。
  static const saveSuccess = '设置已保存';

  /// 保存失败提示。
  static const saveFailed = '设置保存失败，请检查磁盘权限';

  /// 中继开启确认标题。
  static const relayConfirmTitle = '确认开启中继？';

  /// 中继开启确认正文。
  static const relayConfirmBody = '中继会转发已加密的媒体流量，但会增加带宽成本与延迟。仅在双方网络无法直连时使用。';

  /// 取消。
  static const cancel = '取消';

  /// 确认。
  static const confirm = '确认';

  // =====** 通用错误 =====**

  /// 桥接初始化失败。
  static const bridgeInitFailed = 'Rust 核心加载失败';

  /// 未知错误。
  static const unknownError = '未知错误';
}
