#include "flutter_window.h"

#include <atomic>
#include <chrono>
#include <memory>
#include <optional>
#include <string>
#include <thread>

#include "control_texture.h"
#include "flutter/generated_plugin_registrant.h"
#include "utils.h"

FlutterWindow::FlutterWindow(const flutter::DartProject& project)
    : project_(project) {}

FlutterWindow::~FlutterWindow() {}

bool FlutterWindow::OnCreate() {
  if (!Win32Window::OnCreate()) {
    return false;
  }

  RECT frame = GetClientArea();
  AppendStartupLog("client_area=" +
                   std::to_string(frame.right - frame.left) + "x" +
                   std::to_string(frame.bottom - frame.top));

  // The size here must match the window dimensions to avoid unnecessary surface
  // creation / destruction in the startup path.
  flutter_controller_ = std::make_unique<flutter::FlutterViewController>(
      frame.right - frame.left, frame.bottom - frame.top, project_);
  // Ensure that basic setup of the controller was successful.
  if (!flutter_controller_->engine() || !flutter_controller_->view()) {
    AppendStartupLog("flutter_controller=FAILED");
    return false;
  }
  RegisterPlugins(flutter_controller_->engine());
  rdc_control_texture::Attach(flutter_controller_->engine());
  SetChildContent(flutter_controller_->view()->GetNativeWindow());

  // A blank window is otherwise indistinguishable from a broken layout, so
  // record whether the engine ever produced a frame.
  auto first_frame = std::make_shared<std::atomic<bool>>(false);
  flutter_controller_->engine()->SetNextFrameCallback(
      [this, first_frame]() {
        if (!first_frame->exchange(true)) {
          AppendStartupLog("first_frame=rendered");
        }
        this->Show();
      });
  std::thread([first_frame]() {
    std::this_thread::sleep_for(std::chrono::seconds(10));
    if (!first_frame->load()) {
      AppendStartupLog(
          "first_frame=TIMEOUT no frame after 10s; window stays blank");
    }
  }).detach();

  // Flutter can complete the first frame before the "show window" callback is
  // registered. The following call ensures a frame is pending to ensure the
  // window is shown. It is a no-op if the first frame hasn't completed yet.
  flutter_controller_->ForceRedraw();

  return true;
}

void FlutterWindow::OnDestroy() {
  rdc_control_texture::Detach();
  if (flutter_controller_) {
    flutter_controller_ = nullptr;
  }

  Win32Window::OnDestroy();
}

LRESULT
FlutterWindow::MessageHandler(HWND hwnd, UINT const message,
                              WPARAM const wparam,
                              LPARAM const lparam) noexcept {
  // Give Flutter, including plugins, an opportunity to handle window messages.
  if (flutter_controller_) {
    std::optional<LRESULT> result =
        flutter_controller_->HandleTopLevelWindowProc(hwnd, message, wparam,
                                                      lparam);
    if (result) {
      return *result;
    }
  }

  switch (message) {
    case WM_FONTCHANGE:
      flutter_controller_->engine()->ReloadSystemFonts();
      break;
    case WM_GETMINMAXINFO: {
      // 最小窗口 960x640 逻辑像素：低于此宽高时工具栏与表单会互相挤压
      // （清单 P1-11）。按窗口 DPI 换算，125%/150% 缩放下同样成立。
      auto* info = reinterpret_cast<MINMAXINFO*>(lparam);
      const UINT dpi = ::GetDpiForWindow(hwnd);
      const double scale = dpi == 0 ? 1.0 : static_cast<double>(dpi) / 96.0;
      info->ptMinTrackSize.x = static_cast<LONG>(960 * scale);
      info->ptMinTrackSize.y = static_cast<LONG>(640 * scale);
      return 0;
    }
  }

  return Win32Window::MessageHandler(hwnd, message, wparam, lparam);
}
