// ============================================================
// 文件用途：根构建脚本（子项目仓库 + 产物目录重定向）
//
// 层级：app/android
// 边界：只做全局配置；模块级配置在 app/build.gradle.kts
// ============================================================

allprojects {
    repositories {
        // =====** 国内镜像（优先，理由见 settings.gradle.kts 头注释） =====**
        maven { url = uri("https://maven.aliyun.com/repository/google") }
        maven { url = uri("https://maven.aliyun.com/repository/public") }
        maven { url = uri("https://maven.aliyun.com/repository/central") }

        // =====** 官方仓库（兜底） =====**
        google()
        mavenCentral()
    }
}

val newBuildDir: Directory =
    rootProject.layout.buildDirectory
        .dir("../../build")
        .get()
rootProject.layout.buildDirectory.value(newBuildDir)

subprojects {
    val newSubprojectBuildDir: Directory = newBuildDir.dir(project.name)
    project.layout.buildDirectory.value(newSubprojectBuildDir)
}
subprojects {
    project.evaluationDependsOn(":app")
}

tasks.register<Delete>("clean") {
    delete(rootProject.layout.buildDirectory)
}
