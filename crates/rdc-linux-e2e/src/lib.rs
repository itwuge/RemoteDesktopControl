//! rdc-linux-e2e · S8-2 Linux 端到端链路演示库
//!
//! 层级：演示 crate（不进入生产路径）
//! 用途：把采集（rdc-capture）、软件编码（rdc-codec）、解码（外部 ffmpeg）、
//!       X11 上屏与输入注入（rdc-input）串成可实测的两条链路：
//!       1) 单进程闭环 `s8_2_loop`：采集→编码→解码→上屏；
//!       2) 最小被控链路 `s8_2_host` / `s8_2_client`：推流 + 输入注入。
//!
//! 设计约束（见 `logs/TESTPLAN-s8-2.md`）：
//! - 仅 Linux/X11；不做 Wayland、不多显示器、不做硬件编码。
//! - 延迟用 `CLOCK_MONOTONIC` 纳秒时间戳跨进程做差（同机同源时钟）。
//! - 软编走外部 ffmpeg 进程，本 crate 不链接 GPL 组件。
//! - 所有判据以实测为准，不达标就如实打印，不调整阈值。

#[cfg(target_os = "linux")]
pub mod clock;
#[cfg(target_os = "linux")]
pub mod decoder;
#[cfg(target_os = "linux")]
pub mod display;
#[cfg(target_os = "linux")]
pub mod locate;
#[cfg(target_os = "linux")]
pub mod protocol;

// =====** 非 Linux 占位 =====**

/// 非 Linux 目标下的统一提示（bins 在非 Linux 上直接调用本函数退出）。
///
/// # Returns
/// 不返回，直接以退出码 2 结束进程。
///
/// # Errors
/// 不会失败。
#[cfg(not(target_os = "linux"))]
pub fn unsupported() -> ! {
    // 此处早于任何日志初始化，必须走 rdc-diag 的直写 fallback，避免 tracing 无 subscriber 时静默丢失。
    rdc_diag::record::write_fallback("ERROR", "linux-e2e", "rdc-linux-e2e 仅支持 Linux/X11");
    std::process::exit(2);
}
