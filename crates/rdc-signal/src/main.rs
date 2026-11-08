//! rdc-signal · 信令会合服务器二进制入口
//!
//! 层级：L1 服务端（独立可部署进程）
//! 用途：绑定 UDP 21116 并运行 TLS 1.3 over QUIC 信令服务，直到收到 Ctrl+C。
//! 调用方：运维直接启动；rdc-cli demo 使用 lib 版 `SignalServer` 同进程启动。
//!
//! 安全前提：自签证书仅用于本地/内网部署；生产必须由运维注入正式证书。

use std::net::SocketAddr;

use rdc_protocol::ports::PORT_SIGNAL;
use rdc_signal::SignalServer;

// =====** 入口 =====**

/// 进程入口：启动信令服务并等待退出信号。
///
/// # Returns
/// 正常关闭返回 `Ok(())`。
///
/// # Errors
/// 端口绑定或 TLS 初始化失败时返回错误。
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 服务端启动即装日志守卫：否则 tracing 无 subscriber，启动信息会静默丢失。
    let _diag = rdc_diag::init("rdc-signal");
    // 默认监听所有 IPv4 网卡；若需 IPv6 可改为 `[::]:PORT_SIGNAL`。
    let addr = SocketAddr::from(([0, 0, 0, 0], PORT_SIGNAL));
    let server = SignalServer::bind(addr).await?;
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        addr = %server.addr(),
        "rdc-signal 启动"
    );
    // 启动信息已由上面的 tracing::info! 统一输出（同一事实不重复写 stdout）。

    // run() 只在端点关闭时返回；进程收到 Ctrl+C 时由操作系统直接结束。
    server.run().await;
    Ok(())
}
