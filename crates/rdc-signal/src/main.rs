//! rdc-signal · 信令会合服务器（S1 空壳）
//!
//! 层级：L1 服务端（独立可部署进程）
//! 用途：注册、地址交换、配对码校验、限流、中继票据签发（docs/03）
//! 调用方：被控端与主控端经 UDP 21116 访问；运维经健康检查端口探活
//!
//! S1 目标：进程能启动、打印版本与监听计划，不实现任何协议处理。

use rdc_protocol::ports::PORT_SIGNAL;

// =====** 入口 =====**

/// 进程入口：打印启动信息后等待退出（S1 空壳）。
///
/// # Returns
/// 正常退出返回 `Ok(())`。
///
/// # Errors
/// S1 不执行任何 IO，不会失败。
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 版本与端口来自 protocol 单一真源，避免服务端与客户端端口漂移。
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        port = PORT_SIGNAL,
        "rdc-signal 启动（S1 空壳）"
    );
    println!("rdc-signal {} listening on udp/{PORT_SIGNAL} (S1 skeleton)", env!("CARGO_PKG_VERSION"));
    // TODO(stage2): 绑定 UDP 端口并处理 register/call/accept/candidates/relay_ticket/bye。
    Ok(())
}
