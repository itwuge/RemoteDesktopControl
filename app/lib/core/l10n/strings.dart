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

  /// 首页副标题（面向用户，不写技术栈；清单 P0-1）。
  static const appTagline = '远程桌面控制';

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

  /// 关闭。
  static const close = '关闭';

  /// 确认。
  static const confirm = '确认';

  // =====** 通用错误 =====**

  /// 桥接初始化失败。
  static const bridgeInitFailed = '核心服务加载失败';

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

  /// 搜索标签（标签在上、控件在下）。
  static const deviceSearchLabel = '搜索';

  /// 搜索占位（只写可搜索的字段，不重复标签）。
  static const deviceSearchHint = '设备名 / 设备 ID';

  /// 新增设备。
  static const deviceAdd = '新增设备';

  /// 筛选：全部。
  static const filterAll = '全部';

  /// 筛选：仅信任。
  static const filterTrusted = '仅信任';

  /// 空状态标题。
  static const deviceEmptyTitle = '还没有设备';

  /// 空状态说明（下一步动作 + 结果，不用技术黑话）。
  static const deviceEmptyBody = '还没有设备。点「新增设备」，输入对方的设备 ID。';

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

  /// 屏幕墙页说明（只说用户能做什么，不写采集链路）。
  static const wallNote = '本机画面实时预览；设备上线后可在这里点开。';

  /// 未连接的宫格状态。
  static const wallNotConnected = '未连接';

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

  /// 画面建立中的占位文案（不解释纹理/内核实现）。
  static const sessionTexturePending = '正在建立画面…';

  /// 真实画面启动中。
  static const sessionTextureStarting = '正在连接被控端桌面…';

  /// 真实画面启动失败前缀。
  static const sessionTextureFailed = '画面启动失败';

  /// 锁屏采集说明。
  static const sessionLockedHint = '被控端锁屏时，这里看到的就是锁屏画面。';

  /// 断开。
  static const disconnect = '断开';

  /// 重连（画面中断后的恢复入口）。
  static const reconnect = '重连';

  /// 画面连接中断提示前缀。
  static const sessionDisconnected = '画面连接中断：';

  /// 全屏。
  static const fullscreen = '全屏';

  /// 复位视图（缩放/平移回到适配态）。
  static const resetView = '复位视图';

  /// 剪贴板。
  static const clipboard = '剪贴板';

  /// 文件。
  static const files = '文件';

  /// 暂不可用提示（接口缺失时的明确空状态，不用假开关）。
  static const unavailable = '暂不可用';

  /// 控制者角色。
  static const roleController = '控制者';

  /// 观看者角色。
  static const roleViewer = '观看者';

  /// 输入权限提示。
  static const inputAllowed = '可注入输入';

  /// 只读提示。
  static const inputDenied = '只读，不可注入输入';

  // =====** 账号（S5-3） =====**

  /// 账号分组标题（可选功能，不抢主流程；清单 P0-11）。
  static const accountSection = '账号（可选）';

  /// 未登录可用说明（页面必须明写价值，不自曝未实现）。
  static const accountNote = '登录后可在多台设备间自动发现；未登录也能使用全部功能。';

  /// 未登录。
  static const accountSignedOut = '未登录';

  /// 已登录前缀。
  static const accountSignedIn = '已登录';

  /// 服务探测中。
  static const accountServiceProbing = '检测中';

  /// 服务可用。
  static const accountServiceOnline = '连接正常';

  /// 服务未部署/不可达。
  static const accountServiceOffline = '未连接';

  /// 登录按钮。
  static const accountLogin = '登录';

  /// 登出按钮。
  static const accountLogout = '登出';

  /// 重新探测按钮。
  static const accountRetry = '重试';

  /// 同账号设备发现。
  static const accountDiscovery = '同账号设备发现';

  /// 未登录时设备发现不可用的说明。
  static const accountDiscoveryHint = '登录后可开启；未登录时保持关闭。';

  /// 登录弹窗说明（说结果，不自曝实现状态）。
  static const accountLoginBody = '登录后可跨设备自动发现并同步设备列表。';

  /// 账号输入提示。
  static const accountEmailLabel = '账号（邮箱）';

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
  static const grantsEmpty = '还没有授权访问者：点「签发授权」把已配对设备加入。';

  /// 新增授权按钮。
  static const grantAdd = '签发授权';

  /// 吊销授权按钮。
  static const grantRevoke = '吊销';

  /// 修改授权按钮。
  static const grantReissue = '改权限';

  /// 已吊销徽章。
  static const grantRevoked = '已吊销';

  /// 停止远程访问按钮。
  static const stopRemote = '立即停止远程访问';

  /// 恢复远程访问按钮。
  static const resumeRemote = '恢复远程访问（需本机确认）';

  /// 审计空状态。
  static const auditEmpty = '暂无审计记录';

  /// 内核数据错误标题。
  static const kernelError = '权限数据读取失败';

  /// 重试按钮。
  static const retry = '重试';

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

  /// 信任设备说明（去掉版本号与内部实现）。
  static const trustHint = '信任设备可跳过配对确认；同账号信息仅保存在本机。';

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

  // =====** 首页：本机可被控状态（清单 A1 / A3） =====**

  /// 状态卡标题。
  static const homeAccessSection = '本机可被控';

  /// 已允许连接。
  static const homeAccessAllowed = '已允许连接';

  /// 已停止连接。
  static const homeAccessStopped = '已停止连接';

  /// 未知状态（接口读取失败时的明确空状态）。
  static const homeAccessUnknown = '无法读取远程访问状态';

  /// 明确空状态下的说明。
  static const homeAccessUnknownHint = '请到「设置 → 关于与诊断」查看核心连接状态，或重试。';

  /// 恢复按钮（首页短文案）。
  static const homeResume = '恢复远程访问';

  /// 处理中（按钮禁用时的文案）。
  static const processing = '处理中…';

  /// 停止成功提示。
  static const stopDone = '已停止远程访问';

  /// 恢复成功提示。
  static const resumeDone = '已恢复远程访问';

  // =====** 会话页补充 =====**

  /// 连接目标标签。
  static const sessionTargetLabel = '连接目标';

  /// 连接目标：本机预览。
  static const sessionTargetLocal = '本机预览';

  /// 连接目标：设备会话。
  static const sessionTargetDevice = '设备会话';

  /// 断开确认标题。
  static const disconnectConfirmTitle = '断开连接？';

  /// 断开确认正文。
  static const disconnectConfirmBody = '断开后对方画面立即停止，需要重新连接才能继续。';

  /// 断开完成提示。
  static const disconnectDone = '会话已断开';

  /// 剪贴板：从本机复制到对方。
  static const copyToRemote = '从本机复制到对方';

  /// 剪贴板：从对方复制到本机。
  static const copyToLocal = '从对方复制到本机';

  /// 诊断分组标题。
  static const diagnostics = '诊断';

  // =====** 权限页补充 =====**

  /// 已允许连接（权限页状态卡）。
  static const permAllowed = '已允许连接';

  /// 已停止连接（权限页状态卡）。
  static const permStopped = '已停止连接';

  /// 无人在线。
  static const permNoPeople = '0 人在线';

  /// 无远程会话（首页状态卡）。
  static const homeAccessNoSession = '无远程会话';

  /// 停止远程访问确认标题。
  static const stopConfirmTitle = '停止远程访问？';

  /// 停止远程访问确认正文。
  static const stopConfirmBody = '停止后新的连接会被拒绝，正在进行的会话会立即断开。';

  /// 恢复远程访问确认标题。
  static const resumeConfirmTitle = '恢复远程访问？';

  /// 恢复远程访问确认正文。
  static const resumeConfirmBody = '恢复后，已授权的设备可以再次连接本机。';

  /// 修改权限说明（白话）。
  static const grantNote = '修改权限只能更严格；需要放宽请重新签发授权。';

  /// 设备标识标签。
  static const grantFingerprintLabel = '设备标识';

  // =====** 设备列表补充 =====**

  /// 删除确认标题。
  static const deleteConfirmTitle = '删除设备？';

  // =====** 设置页补充 =====**

  /// 恢复默认。
  static const resetDefaults = '恢复默认';

  // =====** 设置 → 关于与诊断（清单 P0-2 / P1-18） =====**

  /// 分组标题。
  static const aboutSection = '关于与诊断';

  /// 版本。
  static const aboutVersion = '版本';

  /// 运行平台。
  static const aboutPlatform = '运行平台';

  /// 核心连接。
  static const aboutBridge = '核心连接';

  /// 核心连接正常。
  static const aboutBridgeOk = '正常';

  /// 核心连接异常。
  static const aboutBridgeDown = '异常';

  /// 诊断说明。
  static const aboutHint = '诊断信息仅在此处显示，主界面不展示工程细节。';

  // =====** 第二批补充 =====**

  /// 通用：更多。
  static const more = '更多';

  /// 通用：上一页的设备卡片信任提示。
  static const trustedTooltip = '已信任：连接时跳过配对确认';

  /// 设备最近在线前缀。
  static const lastOnlinePrefix = '最近在线';

  /// 通信链路：直连。
  static const linkDirect = '直连';

  /// 通信链路：中继可用（未启用中继时不显示）。
  static const linkRelayAvailable = '中继可用';

  /// 设置：启动与托盘分组。
  static const startupSection = '启动与托盘';

  /// 设置：启动与托盘暂不可用说明（接口缺失时只给空状态，不做假开关）。
  static const startupUnavailable =
      '当前版本关闭窗口即退出，不会留在后台；开机自启、托盘驻留与远程时阻止休眠尚未提供。';

  /// 设置：恢复默认确认标题。
  static const resetDefaultsTitle = '恢复默认设置？';

  /// 设置：恢复默认确认正文。
  static const resetDefaultsBody = '只把本页选项恢复为出厂值，不会删除设备与授权；保存后才生效。';

  /// 设置：恢复默认完成提示。
  static const resetDefaultsDone = '已恢复默认，点击「保存设置」后生效';

  /// 关于：检查更新。
  static const checkUpdate = '检查更新';

  /// 关于：无更新通道时的明确说明。
  static const checkUpdateUnavailable =
      '当前版本不提供在线更新；请从发布页下载安装包覆盖安装。';

  /// 关于：能力与限制。
  static const capabilities = '能力与限制';

  /// 关于：能力与限制条目。
  static const capabilitiesLines = <String>[
    '仅支持 Windows 控制端与被控端；手机与平板暂不支持。',
    '暂不支持声音回传；画面之外的声音请用其他方式传递。',
    '暂不支持多显示器切换；连接后显示主显示器。',
    '重启、关机与切换显示器属于敏感操作，连接时仍需本机逐次确认。',
    '不按商业用途封禁，也不强制登录账号。',
  ];

  /// 屏幕墙：去设备列表。
  static const wallGoDevices = '去设备列表添加设备';

  /// 屏幕墙：无设备时的一句引导。
  static const wallNoDevices = '还没有设备；添加后这里会显示它们的缩略画面。';

  /// 权限页：高级权限折叠标题。
  static const advancedPermissions = '高级权限';

  /// 设置：恢复默认按钮。
  static const restoreDefaults = '恢复默认';

  /// 密码框：显示密码。
  static const showPassword = '显示密码';

  /// 密码框：隐藏密码。
  static const hidePassword = '隐藏密码';

  // =====** 首启引导（P1-17） =====**

  /// 首启标题。
  static const firstRunTitle = '这台电脑可以被远程连接吗？';

  /// 首启说明。
  static const firstRunBody =
      '连接前由你决定：允许后，只有你确认过的设备才能看到并操作这台电脑；'
      '选择随时可以更改，不会自动对外开启。';

  /// 首启：允许被连。
  static const firstRunAllow = '允许被连';

  /// 首启：暂不允许。
  static const firstRunDeny = '暂不允许';

  /// 首启：默认值说明。
  static const firstRunDefaultNote = '默认「暂不允许」；之后可在「远程访问权限」页随时更改。';

  /// 首启：保存失败提示。
  static const firstRunSaveFailed = '保存失败，请重试；也可稍后在设置页手动开启。';
}