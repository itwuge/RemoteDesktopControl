# S9 · Android 客户端判据与实测记录

- 记录时间：2026-11-07（CST）
- 工作树：`C:\Users\Administrator\Desktop\RDC-android`，分支 `android-client`（基于 `linux-x11`）
- 构建机：Ubuntu 22.04 VM `192.168.37.130`（Flutter 3.47.9 / Dart 3.13.7 / JDK 17.0.20.1 / Rust 1.99.1 / Android SDK 36 / NDK 28.2.13676358 / cargo-ndk 4.1.2）
- 约定：**【已验证】**=有命令输出或产物佐证；**【未验证】**=缺少设备/条件，未实测。

---

## 一、判据（阶段一开始即登记）

### 阶段 1 · 构建链

| # | 判据 | 判定方式 |
|---|---|---|
| A1 | Gradle/Maven 镜像配置入库，AGP/Kotlin/NDK 版本固定 | `git diff` 可见 settings/build/properties 改动；构建日志不再出现 `services.gradle.org`、`maven.google.com` 拦截 |
| A2 | 不含 Rust 的空壳 APK 能产出 | 文件存在 + `unzip -l` 中只有 Flutter 自带 `libflutter.so`，无 `librdc_bridge.so` |
| A3 | `librdc_bridge.so` 交叉编译通过且为 arm64 | `file` 报 `ELF 64-bit ... ARM aarch64`；`nm -D` 可见 `rdc_selfcheck` |
| A4 | APK 内实际打入 `.so` | `unzip -l app-debug.apk` 出现 `lib/arm64-v8a/librdc_bridge.so` |
| A5 | `dlopen` 成功并返回自检字符串 | 真机 logcat `rdc.AndroidSelfCheck`；无设备时以宿主 Linux 同源码 dlopen 作替代证据（标注 ABI 不同） |

### 阶段 2 · 最小可用功能（本轮仅登记，未完成）

| # | 判据 | 判定方式 |
|---|---|---|
| B1 | MediaCodec 硬解 H.264 出画面 | 真机看到远端画面；记录首帧延迟与 60s 平均帧率 |
| B2 | 触摸→鼠标（点击/拖动/滚轮） | 100 次点击命中偏差 ≤4px（按远端像素计） |
| B3 | 软键盘输入上报 | 中英文输入在被控端可见 |
| B4 | 横竖屏切换不重建解码器 | 旋转前后解码器实例 ID 不变、画面不黑 |
| B5 | 稳定性 | 连续 5 分钟不崩；切前后台不崩 |

### 阶段 3 · 接线（本轮先落判据，再实现）

| # | 判据 | 判定方式 |
|---|---|---|
| C1 | `SessionController` 与 Windows 路径共用骨架 | 同一个 `startPreview/stopPreview/endSession/_startInput/_stopInput` 流程；Android 只在 `_startAndroidVideo/_startAndroidInput` 分支分叉，不存在第二份 session 状态机 |
| C2 | Android 分支创建 `textureId` 并交给现有 `Texture` | 宿主单测用 mock 通道：`create` 返回纹理号，`controller.textureId.value` 等于该值；`session_page.dart` 不改布局 |
| C3 | 视频通道 Annex-B 帧被切分后推给 `AndroidVideoDecoder.push` | 离线码流（内嵌 SPS/PPS/IDR/非 IDR）走 `VideoFrameChannel` → 控制器 → mock `push`；断言访问单元切分正确、顺序不变 |
| C4 | IDR 前不启动解码 | Dart 侧 `AnnexBGate` 丢弃首个 IDR前的非关键帧，mock 不收到这些字节；Kotlin 侧 `FrameGate` 同一判定有 JVM 单测 |
| C4b | 能力位为 0 时不发文本 | 协商位不含 bit13 时 `RemoteTextInput` 的编辑不进入 `gateway.pushInput`；置位后才产生 `kind=text` 事件 |
| C5 | 兼容性结论落盘 | `research/android-s9-text-input-compat.md` 表格补 Windows 主控端与旧 Android 客户端两行，且有代码依据（`Capabilities::DEFAULT`/`negotiate`） |

### 阶段 4 · 真实视频传输（判据先落盘）

| # | 判据 | 判定方式 |
|---|---|---|
| D1 | 视频元数据协议落地 | `rdc-protocol::video` 定义 Meta（宽/高/fps/编码/色彩空间/范围/SPS-PPS 策略 + config_id），编解码互转有单测 |
| D2 | 分片/重组规则明确 | 分片 ≤1352 字节、帧头含 `frame_id`/`pts_us`/`total_len`/`frag_count`；分片乱序可重组；重复分片只留一份 |
| D3 | 丢包/乱序等下一个 IDR | 帧号空洞后 `needs_keyframe=true`，后续非 IDR 整帧丢弃；新 IDR 到达后恢复，统计 `resyncs` 递增 |
| D4 | 元数据变化触发重建 | `config_id`/宽高/编码/色彩任一变化 → `ConfigChanged` 事件，清空待重组帧并重新等 IDR |
| D5 | 桥接订阅 API | `video_ingest`/`video_poll_frame`/`video_stats`/`video_reset` 可用；坏消息只记账不 panic |
| D6 | Dart 吃到桥接帧 | `pollVideoFrame` 轮询 → `VideoFrameChannel` → 切分 → 门控 → `AndroidVideoDecoder.push`，宿主单测覆盖 |
| D7 | 协商位回填 | 会话建立后调用 `sessionNegotiatedCapabilities` 并写入 `setNegotiatedCapabilities`；bit13 为 0 时文本不发 |
| D8 | 端到端判据 | Rust 单测覆盖“协议分片→桥接重组”；Dart 单测覆盖“桥接 DTO→切分→门控→解码器桩”；FFI 边界由 FRB 生成的编译/APK 构建覆盖 |

---

## 二、阶段 1 实测结果

### 2.1 空壳 APK（A2）—— 【已验证】

| 项 | 实测 |
|---|---|
| 命令 | `flutter build apk --debug --target-platform android-arm64`（先 `flutter pub get`） |
| 耗时 | **184 s**（含 pub get；Gradle `assembleDebug` 182.1 s，首次构建含依赖下载） |
| 产物 | `app/build/app/outputs/flutter-apk/app-debug.apk` |
| 体积 | **77,843,119 B**（≈74.2 MiB，debug 未裁剪） |
| sha256 | `9702a458589f3662b058408abdfb3a5aa964efbddd70300cd1359ea1e53240e1` |
| 原生库 | 仅 `lib/arm64-v8a/libflutter.so`（38,846,424 B）与 `lib/arm64-v8a/libVkLayer_khronos_validation.so`（15,240,080 B）；**无 librdc_bridge.so** |

### 2.2 Rust 交叉编译（A3）—— 【已验证】

| 项 | 实测 |
|---|---|
| 命令 | `ANDROID_NDK_HOME=~/Android/Sdk/ndk/28.2.13676358 cargo ndk -t arm64-v8a -o app/android/app/src/main/jniLibs build --release -p rdc-bridge` |
| 耗时 | **53 s**（release，首次含依赖编译） |
| 产物 | `app/android/app/src/main/jniLibs/arm64-v8a/librdc_bridge.so`，**1,141,688 B**（stripped） |
| sha256 | `f4039192a3f9020e4e5c56b5a91c7872fa09bceb9df087d5a7d0287ef1863d3e` |
| 架构 | `ELF 64-bit LSB shared object, ARM aarch64, version 1 (SYSV), dynamically linked, stripped` |
| 依赖 | 仅 `libdl.so / libm.so / libc.so`（未带入 X11/桌面依赖） |
| 符号 | `nm -D` 输出 `0000000000084198 T rdc_selfcheck` |
| 编译告警 | `rdc-bridge` 11 条 dead_code（非 Windows 分支的屏幕墙函数），无错误 |

> 说明：`docs/08` 写的库名是 `librdc.so`，但仓库既有代码与打包脚本统一用 `librdc_bridge.so`；本轮沿用代码事实，并在 Dart/Kotlin 侧按 `librdc_bridge.so` 加载。

### 2.3 含 Rust 库的 APK（A4）—— 【已验证】

| 项 | 实测 |
|---|---|
| 命令 | `flutter build apk --debug --target-platform android-arm64`（增量） |
| 耗时 | **16 s**（增量；不含 pub get） |
| 产物 | `app/build/app/outputs/flutter-apk/app-debug.apk` |
| 体积 | **93,382,770 B**（比空壳 +15.5 MB，主要是压缩后的 .so 与重建资源） |
| sha256 | `32dd5e424c202ef0eda92e5bc4454f1d0d3c7361d2b6f8f055024ea58d73b977` |
| `unzip -l` 关键行 | `1141680 ... lib/arm64-v8a/librdc_bridge.so`（另有 libflutter.so / libVkLayer） |

### 2.4 dlopen + 自检字符串（A5）

| 项 | 结果 |
|---|---|
| 真机 logcat | **【未验证】**：`adb devices` 无设备，VM 无 `/dev/kvm` 不能起模拟器 |
| 替代证据（宿主 Linux x86_64，同源码同符号） | **【已验证】** `cargo build --release -p rdc-bridge` 41.72 s → Python `ctypes.CDLL` 加载成功，`rdc_selfcheck()` 返回 `rdc-bridge selfcheck OK os=linux arch=x86_64 abi=unix` |
| 代码路径 | **【已验证（静态）】** `app/lib/model/gateway/android_selfcheck.dart` 在 Android 上 `DynamicLibrary.open('librdc_bridge.so')` → `lookupFunction('rdc_selfcheck')` → 字符串写入 `RdcLogger`（tag `rdc.AndroidSelfCheck`） |

**诚实边界**：x86_64 宿主 dlopen 只证明「符号与 C ABI 正确、库可被动态加载」；**arm64-v8a 在 Android 运行时能否 dlopen 未验证**（缺真机）。不得据此声称「装到手机上能用」。

### 2.5 网络与镜像（A1）—— 【已验证】

| 探测目标 | 结果 |
|---|---|
| `https://services.gradle.org/distributions/gradle-9.3.1-all.zip` | HTTP **307**（随后不可用）→ wrapper 改用 `https://mirrors.cloud.tencent.com/gradle/gradle-9.3.1-all.zip`（HTTP 200） |
| `https://maven.google.com/...` | **连接超时**（curl 28，12.7 s） |
| `https://maven.aliyun.com/repository/google/...` | HTTP 200 |
| `https://maven.aliyun.com/repository/public/...` | HTTP 200 |
| `https://dl.google.com/dl/android/maven2/...` | HTTP 200（作为兜底可用） |
| `https://repo.maven.apache.org/maven2/` | HTTP 200 |
| 构建结果 | 空壳 APK 构建 **exit=0**，130 个依赖文件正常解析；日志无 `maven.google.com` 报错 |
| 版本固定 | AGP **9.1.0**、Kotlin **2.4.0**、Gradle **9.3.1**、compileSdk/targetSdk **36**、minSdk **26**、NDK **28.2.13676358** |

### 2.6 静态分析与单元测试

| 命令 | 结果 |
|---|---|
| `flutter analyze` | 首轮 1 条 info（`session_page.dart` `unnecessary_underscores`，Flutter 3.47 新增 lint）→ 修复后 **No issues found!**（5.3 s） |
| `flutter test test/touch_mapper_test.dart` | **10/10 通过**（单击序列、长按右键含左键补抬、拖动、滚轮折算与符号、滚动/缩放判定） |
| `flutter test`（全量） | **13/13 通过**（0.00 s 级，宿主无设备依赖） |
| 最终 APK（含触摸手势改动） | `flutter build apk --debug --target-platform android-arm64` **13 s**；93,389,750 B；sha256 `5e47f7a74e1b85f09beaf272ebf93c51ee13136ecdbbbda80dfff14aec28ca90`；`unzip -l` 仍含 `lib/arm64-v8a/librdc_bridge.so`（1,141,680 B） |

### 2.7 本轮（MediaCodec + 文本协议）构建与测试 —— 【已验证】

| 命令 | 结果 |
|---|---|
| `cargo test -p rdc-input -p rdc-protocol -p rdc-bridge` | **rdc-input 28/28、rdc-protocol 37/37、rdc-bridge 10/10** 全部通过（新增文本线格式、畸形载荷、能力位协商、DTO 边界用例） |
| `cargo check -p rdc-input --target x86_64-pc-windows-msvc --tests` | **通过**（0.68 s，含 `windows.rs` 的 UNICODE 注入与 3 条新单测的编译） |
| `flutter analyze` | **No issues found!**（1.6 s） |
| `flutter test`（全量） | **30/30 通过**（新增文本差分/分段 11 条、解码通道契约 6 条） |
| Kotlin JVM 单测 `:app:testDebugUnitTest` | **9/9 通过**（`AnnexBParserTest`，0.031 s，`failures=0 errors=0`） |
| APK 构建 | `flutter build apk --debug --target-platform android-arm64` **10.4 s**（增量）→ `app/build/app/outputs/flutter-apk/app-debug.apk`，**79,031,817 B**，sha256 `12f8a422250e1a17358b02ff1bc8dff4cb57879a4ef647826ec31fd1d353e61b` |
| APK 内容 | `unzip -l`：`lib/arm64-v8a/librdc_bridge.so` **1,144,608 B**（新协议已打入），`classes*.dex` 含 Kotlin 插件 |

### 2.8 本轮（阶段 3 接线）构建与测试 —— 【已验证】

| 命令 | 结果 |
|---|---|
| `flutter analyze` | **No issues found!**（1.6 s，0 条 info/warning） |
| `flutter test`（全量） | **40/40 通过**（新增 `annexb_test.dart` 5 条、`session_controller_android_test.dart` 5 条） |
| Kotlin JVM 单测 `:app:testDebugUnitTest` | **BUILD SUCCESSFUL**（`AnnexBParserTest` 9 条 + 新增 `DecoderGateTest` 3 条，无失败） |
| 离线码流判据 | 宿主单测：非 IDR 先到 → mock `push` 不收到；SPS/PPS/IDR → 推第 1 单元；IDR 后 P 帧 → 推第 2 单元；顺序与 `nalTypes` 均有断言 |
| 能力位判据 | 宿主单测：默认（无 bit13）输入法提交 → `gateway.pushInput` 0 次；置 bit13 → 1 条 `kind=text`；再撤销 → 0 次 |
| APK 构建 | `flutter build apk --debug --target-platform android-arm64` **12.2 s**；**94,614,894 B**；sha256 `293ab42fc6db277589bb19d17997ec5cb143e3158893ce89972bfd91292a4068`；`lib/arm64-v8a/librdc_bridge.so` **1,144,608 B** |
| 未改 Rust | 本轮仅 Dart/Kotlin/文档，未触碰 `crates/`，Rust 测试沿用 2.7 的 75/75 |

### 2.9 本轮（阶段 4 真实视频传输）构建与测试 —— 【部分已验证，VM 已关机】

| 命令 | 结果 |
|---|---|
| `cargo test -p rdc-protocol -p rdc-bridge` | **rdc-protocol 44/44、rdc-bridge 13/13** 全部通过（新增 `video` 协议 7 条、`video_service` 桥接 3 条） |
| `flutter analyze` | **No issues found!**（1.7 s） |
| `flutter test`（全量） | **41/41 通过**（新增「桥接轮询：元数据变化触发重配并重置 IDR 门控」1 条） |
| Kotlin JVM 单测 `:app:testDebugUnitTest` | **BUILD SUCCESSFUL**（Kotlin 未改，命中缓存，9+3 条沿用） |
| `flutter_rust_bridge_codegen generate` | **Done!**（新增 `sessionLocalCapabilities` / `sessionSetPeerCapabilities` / `sessionNegotiatedCapabilities` / `videoIngest` / `videoPollFrame` / `videoStats` / `videoReset` 与 3 个 DTO；u64 → Dart `BigInt`） |
| APK 构建 | ⚠️ **不作为通过项**：构建命令成功，但本轮同步时 `rsync --delete` 清空了 `app/android/app/src/main/jniLibs/`，产物未包含重新编出的 `librdc_bridge.so`；随后 VM 关机，无法补编与校验。**上次已验证 APK 仍是阶段 3 的 94,614,894 B / sha256 `293ab42f…`** |

---

## 三、阶段 2 现状（代码完成，设备侧未验证）

| # | 现状 | 阻塞 |
|---|---|---|
| B1 MediaCodec | **代码完成**：`MediaCodec(video/avc)` → `TextureRegistry.SurfaceProducer` 外部纹理，GPU 直出（`releaseOutputBuffer(render=true)`，无像素回读）；低延迟参数、SPS/PPS 懒配置、队列过深丢到 IDR、p50/p99 统计（最近 256 帧） | 无真机；延迟/帧率/花屏无法实测 |
| B2 触摸→鼠标 | **已完成**：点击/拖动沿用既有指针路径；双指滚动→滚轮（40 本地像素=1 格）、长按→右键，映射纯函数见 `app/lib/model/input/touch_mapper.dart`，10 条单测全通过 | 命中精度/手感需真机 |
| B3 软键盘/IME | **代码完成**：协议 tag4（删除 N × UTF-16 码元 + 插入 UTF-8 文本）+ 能力位 bit13；Windows `SendInput(KEYEVENTF_UNICODE)`；Dart `TextInputConnection` 差分与 1348 字节分段；兼容性说明见 `research/android-s9-text-input-compat.md` | 真机端到端上屏未验证 |
| B4 横竖屏/前后台 | **代码完成**：旋转只改 `SurfaceProducer.setSize` 与坐标映射，`textureId` 不变；前后台 `pause` 释放 MediaCodec/Surface、`resume` 等待新 Surface 与下一个 IDR | 真机切换未验证 |
| B5 稳定性 | 未验证 | 无设备 |

### 3.1 解码器设计的实测边界

- 本 SDK（Flutter 3.47.9 引擎 `1.0.0-89c39edf...`）**没有** `SurfaceTextureEntry.markTextureFrameAvailable()`；`TextureRegistry` 只提供 `createSurfaceProducer()` / `registerSurfaceTexture()`。实现改用 `SurfaceProducer` + 每帧 `scheduleFrame()`。
- 代价说明：`SurfaceProducer` 在 API 29+ 由引擎实现为 HardwareBuffer/ImageReader 路径，**仍然没有 CPU 像素回读**，但比纯 SurfaceTexture 多一次 GPU 纹理拷贝；这是本 SDK 的唯一外部纹理入口，不是本项目选择了 ImageReader。
- 低延迟：`KEY_LOW_LATENCY`（API 30+）、`KEY_OPERATING_RATE`/`KEY_PRIORITY`（API 23+）；`configure` 失败时自动退回不带低延迟参数的路径并记录 `lastError`。
- 统计口径：p50/p99 是「入队 → releaseOutputBuffer(render=true)」的本地耗时，**不含网络传输**；无设备时数值无法产生。

### 3.2 阶段 3 现状（接线完成，传输/设备仍缺）

| # | 现状 | 阻塞 |
|---|---|---|
| C1 共用骨架 | **已完成**：`SessionController` 只有一份 `startPreview/stopPreview/endSession/_startInput/_stopInput/_refreshStatus`；Android 只分叉 `_startAndroidVideo`，Windows/Linux 保留 `_startDesktopPreview` | 无 |
| C2 textureId | **已完成（宿主契约）**：`_startAndroidVideo` 调 `AndroidVideoDecoder.create()` 写入 `textureId`；`session_page.dart` 未改，继续用现有 `Texture` | 真机 `Texture` 是否出画面属 D3 |
| C3 视频字节流 | **Dart 接线已完成**：`VideoFrameChannel` → `AnnexBFrameParser` 切分 → `AnnexBGate` → 串行 `AndroidVideoDecoder.push`；离线码流单测通过 | **真实传输层未实现**：目前没有网络视频通道把被控端码流写进 `VideoFrameChannel`，也没有流元数据（宽/高/fps）来源 |
| C4 IDR 门控 | **已完成**：Dart `AnnexBGate` + Kotlin `FrameGate`（`H264Decoder` 使用），两侧单测通过 | 真机 MediaCodec 从 IDR 起播的实际表现属 D3 |
| C4b 文本门控 | **已完成**：`setNegotiatedCapabilities(bits)` 读 bit13；`RemoteTextInput` 编辑默认被挡，置位后才进 `pushInput` | **协商调用点缺失**：传输层尚未把协商位图传给控制器，生产环境默认仍为 0 |
| C5 兼容结论 | **已完成**：`research/android-s9-text-input-compat.md` §2.1 与 §3 表格补齐（含现有 Windows 主控端、旧 Android 客户端、显式 opt-in 组合） | 无 |

### 3.3 阶段 4 现状（协议 + 桥 + Dart 接收完成，传输层仍缺）

| # | 现状 | 阻塞 / 备注 |
|---|---|---|
| D1 元数据协议 | **已完成**：`crates/rdc-protocol/src/video.rs` 定义 Meta/Frame/Fragment/Done（0x61–0x64）与 `VideoStreamConfig`；编解码严格校验长度与上限；7 条单测 | 无 |
| D2 分片/重组 | **已完成**：≤1352 字节分片、`frame_id`/`pts_us`(微秒)/`total_len`/`frag_count`；乱序按 `frag_index` 重排，重复分片留先到 | 无 |
| D3 丢包等下一个 IDR | **已完成**：帧号空洞置 `needs_keyframe`，非 IDR 整帧丢弃，IDR 到达恢复；`resyncs` 计数；协议层与桥层各有单测 | 真实网络丢包/抖动未实测 |
| D4 元数据变化重建 | **已完成**：`config_id`/尺寸变化 → `ConfigChanged` 清空槽位；Dart `_applyRemoteConfig` 重建解码器并重置 IDR 门控，单测覆盖 | 真机重建画面未验证 |
| D5 桥接订阅 API | **已完成**：`video_ingest`/`video_poll_frame`/`video_stats`/`video_reset`（`video_service.rs`，队列上限 8）；坏消息只记账不 panic | 无 |
| D6 Dart 吃到桥接帧 | **已完成**：8 ms 轮询 → `pollVideoFrame` → `_applyRemoteConfig` → `VideoFrameChannel.push` → `AnnexBFrameParser` → `AnnexBGate` → `AndroidVideoDecoder.push`；宿主单测通过 | 真机解码未验证 |
| D7 协商位回填 | **已完成**：会话建立后与每秒刷新都调用 `sessionNegotiatedCapabilities()` 写入 `setNegotiatedCapabilities`；`local_capabilities()` 显式 opt-in bit13；单测断言握手后 `canSendText=true`、能力位 0 时不发文本 | **真实握手注册报文尚未接入**：`session_set_peer_capabilities` 没有网络层调用点 |
| D8 端到端判据 | **部分完成**：Rust 侧「协议分片→桥重组」单测 + Dart 侧「桥 DTO→切分→门控→解码器桩」单测；FFI 边界由 FRB 生成代码的编译通过覆盖 | **APK 重新打包未验证**（VM 关机 + jniLibs 被同步清空） |

**停止点**：阶段 4 的协议、桥接订阅、Dart 接收路径、协商位回填与宿主判据全部完成（Rust 44+13 通过、Flutter 41/41 通过、analyze 干净）。因 Ubuntu 虚拟机已关机，**APK 重新打包与 Kotlin 产物的重新校验留到下一次开机**；判据 D8 中「FFI 打包进 APK」保持未验证。

**下一块起点**：(1) VM 开机后重建 `librdc_bridge.so`（`packaging/android/build_apk.sh`）并重跑 `:app:testDebugUnitTest` + APK 校验；(2) 把真实网络层（媒体包收发）接到 `video_ingest`，把握手注册报文接到 `session_set_peer_capabilities`。


---

## 四、复现步骤（他人可照做）

```bash
# 1) 构建机准备（Ubuntu 22.04）
rustup target add aarch64-linux-android
cargo install cargo-ndk --locked
export ANDROID_NDK_HOME=$HOME/Android/Sdk/ndk/28.2.13676358

# 2) 拉取分支
git fetch && git worktree add ../RDC-android android-client

# 3) 一键出包（Rust .so + Flutter APK）
bash packaging/android/build_apk.sh

# 4) 安装（需真机，打开「允许未知来源」）
adb install -r dist/RDC-0.1.0-android-arm64-v8a-debug.apk
# 5) 观察自检
adb logcat -s flutter | grep AndroidSelfCheck
# 期望：rdc.AndroidSelfCheck: PASS: rdc-bridge selfcheck OK os=android arch=aarch64 ...
```

---

## 五、失败与阻塞记录

| # | 项 | 状态 | 原因 / 后续 |
|---|---|---|---|
| 1 | 模拟器 | 不可用 | VM 无 `/dev/kvm`（前述报告已实测），不尝试软件模拟 |
| 2 | `x86_64-pc-windows-msvc` 目标首次安装 | 失败一次 | 两个 rustup 进程并发导致 `.partial` 重命名失败；串行重试后安装成功，`cargo check` 通过 |
| 3 | Kotlin 首次编译 | 失败 | SDK 无 `markTextureFrameAvailable`、`ActivityPluginBinding.getLifecycle()` 返回 `Object`；已改用 `SurfaceProducer.scheduleFrame()` 与 `Activity as LifecycleOwner`，复建通过 |
| 4 | 阶段 4 同步误删 jniLibs | 阻塞（待 VM 开机） | `rsync --delete` 把构建机上的 `app/android/app/src/main/jniLibs/` 清空，产物缺 `librdc_bridge.so`；VM 已关机无法补编。**代码与宿主判据不受影响**，下次开机执行 `packaging/android/build_apk.sh` 重编并复验 APK |

## 六、设备未验证清单（不计入通过项）

> 以下全部**没有**真机/模拟器证据，不得当作已通过。

| # | 项 | 为什么没验证 | 需要什么才能验证 |
|---|---|---|---|
| D1 | APK 安装与启动 | `adb devices` 为空 | 一台 arm64 Android 8.0+ 真机 + USB 调试 |
| D2 | `librdc_bridge.so` 在 Android 运行时 dlopen | 依赖 D1 | 真机 logcat（`rdc.AndroidSelfCheck`） |
| D3 | MediaCodec 实际出画面、不花屏 | 依赖 D1 | 真机 + 一段已知 H.264 Annex-B 码流 |
| D4 | 解码延迟 p50/p99、帧率、丢帧 | 依赖 D1；统计代码已就绪 | 真机跑 60 s，读 `stats()` |
| D5 | 30fps 稳定 | 依赖 D1 | 真机 + 被控端视频通道 |
| D6 | IME 中文/emoji 端到端上屏 | 依赖 D1 + Windows 被控端 | 真机输入 → 协议 → Windows `SendInput` |
| D7 | 100 次点击偏差 ≤4px | 依赖 D1 | 真机与被控端联合测量 |
| D8 | 旋转不重建解码器、前后台不崩 | 依赖 D1 | 真机旋转/切后台连续操作 |
| D9 | 连续 5 分钟不崩 | 依赖 D1 | 真机长跑 |
| D10 | 真实网络视频链路（丢包/乱序后等 IDR、元数据变更重建） | 依赖 D1 + 真实媒体传输 | 真机 + 被控端 + 可控丢包工具（如 `tc netem`） |

