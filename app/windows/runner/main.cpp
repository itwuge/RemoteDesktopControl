#include <flutter/dart_project.h>
#include <flutter/flutter_view_controller.h>
#include <windows.h>

#include <cstdlib>
#include <string>

#include "flutter_window.h"
#include "utils.h"

namespace {

// Picks the Flutter Windows rendering backend.
//
// Impeller on Windows draws through OpenGL ES (ANGLE). On some GPU drivers and
// VM guests it silently produces no swapchain content, so the window stays
// pure white while the engine and Dart code keep running. Skia (ANGLE/D3D11)
// is the long-standing, broadly compatible path, so it is the default here.
// Override with RDC_RENDERER=impeller|auto|skia.
flutter::ImpellerSwitch ResolveImpellerSwitch(std::string* name) {
  constexpr DWORD kCapacity = 32;
  wchar_t raw[kCapacity];
  DWORD raw_length = ::GetEnvironmentVariableW(L"RDC_RENDERER", raw, kCapacity);
  const std::wstring requested =
      (raw_length != 0 && raw_length < kCapacity) ? std::wstring(raw, raw_length) : L"skia";
  if (requested == L"impeller") {
    *name = "impeller";
    return flutter::ImpellerSwitch::Enabled;
  }
  if (requested == L"auto") {
    *name = "auto";
    return flutter::ImpellerSwitch::Default;
  }
  *name = "skia";
  return flutter::ImpellerSwitch::Disabled;
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
  AppendStartupLog("startup renderer=" + renderer_name + " gpu=\"" +
                   DescribePrimaryAdapter() + "\" remote_session=" +
                   (IsRemoteSession() ? "true" : "false"));

  FlutterWindow window(project);
  Win32Window::Point origin(10, 10);
  Win32Window::Size size(1280, 720);
  if (!window.Create(L"RDC 远程桌面", origin, size)) {
    AppendStartupLog("window_create=FAILED");
    return EXIT_FAILURE;
  }
  AppendStartupLog("window_create=ok size=1280x720");
  window.SetQuitOnClose(true);

  ::MSG msg;
  while (::GetMessage(&msg, nullptr, 0, 0)) {
    ::TranslateMessage(&msg);
    ::DispatchMessage(&msg);
  }

  ::CoUninitialize();
  return EXIT_SUCCESS;
}
