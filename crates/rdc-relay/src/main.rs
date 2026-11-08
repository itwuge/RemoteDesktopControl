//! rdc-relay · 媒体中继服务器（S1 空壳）
//!
//! 层级：L1 服务端（独立可部署进程）
//! 用途：会话配对、转发**已加密**的媒体包、流量统计与配额（docs/01 C2/C4）
//! 调用方：仅在用户显式开启中继时由 rdc-connect 申请票据后使用；运维经健康检查端口探活
//!
//! 红线：本进程禁止持有任何会话密钥，禁止尝试解密媒体。

use rdc_protocol::ports::{PORT_RELAY_CONTROL, PORT_RELAY_MEDIA};

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
    // 服务端启动即装日志守卫：否则 tracing 无 subscriber，启动信息会静默丢失。
    let _diag = rdc_diag::init("rdc-relay");
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        media_port = PORT_RELAY_MEDIA,
        control_port = PORT_RELAY_CONTROL,
        "rdc-relay 启动（S1 空壳，默认关闭）"
    );
    // 启动信息已由上面的 tracing::info! 统一输出（同一事实不重复写 stdout）。
    // TODO(stage2): 实现会话配对与密文转发；只统计包数与字节数，不触碰载荷。
    Ok(())
}
