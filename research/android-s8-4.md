# Android 客户端可运行性（S8-4 B 部分）

结论先说：**APK 未产出**。不是 Flutter 工程本身的问题，而是这台 VM 的 Android 工具链与网络在首次构建阶段卡住（Gradle 发行包需走镜像、NDK 解压后构建停住）。
下面给出已解决的问题、卡点、缺的依赖与可执行的修复步骤，以及安卓端还差的功能。

## 1. 环境事实

| 项 | 值 |
| --- | --- |
| Flutter / Dart | 3.47.9 stable / 3.13.7 |
| JDK | OpenJDK 17（`/usr/lib/jvm/java-17-openjdk-amd64`） |
| Android Gradle Plugin / Kotlin | 9.1.0 / 2.4.0（Flutter 3.47 脚手架默认） |
| Gradle wrapper | 9.3.1 `-all` |
| 原 Android SDK | `platforms;android-33`、`build-tools;33.0.2`（Flutter 要求 36） |
| NDK | 无（构建过程中被自动安装 `android-ndk-r28c`） |
| 网络（VM） | `dl.google.com` / `mirrors.cloud.tencent.com` 可达；`services.gradle.org` 的 307 重定向目标被拒 |
| KVM | 无 `/dev/kvm` ⇒ 模拟器不可用（与前情一致，不硬撑） |

## 2. 已解决的问题（已实证）

1. **SDK 版本不足** → 用 `sdkmanager` 装上：
   ```
   yes | sdkmanager "platforms;android-36" "build-tools;36.0.0"
   ```
   装完后 `$HOME/Android/Sdk/platforms`、`build-tools` 均出现 36 版本。
2. **Gradle wrapper 无法自举** → `GradleWrapperMain` 下载 `services.gradle.org/distributions/gradle-9.3.1-all.zip`
   时 307 跳转到被拒目标（`connection refused`）。
   修法（已验证有效）：把 `android/gradle/wrapper/gradle-wrapper.properties` 的
   `distributionUrl` 指到可达镜像（腾讯云镜像 `https://mirrors.cloud.tencent.com/gradle/gradle-9.3.1-all.zip`）。
   Gradle 9.3.1 daemon 成功启动（`~/.gradle/wrapper/dists/gradle-9.3.1-all/<hash>/gradle-9.3.1`）。
   > 该改动只落在 VM 的构建副本上，**仓库内文件未改**；是否入库建议由你决定（镜像地址对 CI 不通用）。
3. **NDK 缺失** → Gradle 自动下载并解压 `android-ndk-r28c`，解压到 100%。

## 3. 卡点（导致 APK 未产出）

`flutter build apk --debug` 在 NDK 安装完成后，daemon 日志再无输出（最后一条是 NDK 解压完成），
`app/build/app/outputs` 始终未生成，约 15 分钟后仍未进入打包；为不空耗 CPU 已终止 daemon。

最可能原因（按可能性排序）：

1. **依赖解析走不到 Maven**：AGP 9.1.0 / Kotlin 2.4.0 / AndroidX 需要 `maven.google.com`，与 doctor 报的
   `maven.google.com ... Connection failed` 一致；JVM 侧与 curl 侧的网络行为不同（见第 2 节第 2 条的 307 案例）。
2. 首次构建的 `-all` 发行包 + 8G 堆的 Gradle daemon 在无 GPU/内存有限的 VM 上极慢（`-Xmx8G` 由脚手架默认）。
3. NDK r28c 装完后 AGP 仍要校验 NDK 版本（工程没有 `ndkVersion` 约束）。

## 4. 修复步骤（可直接照做）

1. **配 Maven 镜像**（在 `app/android/settings.gradle.kts` 的 `pluginManagement.repositories` 与
   `dependencyResolutionManagement.repositories` 里，把阿里云放最前）：
   ```kotlin
   maven { url = uri("https://maven.aliyun.com/repository/google") }
   maven { url = uri("https://maven.aliyun.com/repository/public") }
   maven { url = uri("https://maven.aliyun.com/repository/gradle-plugin") }
   ```
   或全局写 `~/.gradle/init.gradle`，对所有工程生效（不改仓库）。
2. **Gradle 发行包走镜像**（第 2 节第 2 条，已验证）。
3. **把 Gradle 堆调小**：`android/gradle.properties` 里 `org.gradle.jvmargs=-Xmx3G -XX:MaxMetaspaceSize=1G`，
   叠 `org.gradle.daemon=false`，排除 VM 内存压力。
4. **固定 NDK**：`android/app/build.gradle.kts` 的 `android { ndkVersion = "28.0.xxxx" }` 或用 `flutter build apk` 前
   `flutter config --android-sdk` 校验；本轮 NDK 是自动装的 `r28c`。
5. 重跑：`cd app && flutter build apk --debug`，产物应在
   `build/app/outputs/flutter-apk/app-debug.apk`。

## 5. 安卓端功能盘点（B2）

| 能力 | 现状 | 说明 |
| --- | --- | --- |
| 设备列表 | **待办** | 共享 Dart 侧有设备模型/网关，但依赖 Rust 桥接库；Android 侧没有打包 `librdc_bridge.so` |
| 发起连接 | **待办** | 同上，且无 Android 网络/权限配置（`INTERNET` 之外还需目标网段可达） |
| 解码画面（MediaCodec） | **待办（完全缺）** | `app/lib` 内无 `MethodChannel`/`EventChannel`，也没有任何 `MediaCodec` 引用；现有 RGBA 降级通道只是“把字节传到 Dart”，Android 上会直接内存带宽爆炸 |
| 触摸注入 | **待办** | 复用桌面 `SessionPage` 的指针事件即可产生事件；但远端注入协议在 Android 上依赖 Rust 桥接库 |
| 软键盘注入 | **待办（完全缺）** | 无 `TextInput` 桥接、无 IME 事件采集 |
| 横竖屏 | **部分具备** | Flutter 侧已按视口等比适配（`ViewportController`），Android 只需在 Manifest 放开 `screenOrientation`；未在真机验证 |
| 原生库装载 | **待办（阻塞项）** | 无 `jniLibs`、无 `cargo-ndk` 构建、无 `externalNativeBuild`；即使 APK 产出，App 启动时 FRB 打开动态库会失败 |

**结论**：本轮连「能装、能起」都未达到——APK 未产出；且即便产出，因原生库未接入也会在启动即失败。
真正要跑通最小链路，必须先补：`cargo-ndk` 构建 + `jniLibs` 打包 + MediaCodec 解码通道 + 输入桥接。

## 6. 真机说明

- 无 Android 真机接入，`adb devices` 为空；VM 无 `/dev/kvm`，模拟器不可用（未硬撑）。
- 因此**没有任何安卓运行截图**，不伪造。若要验证：`adb install -r app-debug.apk` 后手动确认四步（能装/能起/能连/能看）。
