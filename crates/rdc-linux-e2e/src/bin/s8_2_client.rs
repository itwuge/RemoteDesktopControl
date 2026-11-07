//! s8_2_client · 第二进程控制端：接收画面→解码→上屏，自动定位绿块并点击
//!
//! 运行：`DISPLAY=:0 cargo run -p rdc-linux-e2e --bin s8_2_client -- --connect 127.0.0.1:45900 --secs 60 --clicks 10`
//!
//! 判据取证：
//! - 端到端延迟：`CLOCK_MONOTONIC` 纳秒差（host 采集时刻 → client 解码完成），输出 p50/p99；
//! - 点击命中：在解码帧里取最大绿色连通块的重心作为点击点，host 目标窗口回读
//!   `x_root/y_root`，偏差由外部脚本对比；
//! - 稳定性：连续运行时长与解码/上屏帧数。
//!
//! 说明：控制端上屏窗口按 `--scale` 缩小放在屏幕右下角，避免遮住目标控件；
//! 因此画面里会多出一个缩小的绿块镜像，定位逻辑按「面积最大」过滤。

#[cfg(not(target_os = "linux"))]
fn main() {
    rdc_linux_e2e::unsupported();
}

#[cfg(target_os = "linux")]
fn main() {
    std::process::exit(linux_main());
}

#[cfg(target_os = "linux")]
mod linux_impl {
    /// 参数。
    pub struct Args {
        pub connect: String,
        pub secs: u64,
        pub clicks: u32,
        pub click_interval_ms: u64,
        pub move_interval_ms: u64,
        pub scale: u32,
        pub win_x: i16,
        pub win_y: i16,
    }

    /// 解析命令行。
    pub fn parse_args() -> Result<Args, String> {
        let mut args = Args {
            connect: "127.0.0.1:45900".into(),
            secs: 60,
            clicks: 10,
            click_interval_ms: 2000,
            move_interval_ms: 500,
            scale: 2,
            win_x: 640,
            win_y: 400,
        };
        let mut iter = std::env::args().skip(1);
        while let Some(arg) = iter.next() {
            let value = iter.next();
            match arg.as_str() {
                "--connect" => args.connect = value.ok_or("--connect 缺值")?,
                "--secs" => args.secs = value.ok_or("--secs 缺值")?.parse().map_err(|_| "--secs 需整数")?,
                "--clicks" => args.clicks = value.ok_or("--clicks 缺值")?.parse().map_err(|_| "--clicks 需整数")?,
                "--click-interval-ms" => {
                    args.click_interval_ms = value
                        .ok_or("--click-interval-ms 缺值")?
                        .parse()
                        .map_err(|_| "--click-interval-ms 需整数")?
                }
                "--move-interval-ms" => {
                    args.move_interval_ms = value
                        .ok_or("--move-interval-ms 缺值")?
                        .parse()
                        .map_err(|_| "--move-interval-ms 需整数")?
                }
                "--scale" => args.scale = value.ok_or("--scale 缺值")?.parse().map_err(|_| "--scale 需整数")?,
                "--win-x" => args.win_x = value.ok_or("--win-x 缺值")?.parse().map_err(|_| "--win-x 需整数")?,
                "--win-y" => args.win_y = value.ok_or("--win-y 缺值")?.parse().map_err(|_| "--win-y 需整数")?,
                other => return Err(format!("未知参数: {other}")),
            }
        }
        Ok(args)
    }
}

#[cfg(target_os = "linux")]
use std::io::Write;
#[cfg(target_os = "linux")]
use std::net::TcpStream;
#[cfg(target_os = "linux")]
use std::sync::mpsc::{self, Receiver};
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use rdc_input::{encode_event, InputEvent, PointerButton, PointerEvent};
#[cfg(target_os = "linux")]
use rdc_linux_e2e::locate::largest_green;
#[cfg(target_os = "linux")]
use rdc_linux_e2e::{clock, decoder::Decoder, display::Display, protocol};

#[cfg(target_os = "linux")]
fn linux_main() -> i32 {
    use linux_impl::*;
    let args = match parse_args() {
        Ok(args) => args,
        Err(error) => {
            eprintln!("参数错误: {error}");
            return 2;
        }
    };
    match run(&args) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("运行失败: {error}");
            2
        }
    }
}

/// 计算分位数。
#[cfg(target_os = "linux")]
fn percentile(values: &mut [u64], pct: f64) -> u64 {
    if values.is_empty() {
        return 0;
    }
    values.sort_unstable();
    let idx = ((values.len() as f64 - 1.0) * pct).round() as usize;
    values[idx.min(values.len() - 1)]
}

/// 发送一次「移动+按下+抬起」。
///
/// # Arguments
/// * `writer` - 控制端连接写端。
/// * `x` / `y` - 远端桌面坐标。
///
/// # Returns
/// 三个包全部写入成功返回 `Ok(())`。
///
/// # Errors
/// 网络写失败返回说明文本。
#[cfg(target_os = "linux")]
fn send_click(writer: &mut TcpStream, x: i32, y: i32) -> Result<(), String> {
    let events = [
        PointerEvent {
            x,
            y,
            button: None,
            pressed: false,
        },
        PointerEvent {
            x,
            y,
            button: Some(PointerButton::Left),
            pressed: true,
        },
        PointerEvent {
            x,
            y,
            button: Some(PointerButton::Left),
            pressed: false,
        },
    ];
    for event in events {
        let payload = encode_event(&InputEvent::Pointer(event));
        protocol::Packet {
            kind: protocol::KIND_INPUT,
            payload,
        }
        .write_to(writer)
        .map_err(|error| format!("发送输入失败: {error}"))?;
    }
    writer.flush().map_err(|error| format!("刷新 socket 失败: {error}"))
}

/// 发送一条指针移动（用于 E7「注入移动 ≥100 次」的持续取证）。
///
/// # Arguments
/// * `writer` - 控制端连接写端。
/// * `x` / `y` - 远端桌面坐标。
///
/// # Returns
/// 写入成功返回 `Ok(())`。
///
/// # Errors
/// 网络写失败返回说明文本。
#[cfg(target_os = "linux")]
fn send_move(writer: &mut TcpStream, x: i32, y: i32) -> Result<(), String> {
    let payload = encode_event(&InputEvent::Pointer(PointerEvent {
        x,
        y,
        button: None,
        pressed: false,
    }));
    protocol::Packet {
        kind: protocol::KIND_INPUT,
        payload,
    }
    .write_to(writer)
    .map_err(|error| format!("发送移动失败: {error}"))?;
    writer.flush().map_err(|error| format!("刷新 socket 失败: {error}"))
}

/// 读取线程：持续读包进通道。
#[cfg(target_os = "linux")]
fn spawn_reader(mut stream: TcpStream) -> Result<Receiver<protocol::Packet>, String> {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("rdc-net-in".into())
        .spawn(move || loop {
            match protocol::read_packet(&mut stream) {
                Ok(Some(packet)) => {
                    if tx.send(packet).is_err() {
                        break;
                    }
                }
                Ok(None) | Err(_) => break,
            }
        })
        .map_err(|error| format!("读线程创建失败: {error}"))?;
    Ok(rx)
}

#[cfg(target_os = "linux")]
fn run(args: &linux_impl::Args) -> Result<(), String> {
    // 1) 连接被控端。
    let stream = TcpStream::connect(&args.connect).map_err(|error| format!("连接 {} 失败: {error}", args.connect))?;
    stream.set_nodelay(true).ok();
    let mut writer = stream.try_clone().map_err(|error| format!("克隆 socket 失败: {error}"))?;
    let rx = spawn_reader(stream)?;
    println!("CLIENT_CONNECT addr={} pid={}", args.connect, std::process::id());

    // 2) 等 CONFIG。
    let config = loop {
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(packet) if packet.kind == protocol::KIND_CONFIG => {
                match protocol::StreamConfig::from_payload(&packet.payload) {
                    Some(config) => break config,
                    None => return Err("CONFIG 载荷非法".into()),
                }
            }
            Ok(_) => continue,
            Err(_) => return Err("等待 CONFIG 超时".into()),
        }
    };
    println!("CLIENT_CONFIG width={} height={} fps={}", config.width, config.height, config.fps);

    // 3) 上屏窗口（按 scale 缩小）。
    let scale = args.scale.max(1);
    let dw = (config.width / scale).max(1);
    let dh = (config.height / scale).max(1);
    let mut display = Display::create(dw, dh, args.win_x, args.win_y, "rdc-s8-2-client")?;
    println!("CLIENT_WINDOW {}x{} at ({},{})", dw, dh, args.win_x, args.win_y);

    // 4) 解码器。
    let mut decoder = Decoder::spawn(config.width, config.height)?;
    let mut scaled = vec![0u8; dw as usize * dh as usize * 4];
    let mut packets = 0u64;
    let mut frames = 0u64;
    let mut latencies: Vec<u64> = Vec::new();
    let mut clicks_done = 0u32;
    let mut click_points: Vec<(i32, i32)> = Vec::new();
    let mut last_click: Option<Instant> = None;
    let mut moves_done = 0u64;
    let mut last_move: Option<Instant> = None;
    let mut decode_errors = 0u64;
    let mut last_pts = 0u64;
    let mut pts_regressions = 0u64;

    // 5) 主循环。
    let start = Instant::now();
    let mut next_report = 1u64;
    while start.elapsed().as_secs() < args.secs {
        if !display.pump() {
            break;
        }
        // 持续移动（不参与命中判据，只用于证明注入通道长时间可用）。
        if last_move.map_or(true, |t| t.elapsed() >= Duration::from_millis(args.move_interval_ms)) {
            let phase = (moves_done % 4) as i32;
            let (mx, my) = [(40, 40), (80, 40), (80, 80), (40, 80)][phase as usize];
            send_move(&mut writer, mx, my)?;
            moves_done += 1;
            last_move = Some(Instant::now());
        }
        loop {
            let packet = match rx.try_recv() {
                Ok(packet) => packet,
                Err(_) => break,
            };
            match packet.kind {
                protocol::KIND_VIDEO => {
                    if let Some((pts, _keyframe, data)) = protocol::parse_video(&packet.payload) {
                        packets += 1;
                        if decoder.push(data, pts).is_err() {
                            decode_errors += 1;
                        }
                    }
                }
                protocol::KIND_BYE => return finish(args, &mut display, &mut decoder, packets, frames, &mut latencies, &click_points, decode_errors, moves_done, pts_regressions),
                _ => {}
            }
        }
        while let Some(decoded) = decoder.poll() {
            let pts = decoded.pts_ns;
            // E4：解码输出的采集时间戳必须单调递增（配对队列未漂移）。
            if pts != 0 {
                if pts < last_pts {
                    pts_regressions += 1;
                }
                last_pts = pts;
            }
            let now = clock::mono_ns();
            if pts > 0 && now > pts {
                latencies.push(now - pts);
            }
            frames += 1;
            let frame = decoded.data.as_slice();
            // 定位只在「还有点击任务且间隔已到」时做：整帧 BFS 很贵，不能每帧都扫。
            let click_due = clicks_done < args.clicks
                && last_click
                    .map_or(true, |t| t.elapsed() >= Duration::from_millis(args.click_interval_ms));
            if click_due {
                if let Some(blob) = largest_green(frame, config.width, config.height) {
                    send_click(&mut writer, blob.cx as i32, blob.cy as i32)?;
                    clicks_done += 1;
                    click_points.push((blob.cx as i32, blob.cy as i32));
                    last_click = Some(Instant::now());
                    println!(
                        "CLICK idx={} x={} y={} area={} bbox=({},{}..{},{})",
                        clicks_done, blob.cx, blob.cy, blob.area, blob.min_x, blob.min_y, blob.max_x, blob.max_y
                    );
                }
            }
            rdc_linux_e2e::display::scale_nearest(
                frame,
                config.width,
                config.height,
                &mut scaled,
                dw,
                dh,
            );
            display.present(&scaled)?;
        }
        let elapsed = start.elapsed().as_secs();
        if elapsed >= next_report {
            next_report = elapsed + 1;
            println!(
                "CLIENT_SECOND t={elapsed}s packets={} frames={} displayed={} clicks={} moves={}",
                packets,
                frames,
                display.presented(),
                clicks_done,
                moves_done
            );
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    // 通知被控端断开。
    let _ = protocol::Packet {
        kind: protocol::KIND_BYE,
        payload: Vec::new(),
    }
    .write_to(&mut writer);
    let _ = writer.flush();
    finish(args, &mut display, &mut decoder, packets, frames, &mut latencies, &click_points, decode_errors, moves_done, pts_regressions)
}

/// 收尾输出。
#[cfg(target_os = "linux")]
#[allow(clippy::too_many_arguments)]
fn finish(
    args: &linux_impl::Args,
    display: &mut Display,
    decoder: &mut Decoder,
    packets: u64,
    frames: u64,
    latencies: &mut Vec<u64>,
    click_points: &[(i32, i32)],
    decode_errors: u64,
    moves_done: u64,
    pts_regressions: u64,
) -> Result<(), String> {
    decoder.finish();
    while let Some(frame) = decoder.poll() {
        let _ = frame;
    }
    // latencies 以纳秒记录（CLOCK_MONOTONIC），JSON 里统一换算成微秒。
    let p50 = percentile(latencies, 0.50) / 1000;
    let p99 = percentile(latencies, 0.99) / 1000;
    let max = latencies.iter().copied().max().unwrap_or(0) / 1000;
    let points: Vec<String> = click_points
        .iter()
        .map(|(x, y)| format!("[{x},{y}]"))
        .collect();
    println!(
        "CLIENT_JSON {{\"packets\":{},\"decoded\":{},\"decoder_dropped\":{},\"displayed\":{},\"decode_errors\":{},\"pts_regressions\":{},\"lat_p50_us\":{},\"lat_p99_us\":{},\"lat_max_us\":{},\"lat_samples\":{},\"clicks\":{},\"moves\":{},\"click_points\":[{}]}}",
        packets,
        decoder.frames().max(frames),
        decoder.dropped(),
        display.presented(),
        decode_errors,
        pts_regressions,
        p50,
        p99,
        max,
        latencies.len(),
        click_points.len(),
        moves_done,
        points.join(",")
    );
    println!("CLIENT_DONE clicks={}", args.clicks.min(click_points.len() as u32));
    Ok(())
}
