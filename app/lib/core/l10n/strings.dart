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

  // =====** 主界面骨架 =====**

  /// 入口：远程控制。
  static const entryControl = '远程控制';

  /// 入口：设备列表。
  static const entryDevices = '设备列表';

  /// 入口：屏幕墙。
  static const entryWall = '屏幕墙';

  /// 入口：设置。
  static const entrySettings = '设置';

  /// 本机状态标签。
  static const hostLabel = '本机';

  /// 桥接正常。
  static const bridgeOk = '桥接正常';

  /// 桥接异常。
  static const bridgeDown = '桥接异常';

  // =====** 设备列表页 =====**

  /// 搜索占位。
  static const deviceSearchHint = '搜索设备名或 ID';

  /// 新增设备。
  static const deviceAdd = '新增设备';

  /// 筛选：全部。
  static const filterAll = '全部';

  /// 筛选：仅信任。
  static const filterTrusted = '仅信任';

  /// 空状态标题。
  static const deviceEmptyTitle = '还没有设备';

  /// 空状态说明。
  static const deviceEmptyBody = '新增一台设备，或等待对方在本机发起连接后自动出现。';

  /// 连接按钮。
  static const connect = '连接';

  /// 编辑按钮。
  static const edit = '编辑';

  /// 删除按钮。
  static const delete = '删除';

  /// 信任状态。
  static const trusted = '已信任';

  /// 未信任状态。
  static const untrusted = '未信任';

  /// 从未在线。
  static const neverOnline = '从未在线';

  /// 设备 ID 标签。
  static const deviceIdLabel = '设备 ID';

  /// 设备名称标签。
  static const deviceNameLabel = '设备名称';

  /// 信任设备标签。
  static const trustDeviceLabel = '信任此设备';

  /// 同账号标签。
  static const sameAccountLabel = '同账号设备';

  /// 保存按钮。
  static const save = '保存';

  // =====** 屏幕墙页 =====**

  /// 屏幕墙页说明。
  static const wallNote = '本机回环预览（合成画面，真实 848x480/10fps 缩略流）；跨机会话待接线。';

  /// 宫格标签。
  static const wallGridLabel = '宫格';

  /// 等待设备。
  static const wallWaiting = '等待设备上线';

  /// 本机预览。
  static const wallLocalPreview = '本机预览';

  /// 帧率标签。
  static const wallFps = '帧率';

  /// 延迟标签。
  static const wallLatency = '预览延迟';

  // =====** 远程控制页 =====**

  /// 纹理待接线说明。
  static const sessionTexturePending = '外部纹理待接线：内核尚未注册 Windows 纹理，当前不显示假画面。';

  /// 断开。
  static const disconnect = '断开';

  /// 全屏。
  static const fullscreen = '全屏';

  /// 剪贴板。
  static const clipboard = '剪贴板';

  /// 文件。
  static const files = '文件';

  /// 待接线提示。
  static const pendingWiring = '该功能待接线';

  /// 控制者角色。
  static const roleController = '控制者';

  /// 观看者角色。
  static const roleViewer = '观看者';

  /// 输入权限提示。
  static const inputAllowed = '可注入输入';

  /// 只读提示。
  static const inputDenied = '只读，不可注入输入';

  // =====** 设置页（S5-2） =====**

  /// 网络与中继分组。
  static const networkSection = '网络与中继';

  /// 中继关闭提示（必须逐字展示）。
  static const relayOffHint = '关闭时零中继连接、失败不回退。';

  /// 中继开启提示。
  static const relayOnHint = '已开启：打洞失败时按下方策略使用中继服务器。';

  /// 中继状态：开。
  static const relayOn = '中继：开启';

  /// 中继状态：关。
  static const relayOff = '中继：关闭';

  /// 编码参数分组。
  static const encodingSection = '编码参数';

  /// 分辨率标签。
  static const resolutionLabel = '分辨率';

  /// 帧率标签。
  static const fpsLabel = '帧率';

  /// 码率标签。
  static const bitrateLabel = '码率';

  /// 降级策略分组。
  static const degradationSection = '降级策略';

  /// 自动重连标签。
  static const autoReconnectLabel = '断线自动重连';

  /// 方向标签。
  static const orientationShort = '方向';

  /// 快捷设置分组。
  static const quickSection = '快捷设置';

  /// 远程访问权限入口。
  static const permissionEntry = '远程访问权限';

  /// 权限入口说明。
  static const permissionEntryHint = '管理谁能远程查看或控制本机';

  // =====** 远程访问权限页 =====**

  /// 权限页标题。
  static const permissionTitle = '远程访问权限';

  /// ① 总开关。
  static const permSwitchSection = '远程访问';

  /// 总开关说明。
  static const permSwitchHint = '关闭时拒绝一切入站连接，已建立的会话也会断开。';

  /// ② 并发与接管。
  static const permConcurrentSection = '并发与接管';

  /// 并发模式标签。
  static const concurrentModeLabel = '并发模式';

  /// 独占。
  static const concurrentExclusive = '独占（一个控制者）';

  /// 一控多观。
  static const concurrentManyWatch = '一控多观（1 控制 + 观看者）';

  /// 接管策略标签。
  static const takeoverLabel = '接管策略';

  /// 接管：自动。
  static const takeoverAuto = '自动接管';

  /// 接管：确认。
  static const takeoverConfirm = '本机确认';

  /// 接管：禁止。
  static const takeoverForbid = '禁止接管';

  /// 确认超时标签。
  static const takeoverTimeoutLabel = '确认窗口';

  /// ③ 权限模板。
  static const permTemplateSection = '权限模板';

  /// 模板：只读。
  static const templateReadonly = '只读';

  /// 模板：标准。
  static const templateStandard = '标准';

  /// 模板：完整。
  static const templateFull = '完整';

  /// 模板：自定义。
  static const templateCustom = '自定义';

  /// ④ 授权访问者。
  static const permGrantsSection = '授权访问者';

  /// 授权列表空状态。
  static const grantsEmpty = '还没有授权访问者（授权列表内核接口待接线）';

  /// ⑤ 连接提示。
  static const permNoticeSection = '连接提示';

  /// 常驻提示标签。
  static const localNoticeLabel = '连接时本机常驻提示';

  /// ⑥ 审计与保留。
  static const permAuditSection = '审计与保留';

  /// 保留天数标签。
  static const retentionLabel = '日志保留';

  /// ⑦ 信任设备。
  static const permTrustSection = '信任设备';

  /// 信任设备说明。
  static const trustHint = '信任设备可跳过配对确认；同账号字段仅保存，S5-2 不做身份判定。';

  /// 本地配对策略。
  static const localPairPolicyLabel = '本地配对';

  /// 同账号策略。
  static const sameAccountPolicyLabel = '同账号设备';

  /// 策略：询问。
  static const policyAskShort = '询问';

  /// 策略：自动。
  static const policyAutoShort = '自动';

  /// 免密策略。
  static const passwordlessLabel = '允许完全免密（仅内网 + 二次确认）';

  /// 待接线徽章。
  static const pendingBadge = '待接线';
}
