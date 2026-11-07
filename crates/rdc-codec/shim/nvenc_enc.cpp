// rdc-codec / NVENC 原生编码 shim（Phase 1 探测骨架）
//
// 实现范围：只做系统 `nvEncodeAPI64.dll` 的加载与版本探测，不创建 NVENC 会话。
// 这样在没有 N 卡的开发机上也能编译、打包，并验证「不可用时安全回退」这条路径。
#include "nvenc_shim.h"

#include <windows.h>

#include <string>

namespace {

// 本项目锁定的 NVENC API 版本：nv-codec-headers 12.2。
// `NvEncodeAPIGetMaxSupportedVersion` 的编码规则：低 4 位为 minor，其余位为 major，
// 因此 12.2 = (12 << 4) | 2 = 194（不能与头文件里用于结构体版本的 NVENCAPI_VERSION 混用）。
constexpr unsigned int kRequiredApiVersion = 194u;

// 系统 nvEncodeAPI64.dll 句柄；进程内只加载一次，退出时由系统回收。
HMODULE g_api = nullptr;

// 最近一次错误/说明文本。
std::string g_error;

// `NvEncodeAPIGetMaxSupportedVersion` 的签名（避免依赖 NVENC SDK 头文件）。
using GetMaxVersionFn = int(WINAPI*)(unsigned int* version);

} // namespace

extern "C" int rdcnvenc_probe(unsigned int* api_version, unsigned int* max_version,
                              int* h264_available) {
    if (api_version != nullptr) {
        *api_version = 0;
    }
    if (max_version != nullptr) {
        *max_version = 0;
    }
    if (h264_available != nullptr) {
        // Phase 1 不创建编码会话，不能把「DLL 存在」当成「H.264 可用」。
        *h264_available = 0;
    }
    g_error.clear();

    // 运行期加载系统驱动 DLL；发布包不携带它。
    if (g_api == nullptr) {
        g_api = LoadLibraryW(L"nvEncodeAPI64.dll");
    }
    if (g_api == nullptr) {
        g_error = "LoadLibrary(nvEncodeAPI64.dll) 失败，GetLastError=" +
                  std::to_string(GetLastError());
        return RDCNVENC_STATUS_DRIVER_DLL_MISSING;
    }

    auto get_max_version =
        reinterpret_cast<GetMaxVersionFn>(GetProcAddress(g_api, "NvEncodeAPIGetMaxSupportedVersion"));
    if (get_max_version == nullptr) {
        g_error = "nvEncodeAPI64.dll 缺少导出 NvEncodeAPIGetMaxSupportedVersion";
        return RDCNVENC_STATUS_ENTRY_MISSING;
    }

    unsigned int driver_max = 0;
    const int status = get_max_version(&driver_max);
    if (status != 0) {
        g_error = "NvEncodeAPIGetMaxSupportedVersion 返回 " + std::to_string(status);
        return RDCNVENC_STATUS_UNKNOWN;
    }
    if (api_version != nullptr) {
        *api_version = kRequiredApiVersion;
    }
    if (max_version != nullptr) {
        *max_version = driver_max;
    }
    if (driver_max < kRequiredApiVersion) {
        g_error = "NVENC API 版本过低：驱动最高 " + std::to_string(driver_max) +
                  "，本项目要求 " + std::to_string(kRequiredApiVersion) + "（12.2）";
        return RDCNVENC_STATUS_VERSION_UNSUPPORTED;
    }

    // API 版本满足要求，但 Phase 1 没有编码会话。
    g_error = "Phase 1 骨架：NVENC API 可用，编码会话尚未实现（未验证）";
    return RDCNVENC_STATUS_OK;
}

extern "C" const char* rdcnvenc_last_error(void) {
    return g_error.c_str();
}
