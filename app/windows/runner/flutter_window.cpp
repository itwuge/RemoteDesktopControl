#include "flutter_window.h"

#include <windows.h>

#include <atomic>
#include <chrono>
#include <memory>
#include <optional>
#include <string>
#include <thread>

#include "control_texture.h"
#include "flutter/generated_plugin_registrant.h"
#include "render_fallback.h"
#include "utils.h"

namespace {

// 定时器：每秒评估一次渲染健康状态（3 秒看门狗 + 首帧后纯色采样）。
constexpr UINT_PTR kRenderWatchTimer = 1;

// 判定「没有呈现界面」的采样容差：每个通道的最大差值不超过该值即视为纯色。
constexpr int kBlankTolerance = 6;

// 采样 13 x 9 个点，覆盖整块客户区，避免只取角落。
constexpr int kSampleColumns = 13;
constexpr int kSampleRows = 9;

// 抓屏并判断画面是否为纯色（全白 / 全黑 / 单色）。
//
// 先试 PrintWindow(PW_RENDERFULLCONTENT)，它能拿到 DWM 合成后的界面内容，
// 不受其他窗口遮挡；失败时退回屏幕 BitBlt。两者都失败时不作判定，
// 避免把「抓不到」误报成「白屏」。
// |print_only| 用于兼容渲染重试后的判定：此时保底窗口盖在主窗口上方，
// 屏幕 BitBlt 会抓到保底窗口本身，因此只允许使用 PrintWindow。
bool CaptureLooksBlank(HWND window, bool print_only, bool* sampled) {
  *sampled = false;
  RECT client{};
  if (!::GetClientRect(window, &client)) {
    return false;
  }
  const int width = client.right - client.left;
  const int height = client.bottom - client.top;
  if (width < 64 || height < 64) {
    return false;
  }

  HDC screen = ::GetDC(nullptr);
  if (screen == nullptr) {
    return false;
  }
  HDC memory = ::CreateCompatibleDC(screen);
  if (memory == nullptr) {
    ::ReleaseDC(nullptr, screen);
    return false;
  }
  HBITMAP bitmap = ::CreateCompatibleBitmap(screen, width, height);
  if (bitmap == nullptr) {
    ::DeleteDC(memory);
    ::ReleaseDC(nullptr, screen);
    return false;
  }
  HGDIOBJ previous = ::SelectObject(memory, bitmap);

  // PW_RENDERFULLCONTENT (0x00000002)：Win10+ 才能抓到 DWM 合成内容。
  bool captured = ::PrintWindow(window, memory, 2) != FALSE;
  if (!captured && !print_only) {
    POINT origin{0, 0};
    ::ClientToScreen(window, &origin);
    captured = ::BitBlt(memory, 0, 0, width, height, screen, origin.x,
                        origin.y, SRCCOPY | CAPTUREBLT) != FALSE;
  }
  ::ReleaseDC(nullptr, screen);

  int min_r = 255;
  int max_r = 0;
  int min_g = 255;
  int max_g = 0;
  int min_b = 255;
  int max_b = 0;
  int samples = 0;
  for (int row = 0; row < kSampleRows; ++row) {
    for (int column = 0; column < kSampleColumns; ++column) {
      const int x = width * (column + 1) / (kSampleColumns + 1);
      const int y = height * (row + 1) / (kSampleRows + 1);
      const COLORREF color = ::GetPixel(memory, x, y);
      if (color == CLR_INVALID) {
        continue;
      }
      const int red = static_cast<int>(GetRValue(color));
      const int green = static_cast<int>(GetGValue(color));
      const int blue = static_cast<int>(GetBValue(color));
      min_r = red < min_r ? red : min_r;
      max_r = red > max_r ? red : max_r;
      min_g = green < min_g ? green : min_g;
      max_g = green > max_g ? green : max_g;
      min_b = blue < min_b ? blue : min_b;
      max_b = blue > max_b ? blue : max_b;
      ++samples;
    }
  }

  ::SelectObject(memory, previous);
  ::DeleteObject(bitmap);
  ::DeleteDC(memory);

  const bool complete = captured && samples >= (kSampleRows * kSampleColumns) / 3;
  *sampled = complete;
  if (!complete) {
    return false;
  }
  return (max_r - min_r) <= kBlankTolerance &&
         (max_g - min_g) <= kBlankTolerance &&
         (max_b - min_b) <= kBlankTolerance;
}

long long MillisecondsSince(std::chrono::steady_clock::time_point start) {
  return static_cast<long long>(
      std::chrono::duration_cast<std::chrono::milliseconds>(
          std::chrono::steady_clock::now() - start)
          .count());
}

}  // namespace

FlutterWindow::FlutterWindow(const flutter::DartProject& project,
                             const std::string& renderer_backend)
    : project_(project), renderer_backend_(renderer_backend) {}

FlutterWindow::~FlutterWindow() {}

bool FlutterWindow::OnCreate() {
  if (!Win32Window::OnCreate()) {
    return false;
  }

  RECT frame = GetClientArea();
  AppendStartupLog("client_area=" +
                   std::to_string(frame.right - frame.left) + "x" +
                   std::to_string(frame.bottom - frame.top));
  AppendStartupLog("render.init_start backend=" + renderer_backend_ +
                   " attempt=1");

  // The size here must match the window dimensions to avoid unnecessary surface
  // creation / destruction in the startup path.
  flutter_controller_ = std::make_unique<flutter::FlutterViewController>(
      frame.right - frame.left, frame.bottom - frame.top, project_);
  // Ensure that basic setup of the controller was successful.
  if (!flutter_controller_->engine() || !flutter_controller_->view()) {
    AppendStartupLog("render.init_failed code=RDR-001 cause=init_failed");
    ShowRenderFallback(L"渲染后端初始化失败：Flutter 渲染控制器没有创建成功。");
    return true;  // 保底窗口接管显示，进程继续存活。
  }
  RegisterPlugins(flutter_controller_->engine());
  rdc_control_texture::Attach(flutter_controller_->engine());
  SetChildContent(flutter_controller_->view()->GetNativeWindow());

  StartRenderWatch(1);
  InstallFirstFrameCallback();

  // Flutter can complete the first frame before the "show window" callback is
  // registered. The following call ensures a frame is pending to ensure the
  // window is shown. It is a no-op if the first frame hasn't completed yet.
  flutter_controller_->ForceRedraw();
  ::SetTimer(GetHandle(), kRenderWatchTimer, 1000, nullptr);

  return true;
}

void FlutterWindow::OnDestroy() {
  ::KillTimer(GetHandle(), kRenderWatchTimer);
  rdc_fallback::Shutdown();
  rdc_control_texture::Detach();
  if (flutter_controller_) {
    flutter_controller_ = nullptr;
  }

  Win32Window::OnDestroy();
}

void FlutterWindow::StartRenderWatch(int attempt_index) {
  attempt_ = attempt_index;
  first_frame_seen_ = false;
  blank_checked_ = false;
  watch_start_ = std::chrono::steady_clock::now();
}

void FlutterWindow::InstallFirstFrameCallback() {
  auto first_frame = std::make_shared<std::atomic<bool>>(false);
  flutter_controller_->engine()->SetNextFrameCallback([this, first_frame]() {
    if (first_frame->exchange(true)) {
      return;
    }
    first_frame_seen_ = true;
    AppendStartupLog("render.ready backend=" + renderer_backend_ +
                     " attempt=" + std::to_string(attempt_) + " elapsed_ms=" +
                     std::to_string(MillisecondsSince(watch_start_)));
    if (fallback_shown_) {
      // 兼容渲染重试或慢机器：首帧到了，但保底窗口先不要收，
      // 等 3 秒采样确认真的有界面再收，避免重试失败时闪回纯白。
      recovering_ = true;
      blank_checked_ = false;
      watch_start_ = std::chrono::steady_clock::now();
      ::ShowWindow(GetHandle(), SW_SHOWNOACTIVATE);
      if (HWND fallback = ::FindWindowW(L"RDC_RENDER_FALLBACK_WINDOW", nullptr)) {
        ::SetWindowPos(fallback, HWND_TOP, 0, 0, 0, 0,
                       SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
      }
      return;
    }
    this->Show();
  });
}

void FlutterWindow::EvaluateRenderState() {
  if (GetHandle() == nullptr) {
    return;
  }
  if (!first_frame_seen_) {
    if (fallback_shown_) {
      // 重试后 10 秒仍无首帧：如实更新状态，窗口保持可见。
      if (attempt_ > 1 && MillisecondsSince(watch_start_) >= 10000) {
        rdc_fallback::SetStatusText(
            L"兼容渲染在 10 秒内仍未产出首帧。保底窗口保留，请按第 2、3 步继续排查。");
      }
      return;
    }
    if (MillisecondsSince(watch_start_) >= 3000) {
      ShowRenderFallback(L"3 秒内主窗口没有完成首帧呈现。");
    }
    return;
  }
  if (blank_checked_) {
    return;
  }
  if (MillisecondsSince(watch_start_) < 3000) {
    return;
  }
  blank_checked_ = true;
  bool sampled = false;
  const bool blank = CaptureLooksBlank(GetHandle(), recovering_, &sampled);
  if (sampled && !blank) {
    AppendStartupLog("render_check=content");
    if (fallback_shown_) {
      fallback_shown_ = false;
      recovering_ = false;
      rdc_fallback::Hide();
    }
    return;
  }
  if (sampled && blank) {
    if (fallback_shown_) {
      AppendStartupLog("render_check=blank after retry");
      rdc_fallback::SetStatusText(
          L"兼容渲染切换成功，但画面仍然是纯白/纯色，界面无法显示。"
          L"保底窗口保留，请按第 2、3 步继续排查。");
      recovering_ = false;
    } else {
      AppendStartupLog("render_check=blank code=RDR-001");
      ShowRenderFallback(L"主窗口已产出首帧，但画面没有呈现界面（全白或纯色）。");
    }
    return;
  }
  AppendStartupLog("render_check=unavailable（抓屏失败，暂不判定）");
  if (fallback_shown_ && attempt_ > 1) {
    rdc_fallback::SetStatusText(
        L"无法验证兼容渲染是否生效：抓屏失败。保底窗口保留，请按第 2、3 步排查。");
    recovering_ = false;
  }
}

void FlutterWindow::ShowRenderFallback(const wchar_t* cause) {
  if (fallback_shown_) {
    return;
  }
  fallback_shown_ = true;
  AppendStartupLog("render.init_failed code=RDR-001 cause=render_not_visible "
                   "elapsed_ms=" +
                   std::to_string(MillisecondsSince(watch_start_)));
  rdc_fallback::SetContext(renderer_backend_, cause, attempt_);
  rdc_fallback::SetRetryHandler([this]() { return RetryWithCompatRenderer(); });
  rdc_fallback::Show(GetHandle());
}

bool FlutterWindow::RetryWithCompatRenderer() {
  AppendStartupLog("render.retry from_backend=" + renderer_backend_ +
                   " to_backend=skia attempt=" + std::to_string(attempt_ + 1));
  // Dart 侧据此在设置页显示「当前渲染：兼容模式」，并让下次启动沿用。
  ::SetEnvironmentVariableW(L"RDC_COMPAT", L"1");
  WriteRenderModeSetting("compat");

  if (flutter_controller_) {
    rdc_control_texture::Detach();
    flutter_controller_ = nullptr;
  }
  project_.set_impeller_switch(flutter::ImpellerSwitch::Disabled);

  RECT frame = GetClientArea();
  flutter_controller_ = std::make_unique<flutter::FlutterViewController>(
      frame.right - frame.left, frame.bottom - frame.top, project_);
  if (!flutter_controller_->engine() || !flutter_controller_->view()) {
    AppendStartupLog("render.retry=FAILED code=RDR-001 cause=init_failed");
    flutter_controller_ = nullptr;
    return false;
  }
  RegisterPlugins(flutter_controller_->engine());
  rdc_control_texture::Attach(flutter_controller_->engine());
  SetChildContent(flutter_controller_->view()->GetNativeWindow());
  renderer_backend_ = "skia(compat)";

  recovering_ = true;
  StartRenderWatch(attempt_ + 1);
  InstallFirstFrameCallback();
  flutter_controller_->ForceRedraw();
  return true;
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
    case WM_TIMER:
      if (wparam == kRenderWatchTimer) {
        EvaluateRenderState();
        return 0;
      }
      break;
    case WM_FONTCHANGE:
      if (flutter_controller_ && flutter_controller_->engine()) {
        flutter_controller_->engine()->ReloadSystemFonts();
      }
      break;
  }

  return Win32Window::MessageHandler(hwnd, message, wparam, lparam);
}
