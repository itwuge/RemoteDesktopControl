#include "utils.h"

#include <flutter_windows.h>
#include <io.h>
#include <stdio.h>
#include <windows.h>

#include <chrono>
#include <cstdlib>
#include <ctime>
#include <filesystem>
#include <fstream>
#include <iostream>
#include <mutex>
#include <sstream>

void CreateAndAttachConsole() {
  if (::AllocConsole()) {
    FILE *unused;
    if (freopen_s(&unused, "CONOUT$", "w", stdout)) {
      _dup2(_fileno(stdout), 1);
    }
    if (freopen_s(&unused, "CONOUT$", "w", stderr)) {
      _dup2(_fileno(stdout), 2);
    }
    std::ios::sync_with_stdio();
    FlutterDesktopResyncOutputStreams();
  }
}

std::vector<std::string> GetCommandLineArguments() {
  // Convert the UTF-16 command line arguments to UTF-8 for the Engine to use.
  int argc;
  wchar_t** argv = ::CommandLineToArgvW(::GetCommandLineW(), &argc);
  if (argv == nullptr) {
    return std::vector<std::string>();
  }

  std::vector<std::string> command_line_arguments;

  // Skip the first argument as it's the binary name.
  for (int i = 1; i < argc; i++) {
    command_line_arguments.push_back(Utf8FromUtf16(argv[i]));
  }

  ::LocalFree(argv);

  return command_line_arguments;
}

std::string Utf8FromUtf16(const wchar_t* utf16_string) {
  if (utf16_string == nullptr) {
    return std::string();
  }
  // First, find the length of the string with a safe upper bound (CWE-126).
  // UNICODE_STRING_MAX_CHARS (32767) is the maximum length of a UNICODE_STRING.
  int input_length = static_cast<int>(wcsnlen(utf16_string, UNICODE_STRING_MAX_CHARS));
  // Now use that bounded length to determine the required buffer size.
  // When an explicit length is passed, WideCharToMultiByte does not include
  // the null terminator in its returned size.
  int target_length = ::WideCharToMultiByte(
      CP_UTF8, WC_ERR_INVALID_CHARS, utf16_string,
      input_length, nullptr, 0, nullptr, nullptr);
  std::string utf8_string;
  if (target_length == 0 || static_cast<size_t>(target_length) > utf8_string.max_size()) {
    return utf8_string;
  }
  utf8_string.resize(target_length);
  int converted_length = ::WideCharToMultiByte(
      CP_UTF8, WC_ERR_INVALID_CHARS, utf16_string,
      input_length, utf8_string.data(), target_length, nullptr, nullptr);
  if (converted_length == 0) {
    return std::string();
  }
  return utf8_string;
}

namespace {

std::mutex& StartupLogMutex() {
  static std::mutex mutex;
  return mutex;
}

std::wstring StartupLogPath() {
  constexpr DWORD kAppDataCapacity = MAX_PATH * 2;
  wchar_t appdata[kAppDataCapacity];
  DWORD appdata_length = ::GetEnvironmentVariableW(L"APPDATA", appdata, kAppDataCapacity);
  std::filesystem::path base;
  if (appdata_length != 0 && appdata_length < kAppDataCapacity) {
    base = appdata;
  } else {
    wchar_t buffer[MAX_PATH];
    if (::GetTempPathW(MAX_PATH, buffer) == 0) {
      return std::wstring();
    }
    base = buffer;
  }
  return (base / L"RDC" / L"logs" / L"startup.log").wstring();
}

}  // namespace

void AppendStartupLog(const std::string& line) {
  std::lock_guard<std::mutex> lock(StartupLogMutex());
  const std::wstring path = StartupLogPath();
  if (path.empty()) {
    return;
  }
  try {
    const std::filesystem::path log_path(path);
    std::filesystem::create_directories(log_path.parent_path());
    std::ofstream out(log_path, std::ios::app);
    if (!out.is_open()) {
      return;
    }
    std::time_t now = std::time(nullptr);
    std::tm local{};
    (void)localtime_s(&local, &now);
    char stamp[32] = {0};
    std::strftime(stamp, sizeof(stamp), "%Y-%m-%d %H:%M:%S", &local);
    out << "[" << stamp << "] " << line << "\n";
    out.flush();
  } catch (...) {
    // Logging must never take the app down.
  }
}

std::string DescribePrimaryAdapter() {
  DISPLAY_DEVICEW device{};
  device.cb = static_cast<DWORD>(sizeof(device));
  if (::EnumDisplayDevicesW(nullptr, 0, &device, 0)) {
    return Utf8FromUtf16(device.DeviceString);
  }
  return std::string();
}

bool IsRemoteSession() {
  return ::GetSystemMetrics(SM_REMOTESESSION) != 0;
}
