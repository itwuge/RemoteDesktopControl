#ifndef RUNNER_RENDER_FALLBACK_H_
#define RUNNER_RENDER_FALLBACK_H_

#include <windows.h>

#include <functional>
#include <string>

// 原生保底窗口（RDR-001）。
//
// 为什么存在：本机实测 Flutter 的 EGL/GLES 呈现通道在 RX 580 + 该驱动组合下
// 只出纯白画面，任何走 Flutter 的界面都会一起白。因此渲染失败时的提示必须由
// Win32 原生控件绘制（GDI + 标准 BUTTON/STATIC），才能保证白屏时仍然可见、可
// 操作、可被截图取证。
namespace rdc_fallback {

// 面向用户的稳定错误码，与 docs/19 的错误码表一一对应。
constexpr char kRenderErrorCode[] = "RDR-001";

// 设置诊断上下文：渲染后端名、机器可检索的原因码（no_display_output /
// render_not_visible / init_failed）、给用户看的中文原因、第几次尝试。
void SetContext(const std::string& backend, const std::string& cause_code,
                const std::wstring& cause_text, int attempt_index);

// 注册「用兼容渲染重试」的真实动作。返回 true 表示已发起重新初始化，
// 保底窗口保持可见直到首帧到达或再次失败；返回 false 表示本次重试失败。
void SetRetryHandler(std::function<bool()> handler);

// 显示保底窗口。必须在已有消息循环的线程上调用。
// parent 不为空时作为 owner：保底窗口浮在主窗口之上，但主窗口保持可见
// （任务栏可调起，避免出现「程序像没启动」的观感）。
bool Show(HWND parent);

// 保底窗口当前是否可见。
bool IsShown();

// 首帧恢复后隐藏保底窗口，并把主窗口重新显示出来。
void Hide();

// 更新窗口底部的状态行（例如「正在用兼容渲染重新初始化…」）。
void SetStatusText(const std::wstring& text);

// 关闭窗口并释放资源；可重复调用。
void Shutdown();

// 组装「复制诊断」的纯文本内容（UTF-8 / UTF-16 两份）。
// 只包含版本、平台、显卡与驱动、错误码、日志路径与最近 5 条事件，
// 不包含密码、配对码、令牌或任何屏幕内容。
std::string BuildDiagnosticsText();
std::wstring BuildDiagnosticsTextWide();

}  // namespace rdc_fallback

#endif  // RUNNER_RENDER_FALLBACK_H_
