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

### 阶段 5 · 媒体网络层与握手机制（判据先落盘）

| # | 判据 | 判定方式 |
|---|---|---|
| E1 | 加密封装关系先设计后实现 | `research/android-s9-video-transport.md` 写清「媒体分片+S4 头/AEAD 组合、nonce 方向隔离、认证范围、为何不复用控制通道」 |
| E2 | 媒体包可解封装并解密 | `video_ingest_datagram`：32 字节头 + AEAD 解密（AAD=整头）→ 视频消息；被篡改包 `auth_failures` 计数且不入队 |
| E3 | 未安装密钥即拒绝 | 无密钥时密文包按 `key_missing` 丢弃，不静默接受明文 |
| E4 | 时间戳映射 | 帧头 `pts_us` 与包头 `ts_us` 不一致时以包头为准，`ts_mismatches` 递增；输出帧 `pts_us == ts_us` |
| E5 | 握手位图真正回填 | `session_ingest_handshake(HELLO/HELLO_ACK)` 写入对端位图；旧端无 caps 时回退 `DEFAULT`（bit13/bit14 均不生效） |
| E6 | 位变化触发重新协商 | 位图实质变化时 `session_negotiated_epoch` 单调递增；相同位图重复写入不递增 |
| E7 | 位不匹配文本不发 | 协商结果缺 bit13 时 Dart 侧 `RemoteTextInput` 被门控（沿用阶段 3 判据，位图来源改为握手） |
| E8 | 视频能力位显式 opt-in | 新增 bit14 `VIDEO_STREAM`；`DEFAULT` 不含该位；`VIDEO_ENDPOINT` 含 bit13+bit14；单端支持协商后仍为 0 |
| E9 | 多码流/分辨率切换语义 | `select_stream` 只接收选定 `stream_id`；切换清空在途分片并重新等 IDR；分辨率切换走新 `Meta`（config_id 递增） |
| E10 | Windows 主控端 bit13 注册点 | `demo.rs` / `s4demo.rs` / `walldemo.rs` 的主控端注册改用 `Capabilities::CONTROLLER` |
| E11 | 兼容性说明 | 设计文档 §7 表格覆盖旧端对 bit14/握手 caps/密文包/未知 tag 的行为 |

#### 阶段 5 结果（2026-11-08，VM 已恢复）

| 判据 | 结果 | 证据 |
|---|---|---|
| E1 | 通过 | [research/android-s9-video-transport.md](file:///C:/Users/Administrator/Desktop/RDC-android/research/android-s9-video-transport.md) |
| E2 | 通过（宿主测试） | `cargo test -p rdc-bridge`：`datagram_roundtrip_reaches_video_queue`、`tampered_datagram_is_rejected` |
| E3 | 通过（宿主测试） | `missing_key_is_counted`：无密钥 → `key_missing=1` |
| E4 | 通过（宿主测试） | `timestamp_mismatch_is_overridden_by_header`：输出 `pts_us == 12_345`、`ts_mismatches=1` |
| E5 | 通过（宿主测试） | `handshake_capabilities_are_applied_and_epoch_changes`；rdc-security `handshake_carries_capabilities_both_ways` |
| E6 | 通过（宿主测试） | 同上：位图变化 epoch+1，重复写入 epoch 不变 |
| E7 | 通过（沿用阶段 3 单测） | Flutter `能力位为 0 时输入法编辑不进入输入链` 等 3 条 |
| E8 | 通过（宿主测试） | rdc-protocol `video_stream_requires_both_sides`（45 项全过） |
| E9 | 通过（宿主测试） | `stream_filter_drops_other_streams`；切换码流清空分片 |
| E10 | 通过（编译级） | `demo.rs`/`s4demo.rs`/`walldemo.rs` 主控端注册改 `Capabilities::CONTROLLER` |
| E11 | 通过 | 设计文档 §7 |

聚合：`cargo test -p rdc-protocol -p rdc-security -p rdc-bridge` = 45 + 28 + 21 全通过；
`flutter analyze` 无问题；`flutter test` 41/41 通过。
APK 复验见 §2.10。

**阶段 5 停止点**：媒体包→`video_ingest` 的解封装/AEAD/时间戳映射已落地并有宿主单测；
握手 caps 回填与视频能力位/多码流语义已落地。**真实 socket 归属、Android 网络客户端仍缺**，
因此“模拟器客户端连 VM 被控端”的端到端仍无法跑（见 §2.10 与三类缺项）。

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

### 2.10 本轮（阶段 5 媒体网络层 + VM/模拟器复验）—— 【已验证，E2E 仍阻塞】

| 命令 / 动作 | 结果 |
|---|---|
| `cargo test -p rdc-protocol -p rdc-security -p rdc-bridge` | **rdc-protocol 45/45、rdc-security 28/28、rdc-bridge 21/21** 全部通过 |
| `flutter_rust_bridge_codegen generate` | **Done!**（新增 `sessionIngestHandshake` / `sessionNegotiatedEpoch` / `videoIngestDatagram` / `videoSetMediaKey` / `videoClearMediaKey` / `videoMediaStats` / `videoSelectStream` + `MediaTransportStatsDto`） |
| `flutter analyze` | **No issues found!**（12.9 s） |
| `flutter test`（全量） | **41/41 通过** |
| Kotlin JVM 单测 `:app:testDebugUnitTest` | **BUILD SUCCESSFUL in 4m 51s**；报告 `AnnexBParserTest 9/9`、`DecoderGateTest 3/3`，failures=0（时间戳 2026-11-07T18:42Z，非缓存命中） |
| arm64 APK（`packaging/android/build_apk.sh` 同参数） | **成功**：`app-debug.apk` 76 MB，内含 `lib/arm64-v8a/librdc_bridge.so`（1,252,192 B）与 `lib/arm64-v8a/libflutter.so` |
| x86_64 模拟器 APK | **成功**（VM 临时把 `abiFilters` 加 `x86_64`，仅用于模拟器，不改仓库）：内含 `lib/x86_64/libflutter.so`、`lib/x86_64/librdc_bridge.so`（1,418,152 B） |
| 模拟器安装/启动 | `emulator-5554`：`INSTALL_SUCCESS`；`com.rdc.rdc` pid 存活；首页徽标显示 **“桥接正常 / 构建平台 android-x86_64”**，即 Rust FFI 已加载并被调用（logcat 无显式 dlopen 行，**UI 状态是唯一证据**） |
| 横竖屏 | `user_rotation=1` → 截图确认整体旋转、预览区与统计行正常，进程不崩；**因无活跃解码器，`textureId` 是否变化未测** |
| 5 分钟空闲稳定性 | 见下方“稳定/性能”小节（仅空闲，非推流） |
| **真实端到端（模拟器客户端连 VM 被控端）** | ❌ **无法执行**：Android 侧只有 `sessionStart` 回环实现，`app/lib` 中不存在任何 UDP/QUIC 客户端或主机地址输入；Rust 桥也仅暴露 `video_ingest_datagram` 而没有 socket 所有者。属“缺代码”，不是环境问题 |

**稳定/性能（空闲，非推流）**：`dumpsys meminfo com.rdc.rdc`
基线 PSS 253,145 KB / RSS 324,424 KB → 5 分钟后 PSS 262,475 KB / RSS 335,272 KB
（+9.1 MB / +10.6 MB，pid 7204 未变，logcat 无 FATAL、无 ANR）。这只能证明“启动后空闲不泄漏”，
**不能替代推流稳定性**。fps/解码延迟 p50/p99、丢帧、触摸回读、中文 IME、textureId 稳定性
全部依赖真实会话，本轮未覆盖，不写入通过项。

**构建产物清理（本轮收尾）**：删除 VM 侧 `target/` 3.56 GB、`app/build/` 0.75 GB、
`app/android/.gradle/` 1.3 MB、`~/.gradle/caches/` 2.81 GB；根分区已用
43,002,159,104 → 36,382,695,424 B，**释放 6,619,463,680 B（约 6.16 GiB）**。
`jniLibs/{arm64-v8a,x86_64}/librdc_bridge.so` 保留。

---

### 2.11 阶段 6 判据（socket 所有权 + Android 直连客户端）—— 【先落盘，后写代码】

范围：只做「手动 `IP:端口` 直连」，不做信令/账号、不做配对校验。对端为
Linux 被控端 `rdc-serve` 的 **S8-2 明文 TCP 协议**（`kind 1..4`，见
`RDC-linux/research/android-connect-linux.md`）；本轮对生产协议零改动。

| # | 判据 | 阈值 / 判定方式 | 类型 |
|---|---|---|---|
| G1 | 直连页可用 | 输入 IP + TCP 端口（默认 45900），无配对码输入；失败必须给出 `CODE: 人话`（如 `E_CONNECT_REFUSED: 连接 … 失败`） | 宿主可测 |
| G2 | socket 所有者 | TCP 套接字只在 Rust 侧创建与持有（`rdc-bridge/src/direct_session.rs`）；Dart 无 socket 调用 | 静态 |
| G3 | 不改协议 | `git diff crates/rdc-protocol` 为零；线上只出现 kind 1..4；不新增配对/握手报文 | 静态 |
| G4 | 媒体链路 | `kind=VIDEO` 整帧 → 本地 Frame/Fragment 适配 → `video_service` 重组 → MediaCodec 上屏；不重写线格式 | 静态 + 单测 |
| G5 | 接收有界 | 接收线程不碰 UI；帧队列 16 MiB + 64 条双上限，满即丢最旧并累加 `frames_dropped`；单帧超预算直接丢 | 单测 |
| G6 | 出画面 | 模拟器截图可见 VM 桌面（非黑屏/占位图），首个有效帧为 IDR | 设备 |
| G7 | 上屏 fps | 对端 CONFIG 1280×800@60；客户端 10 秒窗口 `fps ≥ 25` | 设备 |
| G8 | 延迟 | 客户端侧 p50 ≤ 120 ms、p99 ≤ 250 ms（对端 pts_ns 采集时间戳 → 解码输出，含网络） | 设备 |
| G9 | 触摸（Pointer 语义） | 点击 3 个已知坐标，VM 侧 `xdotool getmouselocation` 误差 ≤ 2 px 且按键状态改变；断连/切后台补 Up | 设备 |
| G10 | 中文输入 | IME 组合文本经 `kind=INPUT` tag4 上送，VM 侧可回读同一字符串。**已知风险**：Linux 注入器对 `InputEvent::Text` 返回 `Unsupported`（属 Linux 线），若确认不支持则记「阻塞在 Linux 注入器」 | 设备 |
| G11 | 横竖屏 | 旋转后 `textureId` 不变；CONFIG 变化触发一次重配并等待新 IDR，画面比例正确 | 设备 |
| G12 | 5 分钟真推流 | 连续动态桌面 5 分钟：无崩溃/ANR；客户端 RSS 增长 ≤ 50 MB；`frames_dropped/frames_received ≤ 5%` | 设备 |
| G13 | 收尾 | 断开发送 `kind=4 BYE` 并释放 socket/线程；清理 `target/`、`app/build`、`.gradle/caches` 并记录释放字节数 | 宿主 |

**判定纪律**：G6–G12 若无设备/被控端任一侧缺失，只记「未覆盖」，不写入通过项；
数字不达标就写不达标，不取近似值。失败一律附错误码与一句人话。

#### 阶段 6 结果（执行后回填）

| # | 结果 | 证据 |
|---|---|---|
| G1–G13 | 待执行 | — |

---

### 2.12 阶段 6 构建链与连通性判据（2026-11-08 先落盘，后动手）

**环境基线（探测于 2026-11-08，本机 Windows）**

| 项 | 实测状态 | 证据命令 |
|---|---|---|
| Flutter | 3.47.9，位于 `C:\dev\flutter`，**未进 PATH** | `C:\dev\flutter\bin\flutter.bat --version` |
| Rust | 1.99.1，位于 `C:\Users\Administrator\.cargo\bin`，仅装 `x86_64-pc-windows-msvc` | `rustc --version` / `rustup target list --installed` |
| Android SDK | **缺失** | `flutter doctor -v` → `X Unable to locate Android SDK` |
| JDK | **缺失**（无 `java.exe`，`JAVA_HOME` 为空） | 全盘搜索 `java.exe` 无结果 |
| adb / 设备 | **缺失**，`flutter devices` 只有 Windows/Chrome/Edge | `flutter devices` |
| cargo-ndk / aarch64-linux-android | **缺失** | `cargo ndk` → `no such command` |
| Gradle 缓存 | 无 `~/.gradle`，wrapper 需联网下载 9.3.1 | `Get-ChildItem ~/.gradle` |
| 网络 | `maven.google.com` 超时 | `flutter doctor -v` 网络项 |
| 磁盘 | C 盘 13.68 GB 空闲 | `Get-PSDrive C` |
| 历史构建机 | Ubuntu 22.04 VM `192.168.37.130`（SDK 36 / NDK 28.2.13676358 / JDK 17 / cargo-ndk 4.1.2） | §2.10 记录；当前 `vmrun list` 为 0、ping 不通 |
| 现存 APK | `dist/app-debug-s9-stage3.apk` 94,614,894 B，2026-11-07 22:05，不含阶段 6 代码 | `Get-ChildItem dist` |

**构建判据（B 组）**

| # | 判据 | 阈值 / 判定方式 | 可在本机执行 |
|---|---|---|---|
| B1 | Rust 宿主编译 | `cargo check -p rdc-bridge`（x86_64-pc-windows-msvc）退出码 0，无 error | 是 |
| B2 | Rust 单测 | `cargo test -p rdc-protocol -p rdc-bridge` 全绿，`direct_session` 6 条在内 | 是 |
| B3 | Dart 静态 | `flutter analyze` 输出 `No issues found!` | 是 |
| B4 | Dart 单测 | `flutter test` 全绿 | 是 |
| B5 | arm64 APK 产出 | `flutter build apk --debug --target-platform android-arm64` 退出码 0，产物存在且 `unzip -l` 含 `lib/arm64-v8a/librdc_bridge.so` | **否**（需 SDK/JDK/NDK） |
| B6 | APK 可安装 | 设备上 `adb install` 返回 `Success`，启动后 `pidof com.rdc.rdc` 非空 | **否**（需设备） |

**连通性判据（C 组，对应「地址输入 → 连接 → 画面接收 → 输入回传」四环）**

| 环节 | 判据 | 类型 |
|---|---|---|
| C1 地址输入 | 直连对话框可输入 IP 与端口（默认 45900），空 IP / 端口越界给出字段级错误 | 宿主静态 + 页面 |
| C2 连接 | `direct_connect` 失败必须返回 `CODE: 人话`（`E_ARGS`/`E_RESOLVE`/`E_CONNECT_REFUSED`/`E_CONNECT_TIMEOUT`/`E_CONFIG_TIMEOUT`），页面同时写占位区与提示条 | 宿主单测 + 设备 |
| C3 画面接收 | 收到 CONFIG 后 `configured=true`；`kind=VIDEO` 经重组进入解码器；设备上截图可见被控端桌面（非黑屏） | 设备 |
| C4 输入回传 | 触摸经 `kind=INPUT` 上送；被控端 `xdotool getmouselocation` 误差 ≤ 2 px；断连补 Up | 设备 |
| C5 收尾 | 断开释放 socket/接收线程，重复调用安全；`frames_dropped/frames_received ≤ 5%` | 单测 + 设备 |

**判定纪律**：B5/B6/C3/C4 任一所需环境缺失时只记「未覆盖」并写明缺什么，不得记为通过；不得为出包而放宽版本或签名要求。

#### 阶段 6 构建/连通结果（执行后回填）

| # | 结果 | 证据 |
|---|---|---|
| B1 | **通过**：`cargo check -p rdc-bridge` 退出码 0，耗时 1m26s；4 条 `dead_code` 警告（`video_service::snapshot_lock`、`video_transport::{build_datagram,build_plain_datagram,reset_stats}`），其中 `build_plain_datagram` 为本次重构新增且仅被测试路径间接引用 | 2026-11-08 本机执行 |
| B2 | **通过**：`cargo test -p rdc-protocol -p rdc-bridge` → `rdc-bridge 27/27`（含 `direct_session` 6 条）、`rdc-protocol 45/45`，0 failed | 同上 |
| B3 | **通过**：`flutter analyze` → `No issues found!`（21.1 s） | 同上 |
| B4 | **通过**：`flutter test` → `41/41 All tests passed!` | 同上 |
| B5 | **通过（VM 构建）**：Ubuntu VM `192.168.37.130` 执行 `packaging/android/build_apk.sh` 退出码 0（debug 签名），产出 `dist/RDC-0.1.0-android-arm64-v8a-debug.apk` 79,232,753 B，APK 内含 `lib/arm64-v8a/librdc_bridge.so`；SHA-256 已本地复算 | VM `/home/wuge/build.log` |
| B6 | **未通过（环境 ABI 卡点）**：VM 内 x86_64 模拟器 `rdc_a33` 安装报 `INSTALL_FAILED_NO_MATCHING_ABIS`（APK 仅 `arm64-v8a`，模拟器 `ro.product.cpu.abilist` 仅 `x86_64`）；`pidof=3417` 属于上一轮遗留的 x86_64 旧包（`lastUpdateTime=05:17:54`），**不得作为新包启动证据** | `adb install` / `dumpsys package` |
| C1 | **代码就绪**：直连对话框含 IP/端口字段与字段级校验（端口 1..65535），未做设备验证 | `session_page.dart` |
| C2 | **宿主已验证**：连接被拒/超时/无 CONFIG/参数校验 4 条异常路径由 D2–D5 实跑通过；跨机真实网络未覆盖 | §2.13 D 组 |
| C3 | **未覆盖**：需被控端 `rdc-serve` + 设备；本轮只做宿主编译 | — |
| C4 | **未覆盖**：需设备与被控端；已知风险为 Linux 注入器对 `InputEvent::Text` 返回 `Unsupported`（G10） | — |
| C5 | **宿主已验证**：D1 实测断开后 `connected=false`、BYE 送达服务端、重复 `direct_disconnect()` 不 panic；socket/线程回收由 `join` 阻塞返回证明 | §2.13 D1 |

**本轮构建产物占用**：`target/` 1.40 GB、`app/build/` 0.05 GB；C 盘空闲 13.68 GB → **8.85 GB**。
**出包能力结论**：宿主侧四类检查（B1–B4）全部通过；APK（B5/B6）在本机**不可执行**，不记通过。

---

### 2.13 宿主回环联调判据（D 组，2026-11-08 先落盘，后动手）

范围：用本地 `TcpListener` 扮演 `rdc-serve`，只验证 Rust 直连会话层的线格式与状态机；
**不含** Android 解码上屏、真机触摸命中、跨机时钟延迟（这三项保持「未验证」）。

| # | 判据 | 阈值 / 判定方式 | 覆盖点 |
|---|---|---|---|
| D1 | 正常全链路 | 假服务端：accept → 发 `kind=3 CONFIG(1280×800@60)` → 发一帧 `kind=1 VIDEO(IDR, flags=0x01)` → 读客户端输入 → 读 `kind=4 BYE`。客户端：`direct_connect` 返回 `connected=true, configured=true, 1280×800@60`；`frames_received ≥ 1`、`frames_dropped = 0`、`ingest_errors = 0`；`video_poll_frame()` 取出的帧为 `keyframe=true` 且字节与发送一致；`direct_send_input` 返回 1 且 `input_sent = 1`；`direct_disconnect()` 后服务端收到 `kind=4`；断开后 `direct_status().connected = false`；再次 `direct_disconnect()` 不 panic | C2/C5 |
| D2 | 连接被拒 | 先 bind 再 drop 的本地端口，`direct_connect` 返回 `E_CONNECT_REFUSED: …` | C2 |
| D3 | 连接超时 | `direct_connect("192.0.2.1", 45900)`（RFC 5737 不可路由）返回 `E_CONNECT_TIMEOUT: …`，耗时约 3 s；若宿主网络策略立即返回不可达等其它码，**如实记录实际码并记该项未通过**，不改判据 | C2 |
| D4 | 未收到 CONFIG | 假服务端 accept 后保持沉默，`direct_connect` 在 8 s 窗口后返回 `E_CONFIG_TIMEOUT: …`（实测耗时 7.5–10 s） | C2 |
| D5 | 参数校验 | 空 host → `E_ARGS`；`port=0` → `E_ARGS`（均在发起连接前返回） | C2 |

**判定纪律**：D 组全部为宿主可测，必须实跑并留原始输出；任一条失败只记失败与实际错误码，不得改为「通过」或删用例。D1 通过也只能证明「Rust 会话层线格式与状态机可用」，不替代设备侧 C3/C4。

#### D 组结果（执行后回填）

**执行命令**：`cargo test -p rdc-bridge --test direct_session_loopback -- --test-threads=1 --nocapture`
**原始输出**：`running 5 tests` → `5 passed; 0 failed; 0 ignored`，`finished in 13.22s`，退出码 0。

| # | 结果 | 证据 |
|---|---|---|
| D1 | **通过** | 连接→CONFIG→VIDEO→输入→BYE 全链路断言全绿：`configured=true`、`1280×800@60`、`frames_received≥1`、`frames_dropped=0`、`ingest_errors=0`、重组帧字节与发送逐字节一致且 `keyframe=true`、`video_stats().completed=1`、`direct_send_input` 返回 1、服务端读到非空 `kind=2` 载荷、断开后 `connected=false` 且服务端收到空载荷 `kind=4`、二次断开无 panic |
| D2 | **通过** | bind 后立即 drop 的本地端口 → 返回 `E_CONNECT_REFUSED: 连接 … 失败` |
| D3 | **通过** | `192.0.2.1:45900` 实测走满 3 s 超时窗口并返回 `E_CONNECT_TIMEOUT`（本机网络策略未提前拒绝） |
| D4 | **通过** | 只监听不 accept 的本地端口 → 8 s 后返回 `E_CONFIG_TIMEOUT`，实测耗时落在 7.5–10 s 判定窗 |
| D5 | **通过** | 空 host、空白 host、`port=0` 均返回 `E_ARGS` |

**结论**：C2/C5 从「部分覆盖」升为「宿主已验证（Rust 会话层）」。
**仍保持未验证**：真机/模拟器上的画面解码上屏（C3）、触摸命中与中文 IME 回读（C4）、跨机时钟延迟 p50/p99。

---

### 2.14 2026-11-08 VM 构建记录（guard 全程留痕）

| 项 | 实测 |
|---|---|
| 守卫 1 · 宿主可用内存 | **21.57 GB**（阈值 4 GB，通过），总内存 31.91 GB |
| VM | `Desktop\Ubuntu 22.04 64位`，启动后 `vmrun getGuestIPAddress` = `192.168.37.130`，ping 通 |
| VM 工具链 | Flutter 3.47.9 / JDK 17.0.20.1 / SDK `platforms`+`build-tools` / NDK 28.2.13676358 / cargo-ndk + `aarch64-linux-android` / Gradle 缓存 4.6 G / 磁盘 70 G 空闲 |
| 同步方式 | 本地 `tar.gz`（排除 `.git`/`target`/`dist`/`app/build`/`.dart_tool`）1.8 MB → `/home/wuge/rdc-android-sync`（新建目录）；**未触碰** `rdc/`、`rdc-linux/`、`rdc-serve/`、`rdc-android-src/`、`rdc-shots/` 及任何系统配置 |
| 构建命令 | `su - wuge -c 'cd ~/rdc-android-sync && bash packaging/android/build_apk.sh'` |
| 结果 | 退出码 0；`Running Gradle task 'assembleDebug'… 84.4s` → `✓ Built build/app/outputs/flutter-apk/app-debug.apk` |
| 产物 | `dist/RDC-0.1.0-android-arm64-v8a-debug.apk` 79,232,753 B；`SHA-256 e10eb5754ca4e6633484892158cdf23f45d286f17cead0cfac328bd172bf72e7`（VM 端与本地复算一致） |
| 包内 ABI | 仅 `lib/arm64-v8a/`（含 `librdc_bridge.so`、`libflutter.so`） |
| **环境偏差 1** | VM 上 `flutter pub get` 将 3 个传递依赖解析到更新版本（`meta 1.18.3→1.19.0`、`vector_math 2.4.0→2.4.4`、`vm_service` 同版本），并输出 `Changed 29 dependencies`；本轮 APK 与本地 `pubspec.lock` **不完全同源**，下一轮须把 lock 固化后再出包 |
| **环境偏差 2** | Flutter 警告：app 直接应用 Kotlin Gradle Plugin，后续 Flutter 版本将构建失败，需迁移 Built-in Kotlin |
| 设备验证 | 模拟器 `rdc_a33`（x86_64，`/dev/kvm` 可用）70 s 冷启动成功；`adb install` 报 `INSTALL_FAILED_NO_MATCHING_ABIS`，三项验收（能装/能启动/能填地址连上）**全部未验证** |
| 收尾 | `adb emu kill` + `vmrun stop … soft`，`vmrun list` = 0；宿主可用内存回到 **23.25 GB** |

---

### 2.15 交付包固化与模拟器三项验收（E 组，2026-11-08）

**判据（先落盘后执行）**

| # | 判据 | 判定方式 |
|---|---|---|
| E1 | lock 固化 | 构建机 `flutter pub get` 不再改动 `app/pubspec.lock`；该 lock 提交入库；出包前后 lock MD5 不变 |
| E2 | 产物可复现 | 同一源码、同一参数连续两次清构建，APK SHA-256 相同；若不同须给出差异来源 |
| E3 | 仅 debug 出 x86_64 | `debug` 变体允许 x86_64；`release` 与 `packaging/android/build_apk.sh` 零改动；arm64 交付包在改动前后哈希不变（除真实代码变更外） |
| E4 | 三项验收 | ①`adb install` 返回 `Success`；②启动后 `pidof com.rdc.rdc` 非空且桥接正常；③输入 `192.168.37.130:45900` 后连上并持续收帧，截图可见被控端桌面 |
| E5 | 收尾 | 关闭模拟器与 VM，`vmrun list` 为 0，宿主可用内存回到启动前量级 |

**守卫与环境**：启动前宿主可用内存 **22.97 GB / 31.91 GB**（阈值 4 GB，通过）；VM 为 `Desktop\Ubuntu 22.04 64位`，`192.168.37.130`；仅同步 worktree 到 `/home/wuge/rdc-android-sync`，未改动 `rdc/`、`rdc-linux/`、`rdc-serve/`、`rdc-android-src/`、`rdc-shots/` 与系统配置。

#### E1 lock 固化

| 项 | 结果 |
|---|---|
| VM 解析 | `flutter pub get` → `Got dependencies!`（**无变更**），`app/pubspec.lock` MD5 `4c32ba67a8f4ba2707dc828008c23658` |
| 本地对照 | 原 lock 为 `meta 1.18.3 / vector_math 2.4.0` 且 URL 为 `pub.dev`；构建机已解析为 `meta 1.19.0 / vector_math 2.4.4`，URL 记为 `pub.flutter-io.cn`（构建机镜像），版本号与 `sha256` 为判定依据 |
| 提交 | `7602cb4 chore(android): 固化pubspec.lock…` |
| 出包前后 | 两次构建后 lock MD5 仍为 `4c32ba…`，未被 `flutter build` 改写 |

#### E2/E3 构建结果

| 轮次 | 构建条件 | 耗时 | SHA-256 | 大小 |
|---|---|---|---|---|
| 1 | 全新同步目录，冷构建 | 56.5 s | `e10eb575…72e7` | 79,232,753 B |
| 2 | 删除 `app/build`/项目 `.gradle`/`.cxx` 后重建 | 13.2 s（命中 Gradle 构建缓存） | `e10eb575…72e7` | 79,232,753 B |
| 3 | 加入 debug x86_64 配置后复验 arm64 | — | `e10eb575…72e7` | 79,232,753 B |
| A | 修复 FRB 生成代码后重建 | — | `03cd7752…dcd5` | 79,273,401 B |
| B | 同 A，再次清构建 | — | `03cd7752…dcd5` | 79,273,401 B |

- 轮次 1/2 哈希一致；轮次 2 命中 Gradle 构建缓存，独立性弱于轮次 1，但轮次 1 与此前独立构建的旧包哈希相同，可判「可复现」。
- 轮次 3 与轮次 1 完全相同 → **debug x86_64 配置未改变 arm64 交付包**。
- 轮次 A/B 因修复了真实代码缺陷（见下）而必然换哈希，A/B 两次仍一致；`dist/RDC-0.1.0-android-arm64-v8a-debug.apk` 已替换为 `03cd…`，旧 `e10eb…` 不再使用。
- 包内 ABI 仍仅 `lib/arm64-v8a/`（`librdc_bridge.so` + `libflutter.so`）。

**debug x86_64 改动（`app/android/app/build.gradle.kts` 第 53–70 行）**

- 新增 `buildTypes.debug { ndk { abiFilters += listOf("x86_64") } }`（第 53–65 行），含说明注释；`release` 块（第 66–69 行）与 `packaging/android/build_apk.sh` **一字未改**（`git diff packaging/` 为空）。
- 实测结论（如实记录）：Flutter 3.47.9 的 Gradle 插件在 `apply` 阶段就向 `defaultConfig` 注入 `[armeabi-v7a, arm64-v8a, x86_64]`，原写死的 `arm64-v8a` 只是并集追加，**并不能真正拦掉 x86_64**；包内实际 ABI 由 `jniLibs` 中存在哪些 `.so` 决定。因此该改动的真实作用是「把 debug 的 x86_64 意图显式化」，交付包不受影响由轮次 3 的哈希相同证明。
- x86_64 构建另需 `cargo ndk -t x86_64 … build --release -p rdc-bridge`（构建机临时操作，未写入交付脚本）。

#### 发现的真实阻塞与修复（不得绕过）

| 项 | 证据 | 处理 |
|---|---|---|
| 桥接初始化失败 | 首次 x86_64 包启动即红字「桥接异常」；`cargo ndk` 日志 `[main] Rust 桥接初始化失败`，`RustGateway 尚未初始化` 刷屏 | 定位为 FRB 生成代码漏提交：`app` 侧 `rustContentHash = -999264004`，`crates/rdc-bridge/src/frb_generated.rs` 仍为阶段 5 的 `1204307587`，且缺 `wire__crate__api__direct_connect_impl` |
| 修复 | 在 `app/` 执行 `flutter_rust_bridge_codegen generate`（2.13.0）→ `Done!`；Rust 侧 hash 变为 `-999264004`，新增 271 行 / 删除 44 行（含 `direct_connect` 接线）；Dart 生成文件 MD5 与原来完全一致 | 提交 `82bc512 fix(bridge): 重新生成FRB胶水…`；宿主 `cargo test -p rdc-bridge` 退出码 0（loopback 5/5） |
| 影响 | 旧 `e10eb…` 包虽能安装，但启动必失败，**不可交付**；新 `03cd…` 为可交付版本 | — |

#### E4 模拟器三项验收（x86_64，`rdc_a33`，android-33）

| 项 | 结果 | 证据 |
|---|---|---|
| 能装 | `adb install -r -t dist/RDC-0.1.0-android-x86_64-debug.apk` → `Performing Streamed Install` / `Success`；APK SHA-256 `7177e311…c517`，包内仅 `lib/x86_64/` | adb 输出；`unzip -l` |
| 能启动 | `pidof com.rdc.rdc` = `4570`；logcat `[AndroidSelfCheck] PASS: rdc-bridge selfcheck OK os=android arch=x86_64 abi=unix`、`[RustGateway] 动态库由默认加载器装载`；截图徽标「桥接正常 / android-x86_64」 | logcat；`logs/s9-shot2-bridge-ok.png` |
| 能填地址连上 | 直连对话框清空后重新输入 `192.168.37.130`、端口 `45900` → 点「连接」；logcat `[SessionController] 直连建立 192.168.37.130:45900 1280x800@60`、`Android 画面通道 textureId=1`；收帧 **589 → 1982**，丢包 **0.0%**；截图可见 Ubuntu 桌面（顶栏与左侧 Dock 图标），服务端 `rdc-serve` PID 1866（`DISPLAY=:0`，真实图形会话） | logcat；`logs/s9-shot4-typed-ip.png`、`s9-shot5-connected.png`、`s9-shot6-frames.png` |

**证据文件**（`logs/` 被 gitignore，仅本地留存）：`s9-shot1-home.png`（修复前红字桥接异常）、`s9-shot2-bridge-ok.png`、`s9-shot3-direct-dialog.png`、`s9-shot4-typed-ip.png`、`s9-shot5-connected.png`、`s9-shot6-frames.png`。

**仍未验证**：C4 输入回传（本轮未注入触摸/键盘，只验证了收帧）、真实 arm64 真机、跨机网络延迟与丢包、5 分钟长稳；模拟器为软件 GPU（swiftshader），帧率为模拟器内实测值。

#### E5 收尾

| 项 | 结果 |
|---|---|
| 模拟器 | `adb emu kill` 返回 `KO: unknown command`（该版本 console 不支持），随 VM 一并停止 |
| VM | `vmrun stop … soft` → `vmrun list` = **0** |
| 宿主内存 | 关机后可用 **23.89 GB / 31.91 GB**（启动前 22.97 GB） |

---

### 2.16 输入回传 C4 与 5 分钟长稳判据（F 组，2026-11-08 先落盘，后动手）

**F1 输入回传（判据必须落在被控端，不接受只看发送侧日志）**

| # | 判据 | 判定方式 |
|---|---|---|
| F1-a 触摸点击 | 被控端出现可观察点击结果 | 模拟器画面内点击被控端 `xev` 探测窗 → `xev` 日志出现 `ButtonPress/ButtonRelease`（button 1）；`xdotool getmouselocation` 与映射坐标误差 ≤ 2 px；`xdotool getactivewindow` 指向探测窗 |
| F1-b 键盘扫描码 | 被控端出现可观察按键结果 | 向客户端注入 `KEYCODE_A` 按下+抬起 → `xev` 记录 `KeyPress`/`KeyRelease`，`keysym='a'`、`keycode=38` |
| F1-c 中文 IME | 被控端出现中文字符 | 客户端文本编辑上送中文。若客户端 bit13 未协商或服务端 `InputEvent::Text` 返回 `Unsupported`，判**未通过**并记录两侧原始证据；不得用发送侧日志代替 |

**F2 5 分钟长稳**

| 判据 | 阈值 |
|---|---|
| 连接保持 | 连续直连 ≥ 5 分钟；`logcat` 中「直连建立」仅 1 次、无「断开/失败」新增 |
| 收帧 | 起止截图对比「收帧数」持续增长；丢包率 ≤ 1% |
| 进程 | `pidof com.rdc.rdc` 前后一致，无崩溃重启 |
| 内存 | 每分钟采样 PSS；增长 ≤ 30 MB（超出需给出实际值与判读） |
| 异常处理 | 出现断连/崩溃/黑屏立即停止测试并保留 logcat 与截图 |

**执行前置**：宿主可用内存 ≥ 4 GB；VM `192.168.37.130`；`rdc-serve` 在 `DISPLAY=:0`；模拟器 `rdc_a33`；APK 用 `7177e311…`（含 FRB 修复）。

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

