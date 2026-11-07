cmake_minimum_required(VERSION 3.13)
project(runner LANGUAGES CXX)

# Define the application target. To change its name, change BINARY_NAME in the
# top-level CMakeLists.txt, not the value here, or `flutter run` will no longer
# work.
#
# Any new source files that you add to the application should be added here.
add_executable(${BINARY_NAME}
  "main.cc"
  "my_application.cc"
  "${FLUTTER_MANAGED_DIR}/generated_plugin_registrant.cc"
)

# Apply the standard set of build settings. This can be removed for applications
# that need different build settings.
apply_standard_settings(${BINARY_NAME})

# Add preprocessor definitions for the application ID.
add_definitions(-DAPPLICATION_ID="${APPLICATION_ID}")

# Add dependency libraries. Add any application-specific dependencies here.
target_link_libraries(${BINARY_NAME} PRIVATE flutter)
target_link_libraries(${BINARY_NAME} PRIVATE PkgConfig::GTK)

target_include_directories(${BINARY_NAME} PRIVATE "${CMAKE_SOURCE_DIR}")

# =====** RDC Rust 桥接库 =====**
# 用途：编译 crates/rdc-bridge 为 librdc_bridge.so 并放到可执行文件同目录。
# 未找到 cargo 时只告警，构建继续；打包脚本负责最终产物中的库布局。
find_program(RDC_CARGO_EXECUTABLE cargo HINTS "$ENV{HOME}/.cargo/bin" "/usr/local/cargo/bin")
if(RDC_CARGO_EXECUTABLE)
  set(RDC_REPO_ROOT "${CMAKE_CURRENT_SOURCE_DIR}/../../..")

  add_custom_target(rdc_ffi_build ALL
    COMMAND "${RDC_CARGO_EXECUTABLE}" build -p rdc-bridge --release
    WORKING_DIRECTORY "${RDC_REPO_ROOT}"
    COMMENT "构建 Rust FFI 动态库 librdc_bridge.so"
    VERBATIM
  )
  add_dependencies(${BINARY_NAME} rdc_ffi_build)

  add_custom_command(TARGET ${BINARY_NAME} POST_BUILD
    COMMAND ${CMAKE_COMMAND} -E copy_if_different
      "${RDC_REPO_ROOT}/target/release/librdc_bridge.so"
      "$<TARGET_FILE_DIR:${BINARY_NAME}>/librdc_bridge.so"
    COMMENT "复制 librdc_bridge.so 到 bundle 目录"
    VERBATIM
  )

  # 关键：必须把库装进 bundle 根目录（与可执行文件同目录），否则 Dart 侧候选项
  # 只查可执行文件同目录，装载会失败。
  # 注意：顶层 install 会先清空 bundle（cmake_install.cmake 第 47 行），本规则在其后执行。
  install(FILES "${RDC_REPO_ROOT}/target/release/librdc_bridge.so"
    DESTINATION "${CMAKE_BINARY_DIR}/bundle")
else()
  message(WARNING "未找到 cargo，跳过 Rust 桥接库构建")
endif()
