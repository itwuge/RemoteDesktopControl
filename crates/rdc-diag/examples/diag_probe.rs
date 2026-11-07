//! ============================================================
//! rdc-diag · 初始化的可验证样例
//!
//! 用途：证明 `RDC_LOG` 过滤生效：默认只看到 error/warn/info，
//!       `RDC_LOG=debug` 时额外看到 debug 行，`RDC_LOG=off` 时全部静默。
//! 用法：RDC_LOG=debug cargo run -p rdc-diag --example diag_probe
//! 说明：本文件属于 examples，允许直接输出，不属于运行期日志旁路。
//! ============================================================

// =====** 样例入口 =====**

fn main() {
    let guard = rdc_diag::init("diag-probe");
    tracing::error!(target: "rdc_capture", event = "probe.error", module = "capture", "error line (visible by default)");
    tracing::warn!(target: "rdc_capture", event = "probe.warn", module = "capture", "warn line (visible by default)");
    tracing::info!(target: "rdc_capture", event = "probe.info", module = "capture", "info line (visible by default)");
    tracing::debug!(target: "rdc_capture", event = "probe.debug", module = "capture", "debug line (only with RDC_LOG=debug)");
    tracing::trace!(target: "rdc_capture", event = "probe.trace", module = "capture", "trace line (only with RDC_LOG=trace)");
    guard.flush();
}
