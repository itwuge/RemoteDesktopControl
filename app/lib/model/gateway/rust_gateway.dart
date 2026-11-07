// ============================================================
// 文件用途：Rust FFI 网关（全应用唯一允许 import 生成代码的地方）
//
// 层级：model（数据来源层，负责把 Rust 能力转成 Dart 值对象）
// 被谁调用：controller/home_controller.dart、controller/settings_controller.dart
// 边界：不得包含任何 UI 逻辑；不得把生成代码的类型泄漏到 view 层
// ============================================================

import 'dart:io';

import 'package:flutter/services.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart';

import '../../core/utils/logger.dart';
import '../../src/rust/api.dart' as rust_api;
import '../../src/rust/clipboard_service.dart' as rust_clipboard;
import '../../src/rust/file_service.dart' as rust_file;
import '../../src/rust/frb_generated.dart';
import '../../src/rust/input_service.dart' as rust_input;
import '../../src/rust/session_service.dart' as rust_session;
import 'rust_types.dart';

/// Rust 核心的唯一调用入口。
///
/// 线程模型：所有方法都是异步的；生成代码内部把调用投递到 Rust 的线程池，
/// 不会阻塞 Flutter UI 线程（docs/01 §4.1）。
class RustGateway {
  /// 创建网关实例。
  ///
  /// [libraryOverridePath] 仅用于测试或非常规安装布局；生产环境保持 `null`，
  /// 由 [_libraryCandidates] 自动探测。
  RustGateway({this.libraryOverridePath});

  /// 测试/特殊布局下的显式动态库路径。
  final String? libraryOverridePath;

  /// 是否已完成初始化。
  bool _initialized = false;

  /// 返回是否已完成初始化。
  bool get isInitialized => _initialized;

  // =========** 初始化 =========

  /// 初始化 flutter_rust_bridge 并装载 Rust 动态库。
  ///
  /// 失败路径：先按候选路径逐个尝试 `DynamicLibrary.open`；全部不存在时
  /// 退回 FRB 默认加载器（适合 Android/iOS 由系统按包内路径装载）。
  ///
  /// 抛出：动态库缺失或 ABI 不匹配时抛出 [RustLibException]，由 main.dart 捕获并展示。
  Future<void> initialize() async {
    if (_initialized) return;

    final overridePath = libraryOverridePath;
    if (overridePath != null) {
      await RustLib.init(externalLibrary: ExternalLibrary.open(overridePath));
      _initialized = true;
      return;
    }

    // 为什么先探测文件：打包后的桌面应用工作目录不固定，FRB 生成的相对路径
    // （../crates/rdc-bridge/target/release/）只在开发期有效，生产必须按安装布局查找。
    for (final path in _libraryCandidates()) {
      if (File(path).existsSync()) {
        await RustLib.init(externalLibrary: ExternalLibrary.open(path));
        _initialized = true;
        RdcLogger.log('RustGateway', '动态库已装载: $path');
        return;
      }
    }

    // Android/iOS：动态库随 APK/IPA 打包，系统按库名检索，无需绝对路径。
    await RustLib.init();
    _initialized = true;
    RdcLogger.log('RustGateway', '动态库由默认加载器装载');
  }

  /// 按平台列出动态库候选路径。
  ///
  /// 顺序：可执行文件同目录 → macOS Frameworks → 开发期 cargo target 目录。
  List<String> _libraryCandidates() {
    final exeDir = File(Platform.resolvedExecutable).parent.path;
    final sep = Platform.pathSeparator;
    final fileName = _libraryFileName();

    final candidates = <String>[if (exeDir.isNotEmpty) '$exeDir$sep$fileName'];

    if (Platform.isMacOS) {
      // macOS 应用包规范：动态库放 Contents/Frameworks。
      candidates.add('$exeDir$sep..${sep}Frameworks$sep$fileName');
    }

    // 开发期回退：从 app 目录启动时 cargo 产物位置（仓库相对布局）。
    candidates.add(
      '$exeDir$sep..${sep}crates${sep}rdc-bridge${sep}target${sep}release$sep$fileName',
    );

    return candidates;
  }

  /// 返回当前平台的动态库文件名。
  String _libraryFileName() {
    if (Platform.isWindows) return 'rdc_bridge.dll';
    if (Platform.isMacOS) return 'librdc_bridge.dylib';
    return 'librdc_bridge.so';
  }

  // =========** 命令调用 =========

  /// 读取 Rust 核心版本与平台信息。
  ///
  /// 返回：[CoreInfo]，桥接未初始化时抛出 [StateError]。
  Future<CoreInfo> readCoreInfo() async {
    _ensureInitialized();
    // 三个调用互不依赖，并行执行以缩短首页加载时间。
    final results = await Future.wait([
      rust_api.coreVersion(),
      rust_api.platformName(),
      rust_api.bridgeStatus(),
    ]);
    return CoreInfo(
      coreVersion: results[0],
      platform: results[1],
      bridgeStatus: results[2],
    );
  }

  /// 读取 5 行环境自检结果。
  ///
  /// 返回：`[OK] 名称: 详情` 文本列表，顺序与 Rust 端一致。
  Future<List<String>> runSelfCheck() async {
    _ensureInitialized();
    return rust_api.doctorReport();
  }

  // =========** 设备列表 =========

  /// 读取设备列表。
  ///
  /// 返回：真实落盘文件中的条目；文件缺失时为空列表。
  Future<DeviceListSnapshot> loadDevices() async {
    _ensureInitialized();
    final dto = await rust_api.devicesLoad();
    return _toDeviceSnapshot(dto);
  }

  /// 新增设备。
  ///
  /// [deviceId] 设备标识；[name] 显示名；[trusted] 是否信任。
  Future<DeviceListSnapshot> addDevice({
    required String deviceId,
    required String name,
    required bool trusted,
  }) async {
    _ensureInitialized();
    final dto = await rust_api.deviceAdd(
      deviceId: deviceId,
      name: name,
      trusted: trusted,
    );
    return _toDeviceSnapshot(dto);
  }

  /// 修改设备名称、信任与同账号标记。
  Future<DeviceListSnapshot> updateDevice({
    required String deviceId,
    required String name,
    required bool trusted,
    required bool sameAccount,
  }) async {
    _ensureInitialized();
    final dto = await rust_api.deviceUpdate(
      deviceId: deviceId,
      name: name,
      trusted: trusted,
      sameAccount: sameAccount,
    );
    return _toDeviceSnapshot(dto);
  }

  /// 删除设备。
  Future<DeviceListSnapshot> removeDevice(String deviceId) async {
    _ensureInitialized();
    final dto = await rust_api.deviceRemove(deviceId: deviceId);
    return _toDeviceSnapshot(dto);
  }

  /// 把 Rust 设备 DTO 转成值对象。
  DeviceListSnapshot _toDeviceSnapshot(rust_api.DeviceListDto dto) {
    final entries = dto.devices
        .map((device) {
          final millis = device.lastOnlineMs.toInt();
          return DeviceEntry(
            deviceId: device.deviceId,
            name: device.name,
            lastOnline: millis == 0
                ? null
                : DateTime.fromMillisecondsSinceEpoch(millis),
            trusted: device.trusted,
            sameAccount: device.sameAccount,
          );
        })
        .toList(growable: false);
    return DeviceListSnapshot(entries: entries, filePath: dto.path);
  }

  // =========** 设置 =========

  /// 读取设置（Rust 端原子持久化，与 Dart 旧文件兼容）。
  Future<SettingsSnapshot> loadSettings() async {
    _ensureInitialized();
    final dto = await rust_api.settingsLoad();
    return _toSettingsSnapshot(dto);
  }

  /// 原子保存设置。
  Future<SettingsSnapshot> saveSettings(SettingsSnapshot settings) async {
    _ensureInitialized();
    final dto = await rust_api.settingsSave(
      settings: rust_api.SettingsDto(
        allowRelay: settings.allowRelay,
        relayAddress: settings.relayAddress,
        connectionPolicy: settings.connectionPolicy,
        orientation: settings.orientation,
        fileBandwidthRatio: settings.fileBandwidthRatio,
        videoWidth: settings.videoWidth,
        videoHeight: settings.videoHeight,
        videoFps: settings.videoFps,
        videoBitrateMbps: settings.videoBitrateMbps,
        autoReconnect: settings.autoReconnect,
        remoteAccessEnabled: settings.remoteAccessEnabled,
        concurrentMode: settings.concurrentMode,
        takeoverPolicy: settings.takeoverPolicy,
        takeoverTimeoutS: settings.takeoverTimeoutS,
        permissionTemplate: settings.permissionTemplate,
        permissionBits: settings.permissionBits,
        localNotice: settings.localNotice,
        auditRetentionDays: settings.auditRetentionDays,
        localPairPolicy: settings.localPairPolicy,
        sameAccountPolicy: settings.sameAccountPolicy,
        passwordlessPolicy: settings.passwordlessPolicy,
        path: settings.filePath,
      ),
    );
    return _toSettingsSnapshot(dto);
  }

  /// 把 Rust 设置 DTO 转成值对象。
  SettingsSnapshot _toSettingsSnapshot(rust_api.SettingsDto dto) {
    return SettingsSnapshot(
      allowRelay: dto.allowRelay,
      relayAddress: dto.relayAddress,
      connectionPolicy: dto.connectionPolicy,
      orientation: dto.orientation,
      fileBandwidthRatio: dto.fileBandwidthRatio,
      videoWidth: dto.videoWidth,
      videoHeight: dto.videoHeight,
      videoFps: dto.videoFps,
      videoBitrateMbps: dto.videoBitrateMbps,
      autoReconnect: dto.autoReconnect,
      remoteAccessEnabled: dto.remoteAccessEnabled,
      concurrentMode: dto.concurrentMode,
      takeoverPolicy: dto.takeoverPolicy,
      takeoverTimeoutS: dto.takeoverTimeoutS,
      permissionTemplate: dto.permissionTemplate,
      permissionBits: dto.permissionBits,
      localNotice: dto.localNotice,
      auditRetentionDays: dto.auditRetentionDays,
      localPairPolicy: dto.localPairPolicy,
      sameAccountPolicy: dto.sameAccountPolicy,
      passwordlessPolicy: dto.passwordlessPolicy,
      filePath: dto.path,
    );
  }

  // =========** 会话角色与能力 =========

  /// 查询角色权限（`viewer` / `controller`）。
  Future<RoleInfo> roleInfo(String role) async {
    _ensureInitialized();
    final dto = await rust_api.roleInfo(role: role);
    return RoleInfo(
      role: dto.role,
      canInjectInput: dto.canInjectInput,
      canViewWall: dto.canViewWall,
    );
  }

  /// 判定角色能否注入输入（观看者恒为 false）。
  Future<bool> inputAllowed(String role) async {
    _ensureInitialized();
    return rust_api.inputAllowed(role: role);
  }

  // =========** 屏幕墙预览 =========

  /// 启动本机回环屏幕墙预览。
  Future<WallStatusInfo> startWall() async {
    _ensureInitialized();
    return _toWallStatus(await rust_api.wallStart());
  }

  /// 停止预览。
  Future<WallStatusInfo> stopWall() async {
    _ensureInitialized();
    return _toWallStatus(await rust_api.wallStop());
  }

  /// 读取预览状态。
  Future<WallStatusInfo> wallStatus() async {
    _ensureInitialized();
    return _toWallStatus(await rust_api.wallStatus());
  }

  /// 读取最新预览帧；尚无帧时为 `null`。
  Future<WallPreviewFrame?> wallLatest() async {
    _ensureInitialized();
    final dto = await rust_api.wallLatest();
    if (dto == null) return null;
    return WallPreviewFrame(
      streamId: dto.streamId,
      frameId: dto.frameId,
      ptsUs: dto.ptsUs.toInt(),
      width: dto.width,
      height: dto.height,
      latencyUs: dto.latencyUs.toInt(),
      bytes: dto.bytes,
    );
  }

  /// 把 Rust 墙状态 DTO 转成值对象。
  WallStatusInfo _toWallStatus(rust_api.WallStatusDto dto) {
    return WallStatusInfo(
      running: dto.running,
      backend: dto.backend,
      captureBackend: dto.captureBackend,
      fps: dto.fps,
      encodedFrames: dto.encodedFrames.toInt(),
      decodedFrames: dto.decodedFrames.toInt(),
      droppedFrames: dto.droppedFrames.toInt(),
      width: dto.width,
      height: dto.height,
      latencyUs: dto.latencyUs.toInt(),
      latestPtsUs: dto.latestPtsUs.toInt(),
      lastError: dto.lastError,
    );
  }

  // =========** 控制页真实画面（外部纹理） =========

  /// Windows runner 的纹理桥方法通道。
  static const MethodChannel _controlTextureChannel = MethodChannel(
    'rdc/control_texture',
  );

  /// 启动控制页真实画面管线（被控端真实桌面采集）。
  Future<ControlPreviewInfo> startControlPreview() async {
    _ensureInitialized();
    return _toControlPreviewInfo(await rust_api.controlStart());
  }

  /// 停止控制页画面管线。
  Future<ControlPreviewInfo> stopControlPreview() async {
    _ensureInitialized();
    return _toControlPreviewInfo(await rust_api.controlStop());
  }

  /// 读取控制页画面状态。
  Future<ControlPreviewInfo> controlPreviewStatus() async {
    _ensureInitialized();
    return _toControlPreviewInfo(await rust_api.controlStatus());
  }

  /// 读取控制页最新一帧 RGBA 画面（Linux 降级通道；Windows 恒为 `null`）。
  ///
  /// 画面 FPS 由调用方控制；同尺寸逐帧转换开销与帧率成正比，建议 15~30Hz 轮询。
  Future<ControlFrameInfo?> controlFrame() async {
    _ensureInitialized();
    final dto = await rust_api.controlFrame();
    if (dto == null) return null;
    return ControlFrameInfo(
      frameId: dto.frameId.toInt(),
      width: dto.width,
      height: dto.height,
      ptsUs: dto.ptsUs.toInt(),
      latencyUs: dto.latencyUs.toInt(),
      bytes: dto.bytes,
    );
  }

  /// 把共享纹理句柄注册为 Flutter 外部纹理。
  ///
  /// 返回：Flutter 纹理 ID；非 Windows 或注册失败时为 `null`。
  Future<int?> registerControlTexture(ControlPreviewInfo info) async {
    if (!Platform.isWindows) return null;
    final reply = await _controlTextureChannel.invokeMapMethod<String, Object?>(
      'register',
      <String, Object?>{
        'handle_a': info.shareHandleA,
        'handle_b': info.shareHandleB,
        'event_a': info.eventA,
        'event_b': info.eventB,
        'width': info.width,
        'height': info.height,
      },
    );
    return (reply?['texture_id'] as num?)?.toInt();
  }

  /// 注销外部纹理（必须在停止 Rust 管线之前调用，避免句柄被提前释放）。
  Future<void> unregisterControlTexture() async {
    if (!Platform.isWindows) return;
    await _controlTextureChannel.invokeMethod<void>('unregister');
  }

  /// 把 Rust 控制 DTO 转成值对象。
  ControlPreviewInfo _toControlPreviewInfo(rust_api.ControlStatusDto dto) {
    return ControlPreviewInfo(
      running: dto.running,
      backend: dto.backend,
      encoder: dto.encoder,
      encoderFallbackReason: dto.encoderFallbackReason,
      decoder: dto.decoder,
      texturePath: dto.texturePath,
      decodePath: dto.decodePath,
      width: dto.width,
      height: dto.height,
      fps: dto.fps,
      capturedFrames: dto.capturedFrames.toInt(),
      encodedFrames: dto.encodedFrames.toInt(),
      decodedFrames: dto.decodedFrames.toInt(),
      droppedFrames: dto.droppedFrames.toInt(),
      encodedBytes: dto.encodedBytes.toInt(),
      bitrateKbps: dto.bitrateKbps.toInt(),
      latencyLastUs: dto.latencyLastUs.toInt(),
      latencyP50Us: dto.latencyP50Us.toInt(),
      latencyP99Us: dto.latencyP99Us.toInt(),
      readbackCount: dto.readbackCount.toInt(),
      shareHandleA: dto.shareHandleA,
      shareHandleB: dto.shareHandleB,
      eventA: dto.eventA,
      eventB: dto.eventB,
      lastError: dto.lastError,
      encoderResets: dto.encoderResets.toInt(),
      encoderInFlight: dto.encoderInFlight.toInt(),
    );
  }

  // =========** 控制页输入注入 =========

  /// 启动本机回环输入通道；[width]/[height] 为被控端桌面物理尺寸。
  Future<InputStatsInfo> startInput({
    required int width,
    required int height,
  }) async {
    _ensureInitialized();
    return _toInputStats(
      await rust_api.inputStart(desktopWidth: width, desktopHeight: height),
    );
  }

  /// 推送一批输入事件（顺序即注入顺序）。
  Future<InputStatsInfo> pushInput(List<InputEventSpec> events) async {
    _ensureInitialized();
    final dtos = events
        .map(
          (event) => rust_input.InputEventDto(
            kind: event.kind,
            x: event.x,
            y: event.y,
            button: event.button,
            pressed: event.pressed,
            deltaX: event.deltaX,
            deltaY: event.deltaY,
            scanCode: event.scanCode,
            modifiers: event.modifiers,
          ),
        )
        .toList(growable: false);
    return _toInputStats(await rust_api.inputPush(events: dtos));
  }

  /// 冲刷挂起的移动（指针抬起后调用）。
  Future<InputStatsInfo> flushInput() async {
    _ensureInitialized();
    return _toInputStats(await rust_api.inputFlush());
  }

  /// 停止输入通道并抬起所有键/钮。
  Future<InputStatsInfo> stopInput() async {
    _ensureInitialized();
    return _toInputStats(await rust_api.inputStop());
  }

  /// 读取输入统计。
  Future<InputStatsInfo> inputStats() async {
    _ensureInitialized();
    return _toInputStats(await rust_api.inputStatus());
  }

  /// 把 Rust 输入统计 DTO 转成值对象。
  InputStatsInfo _toInputStats(rust_input.InputStatsDto dto) {
    return InputStatsInfo(
      running: dto.running,
      received: dto.received.toInt(),
      emitted: dto.emitted.toInt(),
      injected: dto.injected.toInt(),
      rejected: dto.rejected.toInt(),
      merged: dto.merged.toInt(),
      throttled: dto.throttled.toInt(),
      pointerEvents: dto.pointerEvents.toInt(),
      keyEvents: dto.keyEvents.toInt(),
      wheelEvents: dto.wheelEvents.toInt(),
      keysDown: dto.keysDown.toInt(),
      lastX: dto.lastX,
      lastY: dto.lastY,
      successRate: dto.successRate,
      lastError: dto.lastError,
    );
  }

  // =========** 剪贴板与文件 =========

  /// 启动剪贴板同步（回环两端）。
  Future<ClipboardSyncInfo> startClipboard() async {
    _ensureInitialized();
    return _toClipboard(await rust_api.clipboardStart());
  }

  /// 本机剪贴板 → 远端。
  Future<ClipboardSyncInfo> clipboardSend() async {
    _ensureInitialized();
    return _toClipboard(await rust_api.clipboardSend());
  }

  /// 远端 → 本机剪贴板（含回读校验）。
  Future<ClipboardSyncInfo> clipboardPull() async {
    _ensureInitialized();
    return _toClipboard(await rust_api.clipboardPull());
  }

  /// 模拟对端修改剪贴板。
  Future<ClipboardSyncInfo> clipboardSetRemote(String text) async {
    _ensureInitialized();
    return _toClipboard(await rust_api.clipboardSetRemote(text: text));
  }

  /// 读取远端缓冲区文本（预览）。
  Future<String> clipboardRemoteText() async {
    _ensureInitialized();
    return rust_api.clipboardRemoteText();
  }

  /// 读取本机剪贴板文本（预览）。
  Future<String> clipboardLocalText() async {
    _ensureInitialized();
    return rust_api.clipboardLocalText();
  }

  /// 停止剪贴板同步。
  Future<ClipboardSyncInfo> stopClipboard() async {
    _ensureInitialized();
    return _toClipboard(await rust_api.clipboardStop());
  }

  /// 传输文件到接收目录并校验 SHA-256。
  ///
  /// [interruptAfterChunks] 大于 0 时模拟在第 N 块后断线并续传。
  Future<FileTransferInfo> transferFile(
    String path, {
    int interruptAfterChunks = 0,
    int chunkSize = 0,
  }) async {
    _ensureInitialized();
    final dto = await rust_api.fileTransfer(
      path: path,
      interruptAfterChunks: interruptAfterChunks,
      chunkSize: BigInt.from(chunkSize),
    );
    return _toFileTransfer(dto);
  }

  /// 剪贴板 DTO → 值对象。
  ClipboardSyncInfo _toClipboard(rust_clipboard.ClipboardStatusDto dto) {
    return ClipboardSyncInfo(
      running: dto.running,
      remoteChars: dto.remoteChars.toInt(),
      localChars: dto.localChars.toInt(),
      sent: dto.sent.toInt(),
      applied: dto.applied.toInt(),
      skippedDuplicates: dto.skippedDuplicates.toInt(),
      skippedEchoes: dto.skippedEchoes.toInt(),
      rejected: dto.rejected.toInt(),
      lastHash: dto.lastHash,
      verified: dto.verified,
      lastError: dto.lastError,
    );
  }

  /// 文件传输 DTO → 值对象。
  FileTransferInfo _toFileTransfer(rust_file.FileTransferDto dto) {
    return FileTransferInfo(
      source: dto.source,
      dest: dto.dest,
      bytes: dto.bytes.toInt(),
      chunks: dto.chunks,
      sourceSha256: dto.sourceSha256,
      destSha256: dto.destSha256,
      equal: dto.equal,
      resumed: dto.resumed,
      resumedBytes: dto.resumedBytes.toInt(),
      interrupted: dto.interrupted,
    );
  }

  // =========** 会话收尾（S5-4 块 3） =========

  /// 建立会话链路（回环阶段为进程内 UDP 回环桩）。
  Future<SessionStatusInfo> sessionStart(String role) async {
    _ensureInitialized();
    return _toSessionStatus(await rust_api.sessionStart(role: role));
  }

  /// 读取会话状态。
  Future<SessionStatusInfo> sessionStatus() async {
    _ensureInitialized();
    return _toSessionStatus(await rust_api.sessionStatus());
  }

  /// 断开会话：发 bye → 收回执 → 按序停服务 → 核查句柄与端口。
  Future<SessionTeardownInfo> sessionDisconnect(String reason) async {
    _ensureInitialized();
    final dto = await rust_api.sessionDisconnect(reason: reason);
    return SessionTeardownInfo(
      sessionId: dto.sessionId,
      role: dto.role,
      byeSent: dto.byeSent,
      byeReceived: dto.byeReceived,
      byeAcked: dto.byeAcked,
      ackLatencyUs: dto.ackLatencyUs.toInt(),
      byeBytes: dto.byeBytes,
      steps: dto.steps
          .map(
            (step) => TeardownStepInfo(
              name: step.name,
              ok: step.ok,
              detail: step.detail,
            ),
          )
          .toList(growable: false),
      servicesRunningAfter: dto.servicesRunningAfter,
      handlesBefore: dto.handlesBefore,
      handlesAfter: dto.handlesAfter,
      handlesDelta: dto.handlesDelta,
      portsReleased: dto.portsReleased,
      ok: dto.ok,
      lastError: dto.lastError,
    );
  }

  /// 会话状态 DTO → 值对象。
  SessionStatusInfo _toSessionStatus(rust_session.SessionStatusDto dto) {
    return SessionStatusInfo(
      running: dto.running,
      sessionId: dto.sessionId,
      role: dto.role,
      localPort: dto.localPort,
      peerPort: dto.peerPort,
      elapsedMs: dto.elapsedMs.toInt(),
      byeSent: dto.byeSent,
      byeAcked: dto.byeAcked,
    );
  }

  // =========** 权限页与授权（S5-5） =========

  /// 读取权限页快照（真实授权/信任设备/会话/审计）。
  Future<PermissionSnapshot> loadPermissionSnapshot() async {
    _ensureInitialized();
    return _toPermissionSnapshot(await rust_api.permissionSnapshot());
  }

  /// 为已配对设备签发授权。
  ///
  /// [validityDays] 为 0 时按默认 30 天；超过 90 天会被 Rust 拒绝。
  Future<PermissionSnapshot> createGrantForDevice({
    required String deviceId,
    required String template,
    required int validityDays,
  }) async {
    _ensureInitialized();
    final dto = await rust_api.grantCreateForDevice(
      deviceId: deviceId,
      template: template,
      validityDays: validityDays,
      nowMs: _nowMs(),
    );
    return _toPermissionSnapshot(dto);
  }

  /// 按公钥直接签发授权（高级入口）。
  Future<PermissionSnapshot> createGrant({
    required String granteePub,
    required String granteeName,
    required String template,
    required int validityDays,
  }) async {
    _ensureInitialized();
    final dto = await rust_api.grantCreate(
      granteePub: granteePub,
      granteeName: granteeName,
      template: template,
      validityDays: validityDays,
      nowMs: _nowMs(),
    );
    return _toPermissionSnapshot(dto);
  }

  /// 削减授权权限（只能减，不能加）。
  Future<PermissionSnapshot> updateGrantPerms({
    required String grantId,
    required int perms,
  }) async {
    _ensureInitialized();
    final dto = await rust_api.grantUpdatePerms(
      grantId: grantId,
      perms: perms,
      nowMs: _nowMs(),
    );
    return _toPermissionSnapshot(dto);
  }

  /// 换模板重签（权限提升必须换新令牌）。
  Future<PermissionSnapshot> reissueGrant({
    required String grantId,
    required String template,
    required int validityDays,
  }) async {
    _ensureInitialized();
    final dto = await rust_api.grantReissue(
      grantId: grantId,
      template: template,
      validityDays: validityDays,
      nowMs: _nowMs(),
    );
    return _toPermissionSnapshot(dto);
  }

  /// 吊销授权（最终一致）。
  Future<PermissionSnapshot> revokeGrant(String grantId) async {
    _ensureInitialized();
    final dto = await rust_api.grantRevoke(
      grantId: grantId,
      nowMs: _nowMs(),
    );
    return _toPermissionSnapshot(dto);
  }

  /// 从本机列表移除授权。
  Future<PermissionSnapshot> removeGrant(String grantId) async {
    _ensureInitialized();
    final dto = await rust_api.grantRemove(grantId: grantId);
    return _toPermissionSnapshot(dto);
  }

  /// 立即停止远程访问（停输入 → 断流 → 审计）。
  Future<RemoteStopReport> stopRemoteAccess() async {
    _ensureInitialized();
    final dto = await rust_api.remoteAccessStop(nowMs: _nowMs());
    return RemoteStopReport(
      inputStopped: dto.inputStopped,
      sessionStopped: dto.sessionStopped,
      teardownOk: dto.teardownOk,
      teardownSummary: dto.teardownSummary,
      auditWritten: dto.auditWritten,
      resumeRequiresConfirm: dto.resumeRequiresConfirm,
      stoppedAtMs: dto.stoppedAtMs.toInt(),
      snapshot: _toPermissionSnapshot(dto.snapshot),
    );
  }

  /// 恢复远程访问；[confirm] 必须由本机二次确认后置为 true。
  Future<PermissionSnapshot> resumeRemoteAccess({bool confirm = false}) async {
    _ensureInitialized();
    final dto = await rust_api.remoteAccessResume(
      confirm: confirm,
      nowMs: _nowMs(),
    );
    return _toPermissionSnapshot(dto);
  }

  /// 写入设备公钥（配对流程调用；页面无入口）。
  Future<PermissionSnapshot> setDevicePublicKey({
    required String deviceId,
    required String publicKeyB64,
  }) async {
    _ensureInitialized();
    final dto = await rust_api.deviceSetPublicKey(
      deviceId: deviceId,
      publicKeyB64: publicKeyB64,
    );
    return _toPermissionSnapshot(dto);
  }

  /// 当前 Unix 毫秒（u64 在 Dart 侧是 BigInt）。
  BigInt _nowMs() => BigInt.from(DateTime.now().millisecondsSinceEpoch);

  /// 权限快照 DTO → 值对象。
  PermissionSnapshot _toPermissionSnapshot(rust_api.PermissionSnapshotDto dto) {
    return PermissionSnapshot(
      remoteAccessEnabled: dto.remoteAccessEnabled,
      resumeRequiresConfirm: dto.resumeRequiresConfirm,
      concurrentMode: dto.concurrentMode,
      takeoverPolicy: dto.takeoverPolicy,
      takeoverTimeoutS: dto.takeoverTimeoutS,
      localPerms: dto.localPerms,
      maxGrantPerms: dto.maxGrantPerms,
      activeGrants: dto.activeGrants,
      grants: dto.grants
          .map(
            (grant) => GrantEntry(
              grantId: grant.grantId,
              granteePub: grant.granteePub,
              granteeName: grant.granteeName,
              granteeFingerprint: grant.granteeFingerprint,
              template: grant.template,
              perms: grant.perms,
              effectivePerms: grant.effectivePerms,
              notBeforeMs: grant.notBeforeMs.toInt(),
              expiresAtMs: grant.expiresAtMs.toInt(),
              revoked: grant.revoked,
              createdMs: grant.createdMs.toInt(),
              updatedMs: grant.updatedMs.toInt(),
            ),
          )
          .toList(growable: false),
      revokedEpoch: dto.revokedEpoch.toInt(),
      trustDevices: dto.trustDevices
          .map(
            (device) => TrustDeviceEntry(
              deviceId: device.deviceId,
              name: device.name,
              trusted: device.trusted,
              sameAccount: device.sameAccount,
              fingerprint: device.fingerprint,
              passwordlessReady: device.passwordlessReady,
              authMode: device.authMode,
              lastOnlineMs: device.lastOnlineMs.toInt(),
            ),
          )
          .toList(growable: false),
      sessionRunning: dto.sessionRunning,
      sessionRole: dto.sessionRole,
      connectedLabel: dto.connectedLabel,
      revocationNote: dto.revocationNote,
      auditTail: dto.auditTail
          .map(
            (entry) => PermissionAuditEntry(
              tsMs: entry.tsMs.toInt(),
              kind: entry.kind,
              subject: entry.subject,
              action: entry.action,
              oldValue: entry.oldValue,
              newValue: entry.newValue,
              outcome: entry.outcome,
            ),
          )
          .toList(growable: false),
      grantsPath: dto.grantsPath,
      identityFingerprint: dto.identityFingerprint,
      hint: dto.hint,
    );
  }

  /// 断言桥接已初始化。
  ///
  /// 抛出：[StateError] 当调用方在 init 之前发起命令（属编程错误，应在 main 中避免）。
  void _ensureInitialized() {
    if (!_initialized) {
      throw StateError('RustGateway 尚未初始化');
    }
  }
}
