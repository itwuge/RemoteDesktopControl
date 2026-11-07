// rdc-codec / NVENC 原生编码 shim（内部头，不对外发布）
//
// 用途：Phase 1 只负责在运行期探测系统 `nvEncodeAPI64.dll` 是否存在、能否导出版本查询，
//       并把结果与错误文本通过 C ABI 交回 Rust；编码会话留待 Phase 2 在 N 卡实机实现。
//
// 重要边界：
// - 本 shim 产物是 `rdc-nvenc.dll`，由本仓库构建、随安装包分发；
// - 系统驱动 DLL（`nvEncodeAPI64.dll` / `nvcuda.dll`）**禁止**随包分发，
//   只允许运行期 `LoadLibrary` 加载；缺失时返回明确状态码，由选择链回退到 MFT；
// - 状态码必须与 `crates/rdc-codec/src/platform/windows/nvenc.rs` 顶部常量严格一致。
#pragma once

#include <cstdint>

// =====** 状态码（与 Rust 侧常量一一对应） =====**
#define RDCNVENC_STATUS_OK 0                    // API 可用（Phase 1 仍无会话）
#define RDCNVENC_STATUS_SHIM_MISSING 1          // 本仓库 shim 缺失/符号缺失（Rust 侧判定）
#define RDCNVENC_STATUS_DRIVER_DLL_MISSING 2    // 系统 nvEncodeAPI64.dll 缺失
#define RDCNVENC_STATUS_ENTRY_MISSING 3         // 系统 DLL 缺少版本查询导出
#define RDCNVENC_STATUS_VERSION_UNSUPPORTED 4   // 系统 API 版本低于 12.2（编码值 194）
#define RDCNVENC_STATUS_SESSION_NOT_IMPLEMENTED 5 // Phase 1 骨架：会话未实装
#define RDCNVENC_STATUS_UNKNOWN 99              // 其它未知失败

// =====** C ABI（rdc-nvenc.dll 导出；Rust 侧按此绑定） =====**
extern "C" {
/// 探测系统 NVENC API。
///
/// 参数：
/// - api_version：输出的本 shim 依赖的 API 版本（编码值，12.2 = 194 = (12 << 4) | 2）
/// - max_version：驱动返回的最高支持版本（编码值）
/// - h264_available：Phase 1 固定为 0（未创建会话，不宣称真实能力）
///
/// 返回：RDCNVENC_STATUS_* 状态码。
__declspec(dllexport) int rdcnvenc_probe(unsigned int* api_version, unsigned int* max_version,
                                         int* h264_available);

/// 最近一次探测的错误/说明文本（静态字符串，进程生命周期内有效）。
__declspec(dllexport) const char* rdcnvenc_last_error(void);
}
