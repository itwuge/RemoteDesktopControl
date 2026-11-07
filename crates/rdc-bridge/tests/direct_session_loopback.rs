// ============================================================
// 文件用途：直连会话（阶段 6）的宿主回环联调
//
// 层级：L4 桥接层 · 集成测试
// 被谁调用：`cargo test -p rdc-bridge --test direct_session_loopback`
//
// 为什么放在 tests/ 而不是 src/ 的 mod tests：
//   1) 这里只用公开出口 `rdc_bridge::api::direct_*`，与 Flutter 实际调用路径一致；
//   2) 需要真实 TcpListener/TcpStream 做对端，放在独立测试二进制里不污染库单测。
//
// 对端协议（S8-2 明文 TCP，非 docs/ 生产协议）：
//   `kind u8 | len u32 LE | payload[len]`，
//   kind 3=CONFIG(width,height,fps 各 u32 LE)、
//   kind 1=VIDEO(pts_ns u64 LE + flags u8 + Annex-B)、
//   kind 2=INPUT(rdc_input 单事件)、kind 4=BYE。
//
// 判据：logs/TESTPLAN-s9-android.md §2.13 的 D1–D5。
// 边界：只验证 Rust 会话层线格式与状态机；Android 解码上屏、真机触摸命中、
//       跨机时钟延迟不在本文件范围内（保持「未验证」）。
// ============================================================

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use rdc_bridge::api::{
    direct_connect, direct_disconnect, direct_send_input, direct_status, video_poll_frame,
    video_stats, InputEventDto, VideoFrameDto,
};

// =====** 串行闸门 =====**
//
// 直连会话是进程内单例（`direct_session::slot()`），而 Rust 测试默认多线程并行；
// 不串行会让 D3（3 s 连接超时）与 D4（8 s 等 CONFIG）互相抢同一个 slot。
// 用文件级静态 Mutex：本二进制内所有用例依次执行。
static SERIAL: Mutex<()> = Mutex::new(());

/// 获取串行闸门；前一个用例 panic 时拿到被污染的锁也要继续（否则后续全红）。
fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|poison| poison.into_inner())
}

// =====** 线格式辅助 =====**

/// 按 `kind u8 | len u32 LE | payload` 写一个协议包。
fn write_packet(stream: &mut TcpStream, kind: u8, payload: &[u8]) {
    let mut packet = Vec::with_capacity(5 + payload.len());
    packet.push(kind);
    packet.extend_from_slice(&(payload.len() as u32).to_le_bytes());
    packet.extend_from_slice(payload);
    stream.write_all(&packet).expect("写协议包失败");
}

/// 读一个完整协议包；超时/对端关闭直接 panic（测试里不接受静默丢包）。
fn read_packet(stream: &mut TcpStream) -> (u8, Vec<u8>) {
    let mut header = [0u8; 5];
    stream.read_exact(&mut header).expect("读包头失败");
    let kind = header[0];
    let len = u32::from_le_bytes([header[1], header[2], header[3], header[4]]) as usize;
    let mut payload = vec![0u8; len];
    if len > 0 {
        stream.read_exact(&mut payload).expect("读载荷失败");
    }
    (kind, payload)
}

/// CONFIG 载荷：`width u32 | height u32 | fps u32`，全部小端。
fn config_payload(width: u32, height: u32, fps: u32) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(12);
    for value in [width, height, fps] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

/// VIDEO 载荷：`pts_ns u64 LE | flags u8 | Annex-B`。
fn video_payload(pts_ns: u64, keyframe: bool, data: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(9 + data.len());
    bytes.extend_from_slice(&pts_ns.to_le_bytes());
    bytes.push(if keyframe { 0x01 } else { 0x00 });
    bytes.extend_from_slice(data);
    bytes
}

/// 一帧可辨识的 IDR 形状字节（`00 00 00 01 65` + 填充），用于比对端到端字节一致。
fn idr_frame() -> Vec<u8> {
    let mut bytes = vec![0x00, 0x00, 0x00, 0x01, 0x65];
    bytes.extend_from_slice(&[0xAA; 40]);
    bytes
}

/// 轮询直到条件成立或超时；返回是否成立（不 sleep 轮询，固定 20 ms 间隔）。
fn wait_until(mut condition: impl FnMut() -> bool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if condition() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

/// 从视频服务拉一帧（重组是异步的，允许短暂等待）。
fn poll_frame_within(timeout: Duration) -> Option<VideoFrameDto> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(frame) = video_poll_frame() {
            return Some(frame);
        }
        if Instant::now() >= deadline {
            return None;
        }
        thread::sleep(Duration::from_millis(20));
    }
}

// =====** D1：正常全链路 =====**

/// D1：连接 → CONFIG → VIDEO → 输入 → BYE 全链路。
#[test]
fn d1_full_loopback_connect_config_video_input_bye() {
    let _gate = serial();

    // 先断开可能残留的会话，避免上一用例被 panic 打断后留下 slot。
    direct_disconnect();

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind 失败");
    let addr = listener.local_addr().expect("local_addr 失败");
    let frame = idr_frame();
    // 闭包要 `'static` 且会移动捕获，这里留一份给主线程做逐字节比对。
    let frame_for_server = frame.clone();

    // 假服务端：完整走一遍对端职责，并在结束时回报收到的输入字节。
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept 失败");
        // 两次读取都设 5 s 上限：正常链路应在毫秒级完成，超时即视为失败。
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("设置读超时失败");

        // CONFIG 必须最先发：客户端 `connect` 在等它。
        write_packet(&mut stream, 3, &config_payload(1280, 800, 60));
        // 首帧必须是 IDR：重组器在拿到关键帧前会丢弃非关键帧。
        write_packet(
            &mut stream,
            1,
            &video_payload(1_000_000, true, &frame_for_server),
        );

        let (input_kind, input_payload) = read_packet(&mut stream);
        assert_eq!(input_kind, 2, "客户端应向服务端发送 kind=2 输入包");
        assert!(!input_payload.is_empty(), "输入包载荷不应为空");

        let (bye_kind, bye_payload) = read_packet(&mut stream);
        assert_eq!(bye_kind, 4, "断开时应发送 kind=4 BYE");
        assert!(bye_payload.is_empty(), "BYE 载荷应为空");
        input_payload.len()
    });

    // 客户端：连接并校验 CONFIG 立刻生效。
    let dto = direct_connect(addr.ip().to_string(), addr.port()).expect("直连建立失败");
    assert!(dto.connected, "连接后 connected 应为 true");
    assert!(dto.configured, "收到 CONFIG 后 configured 应为 true");
    assert_eq!(
        (dto.width, dto.height, dto.fps),
        (1280, 800, 60),
        "CONFIG 尺寸/帧率应与对端一致"
    );
    assert_eq!(dto.host, addr.ip().to_string(), "host 应原样回显");
    assert_eq!(dto.port, addr.port(), "port 应原样回显");

    // 帧计数与队列预算：单帧不应触发丢弃或重组错误。
    assert!(
        wait_until(
            || direct_status().frames_received >= 1,
            Duration::from_secs(3)
        ),
        "3 s 内应收到至少 1 帧；实际 {:?}",
        direct_status()
    );
    let status = direct_status();
    assert_eq!(status.frames_dropped, 0, "单帧不应被有界队列丢弃");
    assert_eq!(status.ingest_errors, 0, "合法整帧不应产生重组错误");

    // 重组结果：必须与发送的 Annex-B 逐字节一致，且关键帧标记保留。
    let frame_out = poll_frame_within(Duration::from_secs(3)).expect("3 s 内应重组出一帧");
    assert!(frame_out.keyframe, "首帧 keyframe 应为 true");
    assert_eq!(frame_out.data, frame, "重组帧字节应与发送一致");
    assert_eq!(frame_out.pts_us, 1_000, "pts_ns 应换算为微秒 1000");
    assert_eq!(
        video_stats().completed,
        1,
        "视频服务应记录 1 帧完成；实际 {:?}",
        video_stats()
    );

    // 输入上行：编码非空、计数加一。
    let sent = direct_send_input(vec![move_event(640, 400)]).expect("发送输入失败");
    assert_eq!(sent, 1, "1 条事件应发出 1 条");
    assert_eq!(direct_status().input_sent, 1, "input_sent 应为 1");
    assert_eq!(direct_status().input_errors, 0, "输入不应有失败计数");

    // 收尾：BYE 由客户端主动发出，断开后状态归零。
    direct_disconnect();
    let input_bytes = server.join().expect("假服务端线程 panic");
    assert!(
        input_bytes > 0,
        "服务端应读到非空输入载荷，实际 {input_bytes} 字节"
    );

    let after = direct_status();
    assert!(!after.connected, "断开后 connected 应为 false");
    assert_eq!(after.frames_received, 0, "断开后快照应为默认值");

    // 重复断开必须安全（幂等）。
    direct_disconnect();
}

/// 一条最小指针移动事件（坐标按 CONFIG 物理像素给）。
fn move_event(x: i32, y: i32) -> InputEventDto {
    InputEventDto {
        kind: "move".to_owned(),
        x,
        y,
        button: 0,
        pressed: false,
        delta_x: 0,
        delta_y: 0,
        scan_code: 0,
        modifiers: 0,
        delete_units: 0,
        text: String::new(),
    }
}

// =====** D2：连接被拒 =====**

/// D2：对已关闭的本地端口连接，必须返回 `E_CONNECT_REFUSED`。
#[test]
fn d2_connection_refused_maps_to_code() {
    let _gate = serial();
    direct_disconnect();

    // 先 bind 拿到一个系统分配的端口，再立刻释放：此时该端口无监听者。
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind 失败");
    let addr: SocketAddr = listener.local_addr().expect("local_addr 失败");
    drop(listener);

    let error = direct_connect(addr.ip().to_string(), addr.port()).expect_err("应连接失败");
    assert!(
        error.starts_with("E_CONNECT_REFUSED"),
        "应返回 E_CONNECT_REFUSED，实际：{error}"
    );
}

// =====** D3：连接超时 =====**

/// D3：对 RFC 5737 不可路由地址连接，必须返回 `E_CONNECT_TIMEOUT`。
///
/// 说明：这是唯一依赖宿主网络策略的用例。若本机立即返回「网络不可达」等其它错误码，
/// 按 §2.13 判定纪律记为该项未通过并如实回填，不改断言。
#[test]
fn d3_connect_timeout_maps_to_code() {
    let _gate = serial();
    direct_disconnect();

    let started = Instant::now();
    let error = direct_connect("192.0.2.1".to_owned(), 45900).expect_err("应连接失败");
    let elapsed = started.elapsed();

    assert!(
        error.starts_with("E_CONNECT_TIMEOUT"),
        "应返回 E_CONNECT_TIMEOUT，实际：{error}（耗时 {:?}）",
        elapsed
    );
    // 3 s 连接超时 + 允许解析/调度开销；上限放到 6 s 只防死等，不放宽错误码判据。
    assert!(
        elapsed >= Duration::from_secs(2) && elapsed <= Duration::from_secs(6),
        "耗时应接近 3 s 连接超时，实际 {:?}",
        elapsed
    );
}

// =====** D4：未收到 CONFIG =====**

/// D4：TCP 已连上但服务端沉默，必须返回 `E_CONFIG_TIMEOUT`。
#[test]
fn d4_missing_config_maps_to_code() {
    let _gate = serial();
    direct_disconnect();

    // 只监听不 accept：SYN 握手会完成（进 backlog），但不会发出任何协议包，
    // 正好模拟「对端在忙 / 采集未启动」。
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind 失败");
    let addr = listener.local_addr().expect("local_addr 失败");

    let started = Instant::now();
    let error = direct_connect(addr.ip().to_string(), addr.port()).expect_err("应连接失败");
    let elapsed = started.elapsed();

    assert!(
        error.starts_with("E_CONFIG_TIMEOUT"),
        "应返回 E_CONFIG_TIMEOUT，实际：{error}（耗时 {:?}）",
        elapsed
    );
    // 判定窗口 7.5–10 s：实现是 8 s 截止，留 0.5 s 时钟/调度余量。
    assert!(
        elapsed >= Duration::from_millis(7500) && elapsed <= Duration::from_secs(10),
        "应在 8 s 窗口后超时，实际 {:?}",
        elapsed
    );

    drop(listener);
}

// =====** D5：参数校验 =====**

/// D5：空 host 与 port=0 必须在发起连接前返回 `E_ARGS`。
#[test]
fn d5_argument_validation_maps_to_code() {
    let _gate = serial();
    direct_disconnect();

    let empty_host = direct_connect(String::new(), 45900).expect_err("空 host 应被拒绝");
    assert!(
        empty_host.starts_with("E_ARGS"),
        "空 host 应返回 E_ARGS，实际：{empty_host}"
    );

    let zero_port = direct_connect("127.0.0.1".to_owned(), 0).expect_err("端口 0 应被拒绝");
    assert!(
        zero_port.starts_with("E_ARGS"),
        "端口 0 应返回 E_ARGS，实际：{zero_port}"
    );

    // 空白字符 host 也应被 trim 后判为空。
    let blank_host = direct_connect("   ".to_owned(), 45900).expect_err("空白 host 应被拒绝");
    assert!(
        blank_host.starts_with("E_ARGS"),
        "空白 host 应返回 E_ARGS，实际：{blank_host}"
    );
}
