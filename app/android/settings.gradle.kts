// ============================================================
// 文件用途：Gradle 设置脚本（插件仓库 + 版本固定）
//
// 层级：app/android
// 被谁调用：Gradle 启动时（flutter build apk / gradlew）
// 边界：只声明仓库与插件版本；不写业务构建逻辑
//
// 网络约束（实测于 2026-11-07，Ubuntu 22.04 构建机）：
//   - services.gradle.org / maven.google.com 不可达（连接超时）；
//   - maven.aliyun.com、dl.google.com、repo.maven.apache.org、
//     plugins.gradle.org、mirrors.cloud.tencent.com 可达。
// 因此仓库顺序为「阿里云镜像优先，官方仓库兜底」，避免依赖解析卡死。
// ============================================================

pluginManagement {
    // Flutter SDK 路径来自 local.properties（由 flutter 工具自动写入）。
    val flutterSdkPath =
        run {
            val properties = java.util.Properties()
            file("local.properties").inputStream().use { properties.load(it) }
            val flutterSdkPath = properties.getProperty("flutter.sdk")
            require(flutterSdkPath != null) { "flutter.sdk not set in local.properties" }
            flutterSdkPath
        }

    includeBuild("$flutterSdkPath/packages/flutter_tools/gradle")

    repositories {
        // =====** 国内镜像（优先） =====**
        // 阿里云 google 代理：AGP 与 androidx 等 Google Maven 构件。
        maven { url = uri("https://maven.aliyun.com/repository/google") }
        // 阿里云 public：Maven Central + JCenter 聚合。
        maven { url = uri("https://maven.aliyun.com/repository/public") }
        // Gradle 插件门户镜像：Kotlin/AGP 的 plugin marker 也在这里。
        maven { url = uri("https://maven.aliyun.com/repository/gradle-plugin") }

        // =====** 官方仓库（兜底，仅在镜像缺失时命中） =====**
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

// =====** 版本固定（避免工具链漂移） =====**
// AGP 9.1.0 / Kotlin 2.4.0 与 Flutter 3.47.9 模板一致；
// 修改前必须同时确认 Flutter 的 DependencyVersionChecker 与 NDK 版本。
plugins {
    id("dev.flutter.flutter-plugin-loader") version "1.0.0"
    id("com.android.application") version "9.1.0" apply false
    id("org.jetbrains.kotlin.android") version "2.4.0" apply false
}

include(":app")
