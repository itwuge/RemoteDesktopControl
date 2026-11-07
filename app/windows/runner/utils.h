#ifndef RUNNER_UTILS_H_
#define RUNNER_UTILS_H_

#include <string>
#include <vector>

// Creates a console for the process, and redirects stdout and stderr to
// it for both the runner and the Flutter library.
void CreateAndAttachConsole();

// Takes a null-terminated wchar_t* encoded in UTF-16 and returns a std::string
// encoded in UTF-8. Returns an empty std::string on failure.
std::string Utf8FromUtf16(const wchar_t* utf16_string);

// Gets the command line arguments passed in as a std::vector<std::string>,
// encoded in UTF-8. Returns an empty std::vector<std::string> on failure.
std::vector<std::string> GetCommandLineArguments();

// Appends one timestamped line to %APPDATA%\RDC\logs\startup.log, creating
// the directory when missing. Thread-safe; failures are silently ignored so
// logging can never take the app down.
void AppendStartupLog(const std::string& line);

// Returns the primary display adapter description (diagnostics only).
std::string DescribePrimaryAdapter();

// 活跃显示输出（接在桌面上的输出）检测结果。
// count==0 即 RDR-001 的 no_display_output：显示器关闭/未连接/无虚拟屏。
struct DisplayOutputState {
  int count = 0;
  bool has_physical = false;
  bool has_virtual = false;
};

// 枚举接在桌面上的显示输出。只读系统状态，失败时按 count=0 返回。
DisplayOutputState DetectDisplayOutputs();

// 返回写入 startup.log 的一行代码，例如
// "active count=1 sources=virtual" 或 "none count=0 sources=none"。
std::string DescribeDisplayOutputsCode(const DisplayOutputState& state);

// True when the process is running inside a remote desktop session.
bool IsRemoteSession();

// Absolute path of the startup log file (empty when unavailable).
std::wstring StartupLogFilePathWide();

// Directory that contains the startup log (empty when unavailable).
std::wstring StartupLogDirectoryWide();

// Persisted render mode: "compat" 表示上一次用户选择了兼容渲染，其余为 "standard".
// 文件位置：%APPDATA%\RDC\render_mode.txt。
std::string ReadRenderModeSetting();
bool WriteRenderModeSetting(const std::string& mode);

// 诊断信息（尽力而为，失败时返回 "未知"，绝不抛异常）。
std::string DescribeGpuVendor();
std::string DescribeDriverVersion();
std::string DescribeDriverDate();
std::string DescribeVideoBiosVersion();
std::string DescribeWddmVersion();
std::string DescribeWindowsVersion();

#endif  // RUNNER_UTILS_H_
