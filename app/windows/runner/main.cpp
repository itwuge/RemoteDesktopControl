// 用途：Flutter Windows runner 进程入口（命令行解析、渲染后端选择与主窗口启动）。
#include <flutter/dart_project.h>
#include <flutter/flutter_view_controller.h>
#include <windows.h>

#include <cstdlib>
#include <string>

#include "flutter_window.h"
#include "render_fallback.h"
#include "utils.h"

namespace {

// Picks the Flutter Windows rendering backend.
//
// Impeller on Windows draws through OpenGL ES (ANGLE). On some GPU drivers and
// VM guests it silently produces no swapchain content, so the window stays
// pure white while the engine and Dart code keep running. Skia (ANGLE/D3D11)
// is the long-standing, broadly compatible path, so it is the default here.
// Override with RDC_RENDERER=impeller|auto|skia. 用户点过「用兼容渲染重试」
// 之后，%APPDATA%\RDC\render_mode.txt 里会留下 compat，下次启动继续用 Skia。
flutter::ImpellerSwitch ResolveImpellerSwitch(std::string* name) {
  constexpr DWORD kCapacity = 32;
  wchar_t raw[kCapacity];
  DWORD raw_length = ::GetEnvironmentVariableW(L"RDC_RENDERER", raw, kCapacity);
  const std::wstring requested =
      (raw_length != 0 && raw_length < kCapacity) ? std::wstring(raw, raw_length) : L"";

  if (requested == L"impeller") {
    *name = "impeller";
    return flutter::ImpellerSwitch::Enabled;
  }
  if (requested == L"auto") {
    *name = "auto";
    return flutter::ImpellerSwitch::Default;
  }

  if (ReadRenderModeSetting() == "compat") {
    *name = "skia(compat)";
    // 让 Dart 侧在设置页显示「当前渲染：兼容模式」。
    ::SetEnvironmentVariableW(L"RDC_COMPAT", L"1");
    return flutter::ImpellerSwitch::Disabled;
  }

  *name = "skia";
  return flutter::ImpellerSwitch::Disabled;
}

// 消息循环：主窗口与原生保底窗口共用同一个 pump，绝不另起线程/循环。
int RunMessageLoop() {
  ::MSG msg;
  while (::GetMessage(&msg, nullptr, 0, 0)) {
    ::TranslateMessage(&msg);
    ::DispatchMessage(&msg);
  }
  return 0;
}

}  // namespace

int APIENTRY wWinMain(_In_ HINSTANCE instance, _In_opt_ HINSTANCE prev,
                      _In_ wchar_t *command_line, _In_ int show_command) {
  // Attach to console when present (e.g., 'flutter run') or create a
  // new console when running with a debugger.
  if (!::AttachConsole(ATTACH_PARENT_PROCESS) && ::IsDebuggerPresent()) {
    CreateAndAttachConsole();
  }

  // Initialize COM, so that it is available for use in the library and/or
  // plugins.
  ::CoInitializeEx(nullptr, COINIT_APARTMENTTHREADED);

  flutter::DartProject project(L"data");

  std::vector<std::string> command_line_arguments =
      GetCommandLineArguments();

  project.set_dart_entrypoint_arguments(std::move(command_line_arguments));

  std::string renderer_name;
  project.set_impeller_switch(ResolveImpellerSwitch(&renderer_name));

  FlutterWindow window(project, renderer_name);
  Win32Window::Point origin(10, 10);
  Win32Window::Size size(1280, 720);
  if (!window.Create(L"RDC 远程桌面", origin, size)) {
    AppendStartupLog("window_create=FAILED code=RDR-001");
    // 连主窗口都没建起来：保底窗口仍然必须可用，这是最后的显示通道。
    rdc_fallback::SetContext("unknown", "init_failed",
                             L"主窗口创建失败（Win32 窗口没有建起来）。", 1);
    rdc_fallback::Show(nullptr);
    const int result = RunMessageLoop();
    ::CoUninitialize();
    return result == 0 ? EXIT_SUCCESS : EXIT_FAILURE;
  }
  AppendStartupLog("window_create=ok size=1280x720");
  window.SetQuitOnClose(true);

  const int result = RunMessageLoop();
  ::CoUninitialize();
  return result == 0 ? EXIT_SUCCESS : EXIT_FAILURE;
}
