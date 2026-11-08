#ifndef RUNNER_FLUTTER_WINDOW_H_
#define RUNNER_FLUTTER_WINDOW_H_

#include <flutter/dart_project.h>
#include <flutter/flutter_view_controller.h>

#include <chrono>
#include <memory>
#include <string>

#include "win32_window.h"

// A window that does nothing but host a Flutter view.
class FlutterWindow : public Win32Window {
 public:
  // Creates a new FlutterWindow hosting a Flutter view running |project|.
  // |renderer_backend| 只用于诊断文本（skia / impeller / skia(compat)）。
  FlutterWindow(const flutter::DartProject& project,
                const std::string& renderer_backend);
  virtual ~FlutterWindow();

 protected:
  // Win32Window:
  bool OnCreate() override;
  void OnDestroy() override;
  LRESULT MessageHandler(HWND window, UINT const message, WPARAM const wparam,
                         LPARAM const lparam) noexcept override;

 private:
  // 注册首帧回调：首帧到达即显示主窗口；若保底窗口正显示则收起它。
  void InstallFirstFrameCallback();

  // 重置渲染健康观测（3 秒看门狗 + 首帧后纯色采样）。
  void StartRenderWatch(int attempt_index);

  // 定时评估：3 秒未出首帧或首帧后画面纯色 -> 显示原生保底窗口。
  void EvaluateRenderState();

  // 显示 RDR-001 保底窗口（不依赖 Flutter）。
  // |cause_code| 是机器可检索的原因码（RDR-001 的细分），|cause_text| 给人看。
  void ShowRenderFallback(const std::string& cause_code,
                          const wchar_t* cause_text);

  // 「用兼容渲染重试」：进程内销毁并按 Skia 后端重新初始化 Flutter。
  bool RetryWithCompatRenderer();

  // The project to run.
  flutter::DartProject project_;

  // The Flutter instance hosted by this window.
  std::unique_ptr<flutter::FlutterViewController> flutter_controller_;

  std::string renderer_backend_;
  std::chrono::steady_clock::time_point watch_start_;
  bool first_frame_seen_ = false;
  bool fallback_shown_ = false;
  bool blank_checked_ = false;
  // 兼容渲染重试后，主窗口先以「不抢焦点」方式显示，保底窗口继续盖在上面，
  // 直到 3 秒采样确认真的有界面，才收起保底窗口。避免重试失败闪回纯白。
  bool recovering_ = false;
  int attempt_ = 1;
};

#endif  // RUNNER_FLUTTER_WINDOW_H_
