// ============================================================
// 文件用途：Android 应用模块构建脚本
//
// 层级：app/android/app
// 边界：只声明 Android 打包参数与 Rust 动态库打包位置；
//       业务逻辑在 app/lib（Flutter）与 Kotlin 插件（解码/输入）中
// ============================================================

plugins {
    id("com.android.application")
    // Flutter 模板关闭了 AGP 内置 Kotlin（android.builtInKotlin=false），
    // 因此这里必须显式应用 Kotlin Android 插件，否则 MainActivity.kt 不参与编译。
    id("org.jetbrains.kotlin.android")
    // The Flutter Gradle Plugin must be applied after the Android and Kotlin Gradle plugins.
    id("dev.flutter.flutter-gradle-plugin")
}

android {
    namespace = "com.rdc.rdc"

    // =====** 版本固定（禁止漂移） =====**
    // compileSdk/targetSdk 36：构建机已装 platforms;android-36（实测）。
    compileSdk = 36
    // NDK 28.2.13676358 = r28c，构建机实测安装版本；cargo-ndk 也按该 NDK 编译。
    ndkVersion = "28.2.13676358"

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    defaultConfig {
        applicationId = "com.rdc.rdc"
        // Android 8.0（API 26）：docs/08 角色矩阵的移动端下限。
        minSdk = 26
        targetSdk = 36
        versionCode = flutter.versionCode
        versionName = flutter.versionName

        // 第一版只出 arm64-v8a：真机（2019 年后主流机型）均为 64 位；
        // armeabi-v7a 由 packaging/android/build_apk.sh 的 split 构建按需追加。
        ndk {
            abiFilters += listOf("arm64-v8a")
        }
    }

    // Rust 动态库由 cargo-ndk 输出到 src/main/jniLibs/<abi>/，默认即可打包；
    // 这里显式声明目录，避免被后续 sourceSets 改动遗漏。
    sourceSets.getByName("main") {
        jniLibs.srcDirs("src/main/jniLibs")
    }

    buildTypes {
        // =====** 仅 debug 追加 x86_64（Android 模拟器） =====**
        // 交付包虽走 debug 构建，但只编译 arm64 的 Rust 库（见 packaging/android/build_apk.sh），
        // 因此不会带上 x86_64 的 .so；release 分支与交付脚本都不读取本节。
        // 实测说明：Flutter 3.47.9 的 Gradle 插件会向 defaultConfig 注入
        // [armeabi-v7a, arm64-v8a, x86_64]，单个 ABI 的实际内容由 jniLibs 决定；
        // 本行把「debug 支持 x86_64」从插件的隐式行为改为显式声明。
        debug {
            ndk {
                abiFilters += listOf("x86_64")
            }
        }

        release {
            // 暂用 debug 签名（CI 提供正式 keystore 时再切换，见 packaging/android）。
            signingConfig = signingConfigs.getByName("debug")
        }
    }

    // 打包含 .so 时不再压缩，避免安装期解压占用与启动延迟。
    packaging {
        jniLibs.useLegacyPackaging = false
    }
}

kotlin {
    compilerOptions {
        jvmTarget = org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17
    }
}

flutter {
    source = "../.."
}

dependencies {
    // JVM 单测只覆盖纯 Kotlin 解析逻辑（Annex-B/SPS/PPS），不启动 Android 运行时。
    testImplementation("junit:junit:4.13.2")
}
