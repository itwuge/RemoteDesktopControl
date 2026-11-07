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

### 2.6 静态分析

| 命令 | 结果 |
|---|---|
| `flutter analyze` | 首轮 1 条 info（`session_page.dart:368` `unnecessary_underscores`，Flutter 3.47 新增 lint）→ 已改为 Dart 3 通配符 `_`；复测结果见提交说明 |

---

## 三、阶段 2 现状（明确未完成）

| # | 现状 | 阻塞 |
|---|---|---|
| B1 MediaCodec | 未实现 | 无真机；解码需 SurfaceTexture 外部纹理，无法静态验证出画面 |
| B2 触摸→鼠标 | 共享视图已有「指针按下/拖动/抬起→button/move」与滚轮换算（`session_page.dart`），但**没有**双指滚动→滚轮、长按右键的移动端映射 | 需补 `TouchMapper` 并接入手势；无误触验证手段 |
| B3 软键盘 | 只有硬件键扫描码；IME 文本输入未接 | 需隐藏 TextField/TextInputClient 通道 |
| B4 横竖屏 | `ViewportController` 已是唯一入口，`Texture` 只改尺寸不换 `textureId`，设计满足「只换皮不换核」；Android 尚未产生 textureId | 解码器缺失导致无纹理可测 |
| B5 稳定性 | 未验证 | 无设备 |

**停止点**：阶段 1 完成并提交；阶段 2 起点为「Kotlin MediaCodec 解码器 + Flutter 外部纹理注册」。

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

## 五、失败/未验证清单

| # | 项 | 状态 | 原因 / 后续 |
|---|---|---|---|
| 1 | 真机安装与启动 | 未验证 | `adb devices` 为空；需用户接入一台 arm64 Android 8.0+ 设备并开启 USB 调试 |
| 2 | 模拟器 | 不可用 | VM 无 `/dev/kvm`（前述报告已实测），不尝试软件模拟 |
| 3 | Android 运行时 dlopen | 未验证 | 依赖 #1；宿主 x86_64 替代证据已取得 |
| 4 | MediaCodec 出画面/延迟/帧率 | 未实现 | 阶段 2 |
| 5 | 100 次点击 ≤4px | 未验证 | 需真机与被控端联合测量 |
| 6 | 5 分钟稳定性/前后台 | 未验证 | 需真机 |
