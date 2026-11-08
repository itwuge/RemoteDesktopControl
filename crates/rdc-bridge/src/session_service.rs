// ============================================================
// 文件用途：会话收尾 —— bye 握手 + 子服务按序释放 + 资源核查
//
// 层级：L4 桥接层（句柄核查仅 Windows）
// 被谁调用：api.rs 的 session_start / session_status / session_disconnect
// 边界：回环阶段的对端是进程内 UDP 回环桩（真实网络 bye 待接入会话层）；
//       本模块不实现业务，只负责「发 bye → 收回执 → 按序停服务 → 核查资源」
// ============================================================

use std::net::{SocketAddr, UdpSocket};
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use rdc_protocol::{SignalEnvelope, SignalMessage};

/// bye 往返等待上限：回环桩不超 1 ms，留余量给锁屏/调度抖动。
const BYE_TIMEOUT: Duration = Duration::from_millis(800);

/// 句柄数回落容差：UDP 套接字释放后允许的噪声（线程池/运行时缓存）。
const HANDLE_NOISE: i32 = 8;

// =====** 类型 =====**

/// 会话状态快照。
#[derive(Debug, Clone, Default)]
pub struct SessionStatusDto {
    /// 是否在会话中。
    pub running: bool,
    /// 会话 ID。
    pub session_id: String,
    /// 角色（controller / viewer）。
    pub role: String,
    /// 控制端回环端口。
    pub local_port: u16,
    /// 被控端回环端口。
    pub peer_port: u16,
    /// 已运行时长（毫秒）。
    pub elapsed_ms: u64,
    /// 是否已发出 bye。
    pub bye_sent: bool,
    /// 是否收到对端回执。
    pub bye_acked: bool,
}

/// 单个收尾步骤的结果。
#[derive(Debug, Clone, Default)]
pub struct TeardownStepDto {
    /// 步骤名（`input_stop` / `clipboard_stop` / `control_stop` / `wall_stop`）。
    pub name: String,
    /// 该步骤是否成功。
    pub ok: bool,
    /// 结果说明（失败原因或残留计数）。
    pub detail: String,
}

/// 收尾报告：bye 往返 + 各步骤 + 资源核查。
#[derive(Debug, Clone, Default)]
pub struct SessionTeardownDto {
    /// 会话 ID。
    pub session_id: String,
    /// 角色。
    pub role: String,
    /// bye 是否发出。
    pub bye_sent: bool,
    /// 对端是否解出 bye。
    pub bye_received: bool,
    /// 是否收到回执。
    pub bye_acked: bool,
    /// 回执往返（微秒）。
    pub ack_latency_us: u64,
    /// bye 报文字节数。
    pub bye_bytes: u32,
    /// 按序收尾步骤。
    pub steps: Vec<TeardownStepDto>,
    /// 收尾后仍在运行的服务数（必须为 0）。
    pub services_running_after: u32,
    /// 断开前进程句柄数。
    pub handles_before: u32,
    /// 收尾后进程句柄数。
    pub handles_after: u32,
    /// 句柄差值（after - before）。
    pub handles_delta: i32,
    /// 两个回环端口是否已释放。
    pub ports_released: bool,
    /// 整体是否通过。
    pub ok: bool,
    /// 最近错误。
    pub last_error: Option<String>,
}

// =====** 状态 =====**

/// 活动会话状态。
struct Session {
    /// 会话 ID。
    session_id: String,
    /// 角色。
    role: String,
    /// 控制端回环套接字。
    controller: UdpSocket,
    /// 被控端回环套接字（进程内桩）。
    peer: UdpSocket,
    /// 被控端地址。
    peer_addr: SocketAddr,
    /// 控制端地址。
    controller_addr: SocketAddr,
    /// 开始时刻。
    started: Instant,
    /// 已发 bye 次数。
    bye_sent: bool,
    /// 已收回执。
    bye_acked: bool,
    /// 断开前句柄数。
    handles_before: u32,
}

/// 活动会话槽。
fn slot() -> &'static Mutex<Option<Session>> {
    static SLOT: OnceLock<Mutex<Option<Session>>> = OnceLock::new();
    SLOT.get_or_init(|| Mutex::new(None))
}

/// 最近一次收尾报告（供重复断开时幂等返回）。
fn last_report() -> &'static Mutex<Option<SessionTeardownDto>> {
    static LAST: OnceLock<Mutex<Option<SessionTeardownDto>>> = OnceLock::new();
    LAST.get_or_init(|| Mutex::new(None))
}

// =====** 对外接口 =====**

/// 建立回环会话链路（幂等：重复调用返回同一会话）。
///
/// # Arguments
/// * `role` - 控制端角色名
///
/// # Returns
/// 会话状态快照（含两个回环端口）。
///
/// # Errors
/// 套接字绑定失败或锁异常时返回错误文本。
pub fn start(role: &str) -> Result<SessionStatusDto, String> {
    let mut guard = slot().lock().map_err(|error| error.to_string())?;
    if let Some(session) = guard.as_ref() {
        return Ok(status_of(session));
    }
    let controller = UdpSocket::bind("127.0.0.1:0").map_err(|error| error.to_string())?;
    let peer = UdpSocket::bind("127.0.0.1:0").map_err(|error| error.to_string())?;
    controller
        .set_read_timeout(Some(BYE_TIMEOUT))
        .map_err(|error| error.to_string())?;
    peer.set_read_timeout(Some(BYE_TIMEOUT))
        .map_err(|error| error.to_string())?;
    let peer_addr = peer.local_addr().map_err(|error| error.to_string())?;
    let controller_addr = controller.local_addr().map_err(|error| error.to_string())?;
    let session = Session {
        session_id: format!("loop-{:x}", now_us()),
        role: role.to_owned(),
        controller,
        peer,
        peer_addr,
        controller_addr,
        started: Instant::now(),
        bye_sent: false,
        bye_acked: false,
        handles_before: process_handles(),
    };
    let status = status_of(&session);
    *guard = Some(session);
    Ok(status)
}

/// 读取当前会话状态。
///
/// # Returns
/// 未在会话中时返回 `running=false` 的空快照。
///
/// # Errors
/// 锁异常时返回错误文本。
pub fn status() -> Result<SessionStatusDto, String> {
    let guard = slot().lock().map_err(|error| error.to_string())?;
    Ok(match guard.as_ref() {
        Some(session) => status_of(session),
        None => SessionStatusDto::default(),
    })
}

/// 断开会话：发 `bye` → 收对端回执 → 按序停服务 → 核查句柄与端口。
///
/// # Arguments
/// * `reason` - 结束原因（写入 bye 报文）
///
/// # Returns
/// 收尾报告；未在会话中时返回上一次报告（幂等）。
///
/// # Errors
/// 锁异常时返回错误文本；bye 失败不返回错误，记录在报告的 `ok` 与 `last_error`。
pub fn disconnect(reason: &str) -> Result<SessionTeardownDto, String> {
    let mut guard = slot().lock().map_err(|error| error.to_string())?;
    let Some(session) = guard.take() else {
        // 幂等：已断开过就直接给上次报告；从未建立会话则给空报告。
        let last = last_report().lock().map_err(|error| error.to_string())?;
        return Ok(last.clone().unwrap_or_default()); // gate-allow R-C-2: 从未建立会话时没有上次报告，空报告是该分支的正常取值
    };

    let mut report = SessionTeardownDto {
        session_id: session.session_id.clone(),
        role: session.role.clone(),
        handles_before: session.handles_before,
        ..SessionTeardownDto::default()
    };
    let mut errors: Vec<String> = Vec::new();

    // --- 1. bye 往返（真实 JSON 报文走回环 UDP）---
    match exchange_bye(&session, reason) {
        Ok((sent, received, acked, latency_us, bytes)) => {
            report.bye_sent = sent;
            report.bye_received = received;
            report.bye_acked = acked;
            report.ack_latency_us = latency_us;
            report.bye_bytes = bytes;
            if !acked {
                errors.push("对端未回 bye 回执".to_owned());
            }
        }
        Err(error) => {
            report.bye_sent = true;
            errors.push(error);
        }
    }

    // --- 2. 按序停服务（输入 → 剪贴板 → 画面 → 屏幕墙）---
    report.steps.push(stop_input_step());
    report.steps.push(stop_clipboard_step());
    report.steps.push(stop_control_step());
    report.steps.push(stop_wall_step());

    // --- 3. 收尾后仍在运行的服务数 ---
    report.services_running_after = running_services();

    // --- 4. 端口释放：先关套接字，再用同端口重新绑定验证 ---
    let ports = (session.controller_addr.port(), session.peer_addr.port());
    drop(session.controller);
    drop(session.peer);
    report.ports_released = UdpSocket::bind(("127.0.0.1", ports.0)).is_ok()
        && UdpSocket::bind(("127.0.0.1", ports.1)).is_ok();
    if !report.ports_released {
        errors.push(format!("端口未释放：{} / {}", ports.0, ports.1));
    }

    // --- 5. 句柄核查 ---
    report.handles_after = process_handles();
    report.handles_delta = report.handles_after as i32 - report.handles_before as i32;
    if report.handles_delta > HANDLE_NOISE {
        errors.push(format!("句柄残留 {} 个", report.handles_delta));
    }

    report.ok = report.bye_sent
        && report.bye_received
        && report.bye_acked
        && report.services_running_after == 0
        && report.ports_released
        && report.handles_delta <= HANDLE_NOISE;
    report.last_error = if errors.is_empty() {
        None
    } else {
        Some(errors.join("；"))
    };

    *last_report().lock().map_err(|error| error.to_string())? = Some(report.clone());
    Ok(report)
}

// =====** 内部实现 =====**

/// 一次 bye 往返：控制端发包 → 对端解包并回执 → 控制端验收。
///
/// # Returns
/// `(已发送, 对端已解出, 已收回执, 往返微秒, 报文字节数)`。
///
/// # Errors
/// 序列化、收发或解码失败时返回错误文本。
fn exchange_bye(
    session: &Session,
    reason: &str,
) -> Result<(bool, bool, bool, u64, u32), String> {
    let bye = SignalEnvelope::new(
        "loopback-controller",
        0,
        now_millis(),
        SignalMessage::Bye {
            session: session.session_id.clone(),
            reason: reason.to_owned(),
        },
    );
    let bytes = bye.encode().map_err(|error| error.to_string())?;
    let byte_len = bytes.len() as u32;
    session
        .controller
        .send_to(&bytes, session.peer_addr)
        .map_err(|error| format!("bye 发送失败：{error}"))?;
    let started = Instant::now();

    let mut buffer = [0u8; 4096];
    let (n, from) = session
        .peer
        .recv_from(&mut buffer)
        .map_err(|error| format!("对端未收到 bye：{error}"))?;
    let peer_envelope =
        SignalEnvelope::decode(&buffer[..n]).map_err(|error| format!("bye 解析失败：{error}"))?;
    let received = matches!(
        &peer_envelope.body,
        SignalMessage::Bye { session: id, .. } if id == &session.session_id
    );

    // 对端回执：同会话的 Bye，reason 标记为 ack。
    let ack = SignalEnvelope::new(
        "loopback-peer",
        1,
        now_millis(),
        SignalMessage::Bye {
            session: session.session_id.clone(),
            reason: "ack".to_owned(),
        },
    );
    let ack_bytes = ack.encode().map_err(|error| error.to_string())?;
    session
        .peer
        .send_to(&ack_bytes, from)
        .map_err(|error| format!("回执发送失败：{error}"))?;

    let (m, _) = session
        .controller
        .recv_from(&mut buffer)
        .map_err(|error| format!("bye 回执超时：{error}"))?;
    let ack_envelope =
        SignalEnvelope::decode(&buffer[..m]).map_err(|error| format!("回执解析失败：{error}"))?;
    let acked = matches!(
        &ack_envelope.body,
        SignalMessage::Bye { session: id, reason } if id == &session.session_id && reason == "ack"
    );
    Ok((
        true,
        received,
        acked,
        started.elapsed().as_micros() as u64,
        byte_len,
    ))
}

/// 停止输入通道并回报残留。
fn stop_input_step() -> TeardownStepDto {
    match crate::input_service::stop() {
        Ok(stats) => TeardownStepDto {
            name: "input_stop".to_owned(),
            ok: !stats.running,
            detail: format!(
                "注入 {} 次，抬起残留键 {} 个",
                stats.injected, stats.keys_down
            ),
        },
        Err(error) => TeardownStepDto {
            name: "input_stop".to_owned(),
            ok: false,
            detail: error,
        },
    }
}

/// 停止剪贴板同步并回报残留。
fn stop_clipboard_step() -> TeardownStepDto {
    match crate::clipboard_service::stop() {
        Ok(status) => TeardownStepDto {
            name: "clipboard_stop".to_owned(),
            ok: !status.running,
            detail: format!("外发 {} 条，写回 {} 条", status.sent, status.applied),
        },
        Err(error) => TeardownStepDto {
            name: "clipboard_stop".to_owned(),
            ok: false,
            detail: error,
        },
    }
}

/// 停止控制页画面管线并回报残留。
fn stop_control_step() -> TeardownStepDto {
    #[cfg(windows)]
    {
        match crate::control_service::stop() {
            Ok(snapshot) => TeardownStepDto {
                name: "control_stop".to_owned(),
                ok: !snapshot.running,
                detail: format!(
                    "采集 {} 帧 / 解码 {} 帧",
                    snapshot.captured_frames, snapshot.decoded_frames
                ),
            },
            Err(error) => TeardownStepDto {
                name: "control_stop".to_owned(),
                ok: false,
                detail: error,
            },
        }
    }
    #[cfg(target_os = "linux")]
    {
        match crate::control_service_linux::stop() {
            Ok(snapshot) => TeardownStepDto {
                name: "control_stop".to_owned(),
                ok: !snapshot.running,
                detail: format!(
                    "采集 {} 帧 / 解码 {} 帧",
                    snapshot.captured_frames, snapshot.decoded_frames
                ),
            },
            Err(error) => TeardownStepDto {
                name: "control_stop".to_owned(),
                ok: false,
                detail: error,
            },
        }
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        TeardownStepDto {
            name: "control_stop".to_owned(),
            ok: true,
            detail: "当前平台无画面管线".to_owned(),
        }
    }
}

/// 停止屏幕墙采集并回报残留。
fn stop_wall_step() -> TeardownStepDto {
    match crate::wall_service::stop() {
        Ok(snapshot) => TeardownStepDto {
            name: "wall_stop".to_owned(),
            ok: !snapshot.running,
            detail: format!("采集 {} 次", snapshot.encoded_frames),
        },
        Err(error) => TeardownStepDto {
            name: "wall_stop".to_owned(),
            ok: false,
            detail: error,
        },
    }
}

/// 统计当前仍在运行的子服务数量。
fn running_services() -> u32 {
    let mut running = 0u32;
    if crate::input_service::status().map(|s| s.running).unwrap_or(false) {
        running += 1;
    }
    if crate::clipboard_service::status()
        .map(|s| s.running)
        .unwrap_or(false)
    {
        running += 1;
    }
    if crate::wall_service::status()
        .map(|s| s.running)
        .unwrap_or(false)
    {
        running += 1;
    }
    #[cfg(windows)]
    if crate::control_service::status()
        .map(|s| s.running)
        .unwrap_or(false)
    {
        running += 1;
    }
    #[cfg(target_os = "linux")]
    if crate::control_service_linux::status()
        .map(|s| s.running)
        .unwrap_or(false)
    {
        running += 1;
    }
    running
}

/// 会话快照。
fn status_of(session: &Session) -> SessionStatusDto {
    SessionStatusDto {
        running: true,
        session_id: session.session_id.clone(),
        role: session.role.clone(),
        local_port: session.controller_addr.port(),
        peer_port: session.peer_addr.port(),
        elapsed_ms: session.started.elapsed().as_millis() as u64,
        bye_sent: session.bye_sent,
        bye_acked: session.bye_acked,
    }
}

/// 进程当前句柄数（Windows 才有真实值）。
#[cfg(windows)]
fn process_handles() -> u32 {
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessHandleCount};
    let mut count = 0u32;
    // SAFETY: 只读当前进程句柄计数，失败时 count 保持 0。
    unsafe {
        let _ = GetProcessHandleCount(GetCurrentProcess(), &mut count);
    }
    count
}

#[cfg(not(windows))]
fn process_handles() -> u32 {
    0
}

/// 单调微秒（会话 ID 用）。
fn now_us() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_micros())
        .unwrap_or(0)
}

/// 毫秒时间戳（bye 报文用）。
fn now_millis() -> i64 {
    (now_us() / 1000) as i64
}

// =====** 测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 完整收尾：bye 往返成功、服务全停、端口释放、重复断开幂等。
    #[test]
    fn disconnect_is_idempotent_and_clean() {
        let status = start("controller").expect("建立会话");
        assert!(status.running);
        assert!(status.local_port > 0 && status.peer_port > 0);

        let report = disconnect("单元测试").expect("断开");
        assert!(report.bye_sent && report.bye_received && report.bye_acked);
        assert!(report.bye_bytes > 0);
        assert!(report.steps.len() == 4);
        assert!(report.ports_released);

        // 重复断开：返回同一报告，不报错、不重复停服务。
        let again = disconnect("再次断开").expect("幂等断开");
        assert_eq!(again.session_id, report.session_id);
        assert!(again.bye_acked);
    }
}
