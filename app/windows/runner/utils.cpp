#include "utils.h"

#include <flutter_windows.h>
#include <io.h>
#include <stdio.h>
#include <windows.h>

#include <chrono>
#include <cstdio>
#include <cstdlib>
#include <ctime>
#include <cwchar>
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

std::wstring ResolveUserDataDirectory() {
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
  return (base / L"RDC").wstring();
}

// Reads a REG_SZ (or expandable string) value. Returns an empty string on
// failure; diagnostics code prefers "未知" over throwing.
std::wstring ReadRegistryString(HKEY root, const std::wstring& subkey,
                                const std::wstring& name) {
  wchar_t buffer[1024] = {0};
  DWORD size = static_cast<DWORD>(sizeof(buffer));
  DWORD type = 0;
  const LONG status = ::RegGetValueW(
      root, subkey.c_str(), name.c_str(),
      RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ, &type, buffer, &size);
  if (status != ERROR_SUCCESS) {
    return std::wstring();
  }
  return std::wstring(buffer);
}

// Some GPU BIOS values are REG_MULTI_SZ (one entry per line). Join them with
// "; " so the clipboard text stays single-line-friendly.
std::wstring ReadRegistryMultiString(HKEY root, const std::wstring& subkey,
                                     const std::wstring& name) {
  wchar_t buffer[2048] = {0};
  DWORD size = static_cast<DWORD>(sizeof(buffer));
  DWORD type = 0;
  const LONG status = ::RegGetValueW(root, subkey.c_str(), name.c_str(),
                                     RRF_RT_REG_MULTI_SZ, &type, buffer, &size);
  if (status != ERROR_SUCCESS) {
    return std::wstring();
  }
  std::wstring joined;
  for (const wchar_t* item = buffer; *item != L'\0'; item += wcslen(item) + 1) {
    if (!joined.empty()) {
      joined += L"; ";
    }
    joined += item;
  }
  return joined;
}

std::wstring DisplayClassKey() {
  const std::wstring base =
      L"SYSTEM\\CurrentControlSet\\Control\\Class\\"
      L"{4d36e968-e325-11ce-bfc1-08002be10318}";
  for (int index = 0; index < 16; ++index) {
    wchar_t subkey[256] = {0};
    std::swprintf(subkey, 256, L"%s\\%04d", base.c_str(), index);
    if (!ReadRegistryString(HKEY_LOCAL_MACHINE, subkey, L"DriverVersion").empty()) {
      return subkey;
    }
  }
  return std::wstring();
}

std::string OrUnknown(const std::wstring& value) {
  return value.empty() ? std::string("未知") : Utf8FromUtf16(value.c_str());
}

}  // namespace

std::wstring StartupLogDirectoryWide() {
  const std::wstring base = ResolveUserDataDirectory();
  if (base.empty()) {
    return std::wstring();
  }
  return (std::filesystem::path(base) / L"logs").wstring();
}

std::wstring StartupLogFilePathWide() {
  const std::wstring dir = StartupLogDirectoryWide();
  if (dir.empty()) {
    return std::wstring();
  }
  return (std::filesystem::path(dir) / L"startup.log").wstring();
}

std::string ReadRenderModeSetting() {
  const std::wstring base = ResolveUserDataDirectory();
  if (base.empty()) {
    return "standard";
  }
  const std::filesystem::path path =
      std::filesystem::path(base) / L"render_mode.txt";
  std::ifstream in(path.c_str());
  if (!in.is_open()) {
    return "standard";
  }
  std::string value;
  std::getline(in, value);
  if (value == "compat") {
    return "compat";
  }
  return "standard";
}

bool WriteRenderModeSetting(const std::string& mode) {
  const std::wstring base = ResolveUserDataDirectory();
  if (base.empty()) {
    return false;
  }
  try {
    const std::filesystem::path dir(base);
    std::filesystem::create_directories(dir);
    std::ofstream out((dir / L"render_mode.txt").c_str(), std::ios::trunc);
    if (!out.is_open()) {
      return false;
    }
    out << (mode == "compat" ? "compat" : "standard") << "\n";
    out.flush();
    return true;
  } catch (...) {
    return false;
  }
}

std::string DescribeGpuVendor() {
  const std::string adapter = DescribePrimaryAdapter();
  if (adapter.find("Radeon") != std::string::npos ||
      adapter.find("AMD") != std::string::npos) {
    return "AMD";
  }
  if (adapter.find("NVIDIA") != std::string::npos ||
      adapter.find("GeForce") != std::string::npos) {
    return "NVIDIA";
  }
  if (adapter.find("Intel") != std::string::npos) {
    return "Intel";
  }
  return "未知";
}

std::string DescribeDriverVersion() {
  const std::wstring key = DisplayClassKey();
  if (key.empty()) {
    return "未知";
  }
  return OrUnknown(ReadRegistryString(HKEY_LOCAL_MACHINE, key, L"DriverVersion"));
}

std::string DescribeDriverDate() {
  const std::wstring key = DisplayClassKey();
  if (key.empty()) {
    return "未知";
  }
  return OrUnknown(ReadRegistryString(HKEY_LOCAL_MACHINE, key, L"DriverDate"));
}

std::string DescribeVideoBiosVersion() {
  const std::wstring key = DisplayClassKey();
  if (key.empty()) {
    return "未知";
  }
  const std::wstring multi =
      ReadRegistryMultiString(HKEY_LOCAL_MACHINE, key, L"VideoBIOSVersion");
  if (!multi.empty()) {
    return Utf8FromUtf16(multi.c_str());
  }
  return OrUnknown(ReadRegistryString(HKEY_LOCAL_MACHINE, key, L"VideoBIOSVersion"));
}

std::string DescribeWddmVersion() {
  DWORD value = 0;
  DWORD size = static_cast<DWORD>(sizeof(value));
  DWORD type = 0;
  const LONG status = ::RegGetValueW(
      HKEY_LOCAL_MACHINE, L"SYSTEM\\CurrentControlSet\\Control\\GraphicsDrivers",
      L"WddmVersion", RRF_RT_REG_DWORD, &type, &value, &size);
  if (status != ERROR_SUCCESS) {
    return "未知（注册表无 WddmVersion）";
  }
  char buffer[64] = {0};
  std::snprintf(buffer, sizeof(buffer), "0x%08X（注册表原始值）",
                static_cast<unsigned int>(value));
  return std::string(buffer);
}

std::string DescribeWindowsVersion() {
  const std::wstring product = ReadRegistryString(
      HKEY_LOCAL_MACHINE,
      L"SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion", L"ProductName");
  const std::wstring build = ReadRegistryString(
      HKEY_LOCAL_MACHINE,
      L"SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion", L"CurrentBuildNumber");
  std::string result = product.empty() ? "Windows" : Utf8FromUtf16(product.c_str());
  if (!build.empty()) {
    result += " (build ";
    result += Utf8FromUtf16(build.c_str());
    result += ")";
  }
  result += " x64";
  return result;
}

void AppendStartupLog(const std::string& line) {
  std::lock_guard<std::mutex> lock(StartupLogMutex());
  const std::wstring path = StartupLogFilePathWide();
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
