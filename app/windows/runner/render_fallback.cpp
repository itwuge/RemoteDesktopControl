#include "render_fallback.h"

#include <commctrl.h>
#include <shellapi.h>
#include <windows.h>

#include <cstring>
#include <ctime>
#include <deque>
#include <fstream>
#include <sstream>
#include <string>
#include <utility>

#include "utils.h"

namespace rdc_fallback {
namespace {

constexpr wchar_t kWindowClass[] = L"RDC_RENDER_FALLBACK_WINDOW";

// 控件 ID。按钮 ID 固定，便于自动化脚本按键/截图核对。
constexpr int kIdHeading = 2000;
constexpr int kIdLine1 = 2001;
constexpr int kIdLine2 = 2002;
constexpr int kIdCause = 2003;
constexpr int kIdSteps = 2004;
constexpr int kIdDisplayHint = 2005;
constexpr int kIdStatus = 2006;
constexpr int kIdFooter = 2007;
constexpr int kIdGuide = 2008;

constexpr int kIdRetry = 1001;
constexpr int kIdCopy = 1002;
constexpr int kIdOpenLogs = 1003;

constexpr UINT kMessageExecuteRetry = WM_APP + 1;

// 客户端尺寸按 96 DPI 设计；窗口不可缩放，布局全部用固定坐标。
constexpr int kClientWidth = 860;
constexpr int kClientHeight = 490;

HWND g_window = nullptr;
HWND g_hidden_parent = nullptr;
std::wstring g_cause = L"3 秒内主窗口没有产出首帧";
std::string g_backend = "skia";
int g_attempt = 1;
bool g_retrying = false;
std::function<bool()> g_retry_handler;

HFONT g_font_heading = nullptr;
HFONT g_font_body = nullptr;
HFONT g_font_small = nullptr;

const wchar_t* kHeadingText = L"程序界面无法显示（RDR-001）";
const wchar_t* kLine1Text =
  L"界面渲染初始化失败，主窗口没能显示出来。";
const wchar_t* kLine2Text =
  L"这不是你的操作问题，也不是网络问题。可以按下面三步处理。";
const wchar_t* kStepsText =
  L"1. 先点「用兼容渲染重试」：切换渲染方式后重新初始化，不需要重装、不需要重启电脑。\r\n"
  L"2. 更新或回滚显卡驱动后重启：设备管理器 → 显示适配器 → 右键显卡 → 更新驱动程序；"
  L"若刚更新过驱动才出现此问题，请回滚到上一版本驱动。\r\n"
  L"3. 点「复制诊断」把内容发给我们，或点「打开日志目录」自己查看日志。";
const wchar_t* kDisplayHintText =
  L"被控端显示器请保持开启：显示器关闭或进入睡眠会导致黑屏，本版本不提供虚拟屏。";
const wchar_t* kFooterText =
  L"诊断信息只包含版本、显卡和错误记录，不包含密码、配对码或你的屏幕内容。";
const wchar_t* kGuideLinkText =
  L"<a href=\"guide\">查看诊断说明（RDR-001 的触发条件与三步排查）</a>";

std::wstring WideFromUtf8(const std::string& text) {
  if (text.empty()) {
    return std::wstring();
  }
  const int length = ::MultiByteToWideChar(
      CP_UTF8, 0, text.c_str(), static_cast<int>(text.size()), nullptr, 0);
  if (length <= 0) {
    return std::wstring();
  }
  std::wstring wide(static_cast<size_t>(length), L'\0');
  ::MultiByteToWideChar(CP_UTF8, 0, text.c_str(),
                        static_cast<int>(text.size()), wide.data(), length);
  return wide;
}

std::wstring ExeDirectory() {
  wchar_t buffer[MAX_PATH] = {0};
  const DWORD length = ::GetModuleFileNameW(nullptr, buffer, MAX_PATH);
  if (length == 0 || length >= MAX_PATH) {
    return std::wstring();
  }
  std::wstring path(buffer);
  const size_t slash = path.find_last_of(L'\\');
  if (slash == std::wstring::npos) {
    return std::wstring();
  }
  return path.substr(0, slash);
}

std::wstring ReadAppVersion() {
  const std::wstring dir = ExeDirectory();
  if (dir.empty()) {
    return L"未知";
  }
  std::ifstream in((dir + L"\\BUILD_INFO.txt").c_str());
  if (!in.is_open()) {
    return L"未知";
  }
  std::string line;
  while (std::getline(in, line)) {
    if (line.rfind("version=", 0) == 0) {
      return WideFromUtf8(line.substr(8));
    }
  }
  return L"未知";
}

std::wstring Timestamp() {
  std::time_t now = std::time(nullptr);
  std::tm local{};
  (void)localtime_s(&local, &now);
  wchar_t stamp[64] = {0};
  std::wcsftime(stamp, 64, L"%Y-%m-%d %H:%M:%S", &local);
  return std::wstring(stamp);
}

std::deque<std::wstring> LastLogLines(size_t count) {
  std::deque<std::wstring> lines;
  const std::wstring path = StartupLogFilePathWide();
  if (path.empty()) {
    return lines;
  }
  std::ifstream in(path.c_str());
  if (!in.is_open()) {
    return lines;
  }
  std::string line;
  while (std::getline(in, line)) {
    lines.push_back(WideFromUtf8(line));
    if (lines.size() > count) {
      lines.pop_front();
    }
  }
  return lines;
}

void RememberHiddenParent(HWND parent) {
  g_hidden_parent = parent;
  if (parent != nullptr && ::IsWindowVisible(parent)) {
    ::ShowWindow(parent, SW_HIDE);
  }
}

void ApplyFonts(HWND window) {
  struct FontBinding {
    int id;
    HFONT font;
  };
  const FontBinding bindings[] = {
      {kIdHeading, g_font_heading}, {kIdLine1, g_font_body},
      {kIdLine2, g_font_body},      {kIdCause, g_font_small},
      {kIdSteps, g_font_body},      {kIdDisplayHint, g_font_body},
      {kIdStatus, g_font_small},    {kIdFooter, g_font_small},
      {kIdGuide, g_font_small},     {kIdRetry, g_font_body},
      {kIdCopy, g_font_body},       {kIdOpenLogs, g_font_body}};
  for (const FontBinding& binding : bindings) {
    HWND control = ::GetDlgItem(window, binding.id);
    if (control != nullptr) {
      ::SendMessageW(control, WM_SETFONT,
                     reinterpret_cast<WPARAM>(binding.font), TRUE);
    }
  }
}

HWND CreateLabel(HWND window, int id, const wchar_t* text, int x, int y,
                 int width, int height, bool wrap) {
  const DWORD style = WS_CHILD | WS_VISIBLE | SS_LEFT |
                      (wrap ? 0 : SS_LEFTNOWORDWRAP);
  return ::CreateWindowExW(0, L"STATIC", text, style, x, y, width, height,
                           window,
                           reinterpret_cast<HMENU>(static_cast<INT_PTR>(id)),
                           ::GetModuleHandleW(nullptr), nullptr);
}

bool UpdateStatusText(const std::wstring& text) {
  if (g_window == nullptr) {
    return false;
  }
  HWND status = ::GetDlgItem(g_window, kIdStatus);
  if (status == nullptr) {
    return false;
  }
  ::SetWindowTextW(status, text.c_str());
  ::InvalidateRect(status, nullptr, TRUE);
  return true;
}

void OpenLogDirectory(HWND window) {
  std::wstring dir = StartupLogDirectoryWide();
  if (dir.empty()) {
    ::MessageBoxW(window, L"日志目录不可用：无法定位 %APPDATA% 目录。",
                 L"RDR-001", MB_OK | MB_ICONWARNING);
    return;
  }
  ::CreateDirectoryW(dir.c_str(), nullptr);
  const HINSTANCE result = ::ShellExecuteW(nullptr, L"open", dir.c_str(),
                                           nullptr, nullptr, SW_SHOWNORMAL);
  if (reinterpret_cast<INT_PTR>(result) <= 32) {
    ::MessageBoxW(window,
                 L"打不开日志目录。请手动访问：\r\n"
                 L"%APPDATA%\\RDC\\logs",
                 L"RDR-001", MB_OK | MB_ICONWARNING);
  }
}

void OpenDiagnosticsGuide(HWND window) {
  const std::wstring dir = ExeDirectory();
  if (dir.empty()) {
    return;
  }
  const std::wstring guide = dir + L"\\诊断说明.html";
  const DWORD attributes = ::GetFileAttributesW(guide.c_str());
  if (attributes == INVALID_FILE_ATTRIBUTES) {
    // 安装不完整时退回到安装目录，至少让用户能找到日志与文档。
    ::ShellExecuteW(nullptr, L"open", dir.c_str(), nullptr, nullptr,
                    SW_SHOWNORMAL);
    return;
  }
  ::ShellExecuteW(nullptr, L"open", guide.c_str(), nullptr, dir.c_str(),
                  SW_SHOWNORMAL);
}

void CopyDiagnostics(HWND window) {
  const std::wstring text = BuildDiagnosticsTextWide();
  if (text.empty()) {
    return;
  }
  bool copied = false;
  if (::OpenClipboard(window)) {
    ::EmptyClipboard();
    const size_t bytes = (text.size() + 1) * sizeof(wchar_t);
    HGLOBAL memory = ::GlobalAlloc(GMEM_MOVEABLE, bytes);
    if (memory != nullptr) {
      void* target = ::GlobalLock(memory);
      if (target != nullptr) {
        std::memcpy(target, text.c_str(), bytes);
        ::GlobalUnlock(memory);
        if (::SetClipboardData(CF_UNICODETEXT, memory) != nullptr) {
          copied = true;
        } else {
          ::GlobalFree(memory);
        }
      } else {
        ::GlobalFree(memory);
      }
    }
    ::CloseClipboard();
  }
  if (copied) {
    UpdateStatusText(L"诊断信息已复制到剪贴板，可直接粘贴发送。");
    AppendStartupLog("diag.export bytes=" +
                     std::to_string(static_cast<unsigned long long>(text.size())));
  } else {
    UpdateStatusText(L"复制失败：剪贴板被其他程序占用，请稍后重试。");
  }
}

// 兜底路径：没有可用的进程内重试句柄时，带着兼容渲染标记重新启动本身。
bool RelaunchWithCompatRenderer() {
  WriteRenderModeSetting("compat");
  ::SetEnvironmentVariableW(L"RDC_RENDERER", L"skia");
  ::SetEnvironmentVariableW(L"RDC_COMPAT", L"1");
  wchar_t module[MAX_PATH] = {0};
  if (::GetModuleFileNameW(nullptr, module, MAX_PATH) == 0) {
    return false;
  }
  std::wstring command = L"\"" + std::wstring(module) + L"\"";
  STARTUPINFOW startup{};
  startup.cb = static_cast<DWORD>(sizeof(startup));
  PROCESS_INFORMATION process{};
  const BOOL created = ::CreateProcessW(
      module, const_cast<wchar_t*>(command.c_str()), nullptr, nullptr, FALSE,
      0, nullptr, nullptr, &startup, &process);
  if (!created) {
    return false;
  }
  ::CloseHandle(process.hThread);
  ::CloseHandle(process.hProcess);
  AppendStartupLog("render.retry=relaunch backend=skia(compat)");
  return true;
}

void ExecuteRetry(HWND window) {
  if (g_retrying) {
    return;
  }
  g_retrying = true;
  UpdateStatusText(L"正在用兼容渲染重新初始化，请稍候（最多 10 秒）…");
  AppendStartupLog("render.retry from_backend=" + g_backend +
                   " to_backend=skia attempt=" + std::to_string(g_attempt + 1));
  bool started = false;
  if (g_retry_handler) {
    started = g_retry_handler();
  } else {
    started = RelaunchWithCompatRenderer();
  }
  if (!started) {
    g_retrying = false;
    UpdateStatusText(
        L"兼容渲染也没能初始化成功。请按上面第 2、3 步继续排查，"
        L"保底窗口会一直保留。");
    return;
  }
  if (!g_retry_handler) {
    // 重启进程的路径：本进程退出，由新进程继续。
    ::PostQuitMessage(0);
  }
}

void LayoutControls(HWND window) {
  const HINSTANCE instance = ::GetModuleHandleW(nullptr);
  CreateLabel(window, kIdHeading, kHeadingText, 32, 20, 796, 34, false);
  CreateLabel(window, kIdLine1, kLine1Text, 32, 62, 796, 24, false);
  CreateLabel(window, kIdLine2, kLine2Text, 32, 90, 796, 24, false);
  CreateLabel(window, kIdCause, g_cause.c_str(), 32, 120, 796, 20, true);
  CreateLabel(window, kIdSteps, kStepsText, 32, 148, 796, 150, true);
  CreateLabel(window, kIdDisplayHint, kDisplayHintText, 32, 302, 796, 24,
              false);
  CreateLabel(window, kIdStatus, L"", 32, 330, 796, 20, true);
  CreateLabel(window, kIdFooter, kFooterText, 32, 420, 796, 20, true);

  const DWORD button_style = WS_CHILD | WS_VISIBLE | WS_TABSTOP | BS_PUSHBUTTON;
  ::CreateWindowExW(0, L"BUTTON", L"用兼容渲染重试", button_style | BS_DEFPUSHBUTTON,
                    32, 360, 200, 42, window,
                    reinterpret_cast<HMENU>(static_cast<INT_PTR>(kIdRetry)),
                    instance, nullptr);
  ::CreateWindowExW(0, L"BUTTON", L"复制诊断", button_style, 244, 360, 150, 42,
                    window,
                    reinterpret_cast<HMENU>(static_cast<INT_PTR>(kIdCopy)),
                    instance, nullptr);
  ::CreateWindowExW(0, L"BUTTON", L"打开日志目录", button_style, 406, 360,
                    170, 42, window,
                    reinterpret_cast<HMENU>(static_cast<INT_PTR>(kIdOpenLogs)),
                    instance, nullptr);

  // SysLink 属于原生 comctl 控件；创建失败时退化为普通文字，功能不受影响。
  HWND link = ::CreateWindowExW(
      0, L"SysLink", kGuideLinkText, WS_CHILD | WS_VISIBLE | WS_TABSTOP, 32,
      446, 500, 22, window,
      reinterpret_cast<HMENU>(static_cast<INT_PTR>(kIdGuide)), instance,
      nullptr);
  if (link == nullptr) {
    CreateLabel(window, kIdGuide, L"诊断说明：安装目录下的「诊断说明.html」",
                32, 446, 500, 22, false);
  }

  ApplyFonts(window);
}

LRESULT HandleControlColor(UINT message, WPARAM wparam, LPARAM lparam) {
  HDC device = reinterpret_cast<HDC>(wparam);
  HWND control = reinterpret_cast<HWND>(lparam);
  const int id = ::GetDlgCtrlID(control);
  COLORREF color = ::GetSysColor(COLOR_WINDOWTEXT);
  switch (id) {
    case kIdHeading:
      color = RGB(0xB4, 0x23, 0x18);
      break;
    case kIdDisplayHint:
      color = RGB(0xB4, 0x5C, 0x00);
      break;
    case kIdCause:
    case kIdStatus:
    case kIdFooter:
    case kIdGuide:
      color = RGB(0x66, 0x66, 0x66);
      break;
    default:
      break;
  }
  ::SetTextColor(device, color);
  ::SetBkMode(device, TRANSPARENT);
  if (message == WM_CTLCOLORSTATIC) {
    return reinterpret_cast<LRESULT>(::GetSysColorBrush(COLOR_WINDOW));
  }
  return FALSE;
}

LRESULT CALLBACK FallbackProc(HWND window, UINT message, WPARAM wparam,
                              LPARAM lparam) {
  switch (message) {
    case WM_CREATE:
      LayoutControls(window);
      return 0;
    case WM_CTLCOLORSTATIC:
      return HandleControlColor(message, wparam, lparam);
    case WM_COMMAND: {
      switch (LOWORD(wparam)) {
        case kIdRetry:
          ::PostMessageW(window, kMessageExecuteRetry, 0, 0);
          return 0;
        case kIdCopy:
          CopyDiagnostics(window);
          return 0;
        case kIdOpenLogs:
          OpenLogDirectory(window);
          return 0;
        default:
          break;
      }
      break;
    }
    case kMessageExecuteRetry:
      ExecuteRetry(window);
      return 0;
    case WM_NOTIFY: {
      const NMHDR* header = reinterpret_cast<const NMHDR*>(lparam);
      if (header != nullptr && header->idFrom == kIdGuide &&
          header->code == NM_CLICK) {
        OpenDiagnosticsGuide(window);
        return 0;
      }
      break;
    }
    case WM_CLOSE:
      ::DestroyWindow(window);
      return 0;
    case WM_DESTROY:
      g_window = nullptr;
      if (g_font_heading != nullptr) {
        ::DeleteObject(g_font_heading);
        g_font_heading = nullptr;
      }
      if (g_font_body != nullptr) {
        ::DeleteObject(g_font_body);
        g_font_body = nullptr;
      }
      if (g_font_small != nullptr) {
        ::DeleteObject(g_font_small);
        g_font_small = nullptr;
      }
      ::PostQuitMessage(0);
      return 0;
    default:
      break;
  }
  return ::DefWindowProcW(window, message, wparam, lparam);
}

bool RegisterFallbackClass() {
  static bool registered = false;
  if (registered) {
    return true;
  }
  WNDCLASSEXW window_class{};
  window_class.cbSize = sizeof(window_class);
  window_class.style = CS_HREDRAW | CS_VREDRAW;
  window_class.lpfnWndProc = FallbackProc;
  window_class.hInstance = ::GetModuleHandleW(nullptr);
  window_class.hCursor = ::LoadCursorW(nullptr, IDC_ARROW);
  window_class.hbrBackground =
      reinterpret_cast<HBRUSH>(static_cast<INT_PTR>(COLOR_WINDOW + 1));
  window_class.lpszClassName = kWindowClass;
  window_class.hIcon = ::LoadIconW(nullptr, IDI_APPLICATION);
  if (::RegisterClassExW(&window_class) == 0 &&
      ::GetLastError() != ERROR_CLASS_ALREADY_EXISTS) {
    return false;
  }
  registered = true;
  return true;
}

void CreateFonts() {
  if (g_font_heading != nullptr) {
    return;
  }
  g_font_heading = ::CreateFontW(-28, 0, 0, 0, FW_BOLD, FALSE, FALSE, FALSE,
                                 DEFAULT_CHARSET, OUT_DEFAULT_PRECIS,
                                 CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY,
                                 DEFAULT_PITCH | FF_DONTCARE,
                                 L"Microsoft YaHei UI");
  g_font_body = ::CreateFontW(-17, 0, 0, 0, FW_NORMAL, FALSE, FALSE, FALSE,
                              DEFAULT_CHARSET, OUT_DEFAULT_PRECIS,
                              CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY,
                              DEFAULT_PITCH | FF_DONTCARE,
                              L"Microsoft YaHei UI");
  g_font_small = ::CreateFontW(-15, 0, 0, 0, FW_NORMAL, FALSE, FALSE, FALSE,
                               DEFAULT_CHARSET, OUT_DEFAULT_PRECIS,
                               CLIP_DEFAULT_PRECIS, CLEARTYPE_QUALITY,
                               DEFAULT_PITCH | FF_DONTCARE,
                               L"Microsoft YaHei UI");
}

}  // namespace

void SetContext(const std::string& backend, const std::wstring& cause,
                int attempt_index) {
  if (!backend.empty()) {
    g_backend = backend;
  }
  if (!cause.empty()) {
    g_cause = cause;
  }
  if (attempt_index > 0) {
    g_attempt = attempt_index;
  }
}

void SetRetryHandler(std::function<bool()> handler) {
  g_retry_handler = std::move(handler);
}

bool Show(HWND parent) {
  if (g_window != nullptr) {
    UpdateStatusText(L"");
    RememberHiddenParent(parent);
    ::ShowWindow(g_window, SW_SHOW);
    ::BringWindowToTop(g_window);
    ::SetForegroundWindow(g_window);
    return true;
  }
  if (!RegisterFallbackClass()) {
    return false;
  }
  INITCOMMONCONTROLSEX common_controls{};
  common_controls.dwSize = static_cast<DWORD>(sizeof(common_controls));
  common_controls.dwICC = ICC_LINK_CLASS;
  ::InitCommonControlsEx(&common_controls);
  CreateFonts();

  RECT work_area{};
  ::SystemParametersInfoW(SPI_GETWORKAREA, 0, &work_area, 0);
  RECT window_rect{0, 0, kClientWidth, kClientHeight};
  const DWORD style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
  ::AdjustWindowRect(&window_rect, style, FALSE);
  const int width = window_rect.right - window_rect.left;
  const int height = window_rect.bottom - window_rect.top;
  const int x = work_area.left + ((work_area.right - work_area.left) - width) / 2;
  const int y = work_area.top + ((work_area.bottom - work_area.top) - height) / 2;

  g_window = ::CreateWindowExW(0, kWindowClass, L"程序界面无法显示（RDR-001）",
                               style, x, y, width, height, nullptr, nullptr,
                               ::GetModuleHandleW(nullptr), nullptr);
  if (g_window == nullptr) {
    return false;
  }
  RememberHiddenParent(parent);
  ::ShowWindow(g_window, SW_SHOW);
  ::UpdateWindow(g_window);
  ::SetForegroundWindow(g_window);
  AppendStartupLog("render.fallback_shown code=RDR-001 backend=" + g_backend +
                   " attempt=" + std::to_string(g_attempt));
  return true;
}

bool IsShown() {
  return g_window != nullptr && ::IsWindowVisible(g_window) != FALSE;
}

void Hide() {
  g_retrying = false;
  if (g_window != nullptr && ::IsWindowVisible(g_window)) {
    ::ShowWindow(g_window, SW_HIDE);
  }
  if (g_hidden_parent != nullptr) {
    ::ShowWindow(g_hidden_parent, SW_SHOW);
  }
  UpdateStatusText(L"");
}

void SetStatusText(const std::wstring& text) {
  UpdateStatusText(text);
}

void Shutdown() {
  if (g_window != nullptr) {
    ::DestroyWindow(g_window);
  }
}

std::wstring BuildDiagnosticsTextWide() {
  std::wstringstream out;
  out << L"RDC 渲染故障诊断\r\n";
  out << L"错误码: " << kRenderErrorCode << L" 渲染初始化失败\r\n";
  out << L"触发条件: " << g_cause << L"\r\n";
  out << L"时间: " << Timestamp() << L"\r\n";
  out << L"版本: " << ReadAppVersion() << L"\r\n";
  out << L"平台: " << WideFromUtf8(DescribeWindowsVersion()) << L"\r\n";
  out << L"渲染后端: " << WideFromUtf8(g_backend) << L"（第 "
      << g_attempt << L" 次尝试）\r\n";
  out << L"显卡: " << WideFromUtf8(DescribePrimaryAdapter())
      << L"（" << WideFromUtf8(DescribeGpuVendor()) << L"）\r\n";
  out << L"驱动版本: " << WideFromUtf8(DescribeDriverVersion())
      << L"\r\n";
  out << L"驱动日期: " << WideFromUtf8(DescribeDriverDate())
      << L"\r\n";
  out << L"显卡 BIOS: " << WideFromUtf8(DescribeVideoBiosVersion())
      << L"\r\n";
  out << L"WDDM: " << WideFromUtf8(DescribeWddmVersion()) << L"\r\n";
  out << L"会话类型: " << (IsRemoteSession() ? L"远程会话" : L"本地会话")
      << L"\r\n";
  out << L"日志目录: " << StartupLogDirectoryWide() << L"\r\n";

  const std::deque<std::wstring> lines = LastLogLines(5);
  out << L"最近 5 条事件:\r\n";
  if (lines.empty()) {
    out << L"  （日志缺失）\r\n";
  } else {
    for (const std::wstring& line : lines) {
      out << L"  " << line << L"\r\n";
    }
  }
  return out.str();
}

std::string BuildDiagnosticsText() {
  const std::wstring text = BuildDiagnosticsTextWide();
  return Utf8FromUtf16(text.c_str());
}

}  // namespace rdc_fallback
