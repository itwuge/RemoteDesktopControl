//! rdc-codec / NVIDIA NVENC 编码器后端（Windows，Phase 1 骨架）
//!
//! 层级：L3 编解码层 · 平台适配（`platform/windows.rs` 的子模块）
//! 用途：把 NVENC 纳入后端回退链（AMF → NVENC → oneVPL → MFT → 软编），
//!       并在运行期探测系统 `nvEncodeAPI64.dll` 是否可用。
//! 调用方：`platform::selection`（`RDC_ENCODER` 覆盖与能力探测）。
//!
//! 实现形态（沿用 AMF 已验证的「C++ 薄 shim + Rust 薄封装」）：
//! - `crates/rdc-codec/shim/nvenc_enc.cpp` → `rdc-nvenc.dll` 只负责
//!   `LoadLibrary("nvEncodeAPI64.dll")`、读取 API 版本与错误文本；
//! - 系统驱动 DLL（`nvEncodeAPI64.dll` / `nvcuda.dll`）**不随安装包分发**，
//!   缺失时返回明确状态码并由选择链安全回退到 MFT；
//! - Phase 1 只实现探测骨架：`probe()` 返回空列表，`configure()` 返回
//!   [`CodecError::HardwareUnsupported`]，绝不假装“已启用 NVENC”。
//!
//! 诚实边界：本机无 N 卡，NVENC 真启用、1080p60、p99、掉帧、10 分钟稳定性、
//! 并发会话上限、颜色通道一律「未验证」，必须等 N 卡实机按
//! `logs/PLAN-NVENC-20261108.md` Phase 2 验收。

#![allow(non_snake_case)]

use std::ffi::{c_char, CStr};
use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;

use rdc_runtime::PooledBuffer;
use windows::core::{PCSTR, PCWSTR};
use windows::Win32::Foundation::{FreeLibrary, HMODULE};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

use crate::encoder::{CodecKind, EncodedPacket, EncoderCaps, VideoConfig, VideoEncoder};
use crate::error::CodecError;

// =====** 状态码（与 shim nvenc_shim.h 严格一致） =====**

/// 本仓库 shim `rdc-nvenc.dll` 缺失或缺少导出符号。
pub const STATUS_SHIM_MISSING: u32 = 1;
/// 系统驱动 DLL `nvEncodeAPI64.dll` 缺失（未安装 N 卡驱动或非 N 卡机器）。
pub const STATUS_DRIVER_DLL_MISSING: u32 = 2;
/// 系统 DLL 缺少 `NvEncodeAPIGetMaxSupportedVersion` 导出。
pub const STATUS_ENTRY_MISSING: u32 = 3;
/// 系统 NVENC API 版本低于 12.2（编码值 194 = `(12 << 4) | 2`）。
pub const STATUS_VERSION_UNSUPPORTED: u32 = 4;
/// Phase 1 骨架：API 可用但编码会话尚未实现。
pub const STATUS_SESSION_NOT_IMPLEMENTED: u32 = 5;
/// 其它未知失败。
pub const STATUS_UNKNOWN: u32 = 99;

// =====** shim C ABI =====**

/// `rdcnvenc_probe(api_version, max_version, h264_available) -> status`。
type ProbeFn = unsafe extern "C" fn(*mut u32, *mut u32, *mut i32) -> i32;
/// `rdcnvenc_last_error() -> const char*`。
type LastErrorFn = unsafe extern "C" fn() -> *const c_char;

/// NVENC 不可用详情（用于回退日志，禁止只写「不可用」而不带码因）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NvencUnavailable {
    /// 状态码（见本模块顶部常量）。
    pub code: u32,
    /// 原因文本（优先取 shim 的 `rdcnvenc_last_error()`）。
    pub message: String,
}

impl NvencUnavailable {
    /// 落日志用的单行说明，包含 `code=` 与 `msg=`。
    pub fn note(&self) -> String {
        format!("NVENC 不可用（code={} msg={}）", self.code, self.message)
    }
}

/// NVENC 后端实例（Phase 1 只持有 shim 与探测结果）。
pub struct NvencEncoder {
    /// 本仓库 `rdc-nvenc.dll` 句柄。
    _lib: HMODULE,
    /// Phase 1 的能力说明（`code=5`），供选择链写进回退理由。
    note: String,
}

// 与 AMF/MFT 同一约定：NVENC 会话由单一编码线程独占；shim 句柄本身可跨线程持有。
unsafe impl Send for NvencEncoder {}

impl NvencEncoder {
    /// 尝试加载 shim 并探测系统 NVENC API。
    ///
    /// # Returns
    /// - `Ok(encoder)`：`rdc-nvenc.dll` 可用且系统 API 版本满足要求；
    ///   Phase 1 仍不可编码，`probe()` 为空，`note` 带 `code=5`。
    /// - `Err(NvencUnavailable)`：shim 缺失、系统 DLL 缺失、入口缺失或版本过低。
    ///
    /// # Errors
    /// 任何不可用情形都返回 [`NvencUnavailable`]，由选择链安全回退，不 panic。
    pub fn try_new() -> Result<Self, NvencUnavailable> {
        let lib = load_shim().ok_or_else(|| NvencUnavailable {
            code: STATUS_SHIM_MISSING,
            message: "未找到 rdc-nvenc.dll（先运行 crates/rdc-codec/shim/build_nvenc.ps1）"
                .to_owned(),
        })?;
        unsafe {
            let probe: ProbeFn = symbol(lib, b"rdcnvenc_probe\0").ok_or_else(|| {
                let _ = FreeLibrary(lib);
                NvencUnavailable {
                    code: STATUS_SHIM_MISSING,
                    message: "rdc-nvenc.dll 缺少导出符号 rdcnvenc_probe".to_owned(),
                }
            })?;
            let last_error: LastErrorFn =
                symbol(lib, b"rdcnvenc_last_error\0").ok_or_else(|| {
                    let _ = FreeLibrary(lib);
                    NvencUnavailable {
                        code: STATUS_SHIM_MISSING,
                        message: "rdc-nvenc.dll 缺少导出符号 rdcnvenc_last_error".to_owned(),
                    }
                })?;

            let mut api_version = 0u32;
            let mut max_version = 0u32;
            let mut h264_available = 0i32;
            let status = probe(&mut api_version, &mut max_version, &mut h264_available);
            if status != 0 {
                let message = last_error_text(last_error);
                let _ = FreeLibrary(lib);
                let code = status_code(status);
                eprintln!("[enc] nvenc.probe dll=nvEncodeAPI64.dll status={code} msg={message}");
                return Err(NvencUnavailable { code, message });
            }

            // Phase 1：即使 API 可用也不创建会话，避免在无实机条件下声称“已启用”。
            let note = format!(
                "NVENC API 可用（dll=nvEncodeAPI64.dll api={api_version} max={max_version}），\
                 但 Phase 1 编码会话尚未实现（code={STATUS_SESSION_NOT_IMPLEMENTED}，未验证）"
            );
            eprintln!(
                "[enc] nvenc.probe dll=nvEncodeAPI64.dll api={api_version} max={max_version} \
                 session=not-implemented"
            );
            Ok(Self { _lib: lib, note })
        }
    }

    /// Phase 1 能力说明（含状态码，写进选择链理由）。
    pub fn note(&self) -> &str {
        &self.note
    }
}

impl Drop for NvencEncoder {
    /// 释放 shim 句柄；系统 `nvEncodeAPI64.dll` 由 shim 内部持有，进程退出时自然释放。
    fn drop(&mut self) {
        unsafe {
            let _ = FreeLibrary(self._lib);
        }
    }
}

impl VideoEncoder for NvencEncoder {
    /// Phase 1 固定返回空列表：没有真实会话就不能宣称具备 H.264 编码能力。
    fn probe(&self, _codecs: &[CodecKind]) -> Vec<EncoderCaps> {
        Vec::new()
    }

    /// Phase 1 不创建会话；被调用说明选择链异常，必须显式失败而不是静默降级。
    fn configure(&mut self, _config: VideoConfig) -> Result<(), CodecError> {
        eprintln!(
            "[enc] nvenc.configure unsupported code={STATUS_SESSION_NOT_IMPLEMENTED} \
             reason=Phase 1 骨架未实装编码会话"
        );
        Err(CodecError::HardwareUnsupported)
    }

    /// Phase 1 无编码回路；直接拒绝提交。
    fn push(
        &mut self,
        _frame: PooledBuffer,
        _pts_ns: u64,
        _force_keyframe: bool,
    ) -> Result<(), CodecError> {
        Err(CodecError::HardwareUnsupported)
    }

    /// Phase 1 无输出。
    fn poll_packet(&mut self) -> Option<EncodedPacket> {
        None
    }

    /// Phase 1 不支持重配。
    fn reconfigure(&mut self, _config: VideoConfig) -> Result<(), CodecError> {
        Err(CodecError::HardwareUnsupported)
    }

    /// Phase 1 无编码回路，关键帧请求不产生效果，但必须留痕。
    fn request_keyframe(&mut self) {
        eprintln!("[enc] nvenc.keyframe unsupported code={STATUS_SESSION_NOT_IMPLEMENTED}");
    }

    /// 实际后端标识；只有 Phase 2 真启用后才允许出现 `nvenc-*`。
    fn backend_name(&self) -> &str {
        "nvenc-h264"
    }

    /// Phase 1 无滞留包。
    fn flush(&mut self) -> Result<Vec<EncodedPacket>, CodecError> {
        Ok(Vec::new())
    }
}

// =====** DLL 加载 =====**

/// 按优先级返回 shim 候选路径（均为本仓库产物，不含驱动 DLL）。
fn shim_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Ok(path) = std::env::var("RDC_NVENC_DLL") {
        candidates.push(PathBuf::from(path));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("rdc-nvenc.dll"));
        }
    }
    // 开发期：target/nvenc/rdc-nvenc.dll（build_nvenc.ps1 的默认输出）。
    candidates.push(PathBuf::from("target/nvenc/rdc-nvenc.dll"));
    candidates
}

/// 依次尝试候选路径加载 shim；全部失败返回 `None`。
fn load_shim() -> Option<HMODULE> {
    for path in shim_candidates() {
        if !path.exists() {
            continue;
        }
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        if let Ok(handle) = unsafe { LoadLibraryW(PCWSTR(wide.as_ptr())) } {
            return Some(handle);
        }
    }
    None
}

/// 把 shim 返回的原始状态映射为稳定状态码；未知值统一落到 `STATUS_UNKNOWN`。
fn status_code(raw: i32) -> u32 {
    match raw {
        0 => 0,
        1 => STATUS_SHIM_MISSING,
        2 => STATUS_DRIVER_DLL_MISSING,
        3 => STATUS_ENTRY_MISSING,
        4 => STATUS_VERSION_UNSUPPORTED,
        5 => STATUS_SESSION_NOT_IMPLEMENTED,
        _ => STATUS_UNKNOWN,
    }
}

/// 从 shim 导出表取符号。
unsafe fn symbol<T: Copy>(lib: HMODULE, name: &[u8]) -> Option<T> {
    let proc = GetProcAddress(lib, PCSTR(name.as_ptr()))?;
    Some(std::mem::transmute_copy(&proc))
}

/// 读取 shim 的最近错误文本；空指针时返回占位说明。
unsafe fn last_error_text(last_error: LastErrorFn) -> String {
    let ptr = last_error();
    if ptr.is_null() {
        return "shim 未提供错误文本".to_owned();
    }
    CStr::from_ptr(ptr).to_string_lossy().into_owned()
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 状态码必须唯一，且与 `shim/nvenc_shim.h` 的宏值一致。
    #[test]
    fn status_codes_are_stable_and_unique() {
        let codes = [
            STATUS_SHIM_MISSING,
            STATUS_DRIVER_DLL_MISSING,
            STATUS_ENTRY_MISSING,
            STATUS_VERSION_UNSUPPORTED,
            STATUS_SESSION_NOT_IMPLEMENTED,
            STATUS_UNKNOWN,
        ];
        assert_eq!(codes, [1, 2, 3, 4, 5, 99]);
    }

    /// 回退日志必须带 `code=` 与 `msg=`，不允许只写「不可用」。
    #[test]
    fn unavailable_note_contains_code_and_message() {
        let detail = NvencUnavailable {
            code: STATUS_DRIVER_DLL_MISSING,
            message: "LoadLibrary 失败".to_owned(),
        };
        let note = detail.note();
        assert!(note.contains("code=2"));
        assert!(note.contains("msg=LoadLibrary 失败"));
    }
}
