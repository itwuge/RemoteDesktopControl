cmake_minimum_required(VERSION 3.14)
project(runner LANGUAGES CXX)

# Define the application target. To change its name, change BINARY_NAME in the
# top-level CMakeLists.txt, not the value here, or `flutter run` will no longer
# work.
#
# Any new source files that you add to the application should be added here.
add_executable(${BINARY_NAME} WIN32
  "control_texture.cpp"
  "flutter_window.cpp"
  "main.cpp"
  "render_fallback.cpp"
  "utils.cpp"
  "win32_window.cpp"
  "${FLUTTER_MANAGED_DIR}/generated_plugin_registrant.cc"
  "Runner.rc"
  "runner.exe.manifest"
)

# Apply the standard set of build settings. This can be removed for applications
# that need different build settings.
apply_standard_settings(${BINARY_NAME})

# 源码含中文注释与中文字面量，统一按 UTF-8 解析；否则 MSVC 按系统代码页(936)
# 解码会把注释尾字节当成续行符，直接导致语法错误。
target_compile_options(${BINARY_NAME} PRIVATE "/utf-8")

# Add preprocessor definitions for the build version.
target_compile_definitions(${BINARY_NAME} PRIVATE "FLUTTER_VERSION=\"${FLUTTER_VERSION}\"")
target_compile_definitions(${BINARY_NAME} PRIVATE "FLUTTER_VERSION_MAJOR=${FLUTTER_VERSION_MAJOR}")
target_compile_definitions(${BINARY_NAME} PRIVATE "FLUTTER_VERSION_MINOR=${FLUTTER_VERSION_MINOR}")
target_compile_definitions(${BINARY_NAME} PRIVATE "FLUTTER_VERSION_PATCH=${FLUTTER_VERSION_PATCH}")
target_compile_definitions(${BINARY_NAME} PRIVATE "FLUTTER_VERSION_BUILD=${FLUTTER_VERSION_BUILD}")

# Disable Windows macros that collide with C++ standard library functions.
target_compile_definitions(${BINARY_NAME} PRIVATE "NOMINMAX")

# Add dependency libraries and include directories. Add any application-specific
# dependencies here.
target_link_libraries(${BINARY_NAME} PRIVATE flutter flutter_wrapper_app)
target_link_libraries(${BINARY_NAME} PRIVATE "dwmapi.lib")
# 原生保底窗口（RDR-001）：ShellExecuteW 打开日志目录 / 诊断说明，SysLink 来自 comctl32。
target_link_libraries(${BINARY_NAME} PRIVATE "shell32.lib" "comctl32.lib")
target_include_directories(${BINARY_NAME} PRIVATE "${CMAKE_SOURCE_DIR}")

# Run the Flutter tool portions of the build. This must not be removed.
add_dependencies(${BINARY_NAME} flutter_assemble)

# =====** RDC Rust 桥接库 =====**
# 用途：把 crates/rdc-bridge 编译为 rdc_bridge.dll 并复制到可执行文件目录，
# 避免依赖 Flutter 插件（插件工程需要 Windows 开发者模式创建符号链接）。
# 未找到 cargo 时只告警：Flutter 构建仍可完成，打包脚本会单独补建动态库。
find_program(RDC_CARGO_EXECUTABLE cargo HINTS "$ENV{USERPROFILE}/.cargo/bin" "$ENV{HOME}/.cargo/bin")
if(RDC_CARGO_EXECUTABLE)
  set(RDC_REPO_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/../../..")

  # DLL 产物目录：默认仓库内 target/release；外部构建脚本用 RDC_DLL_DIR 显式指定，
  # 该值必须与 cargo 的 CARGO_TARGET_DIR 指向同一目录，否则下面的复制会找不到文件（MSB3073）。
  set(RDC_DLL_SOURCE_DIR "$ENV{RDC_DLL_DIR}")
  if(NOT RDC_DLL_SOURCE_DIR)
    set(RDC_DLL_SOURCE_DIR "${RDC_REPO_ROOT}/target/release")
  endif()
  message(STATUS "rdc_bridge.dll 来源目录：${RDC_DLL_SOURCE_DIR}")

  add_custom_target(rdc_ffi_build ALL
    COMMAND "${RDC_CARGO_EXECUTABLE}" build -p rdc-bridge --release
    WORKING_DIRECTORY "${RDC_REPO_ROOT}"
    COMMENT "构建 Rust FFI 动态库 rdc_bridge.dll"
    VERBATIM
  )
  add_dependencies(${BINARY_NAME} rdc_ffi_build)

  add_custom_command(TARGET ${BINARY_NAME} POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E copy_if_different
      "${RDC_DLL_SOURCE_DIR}/rdc_bridge.dll"
      "$<TARGET_FILE_DIR:${BINARY_NAME}>/rdc_bridge.dll"
    COMMENT "复制 rdc_bridge.dll 到输出目录"
    VERBATIM
  )
else()
  message(WARNING "未找到 cargo，跳过 Rust 桥接库构建；请先安装 Rust 工具链")
endif()
