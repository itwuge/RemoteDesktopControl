//! 用途：s8_2_host · 最小被控服务：采集→软编→TCP 推流 + 接收输入并注入
//!
//! 运行：`DISPLAY=:0 cargo run -p rdc-linux-e2e --bin s8_2_host -- --listen 127.0.0.1:45900 --secs 60`
//!
//! 行为：
//! 1) 启动采集与软件编码；
//! 2) 接受一个控制端连接，先发 `CONFIG`，再持续推 `VIDEO`；
//! 3) 独立线程读 `INPUT` 包并用 rdc-input 注入到本机 X11。
//!
//! 输出：每秒 `HOST_SECOND`，结束 `HOST_JSON`。

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
        pub listen: String,
        pub secs: u64,
        pub fps: u32,
        pub bitrate_bps: u32,
        pub display_id: u32,
        pub max_backlog: u64,
    }

    /// 解析命令行。
    pub fn parse_args() -> Result<Args, String> {
        let mut args = Args {
            listen: "127.0.0.1:45900".into(),
            secs: 60,
            fps: 60,
            bitrate_bps: 8_000_000,
            display_id: 0,
            max_backlog: 3,
        };
        let mut iter = std::env::args().skip(1);
        while let Some(arg) = iter.next() {
            let value = iter.next();
            match arg.as_str() {
                "--listen" => args.listen = value.ok_or("--listen 缺值")?,
                "--secs" => args.secs = value.ok_or("--secs 缺值")?.parse().map_err(|_| "--secs 需整数")?,
                "--fps" => args.fps = value.ok_or("--fps 缺值")?.parse().map_err(|_| "--fps 需整数")?,
                "--bitrate" => {
                    args.bitrate_bps = value.ok_or("--bitrate 缺值")?.parse().map_err(|_| "--bitrate 需整数")?
                }
                "--display-id" => {
                    args.display_id = value.ok_or("--display-id 缺值")?.parse().map_err(|_| "--display-id 需整数")?
                }
                "--max-backlog" => {
                    args.max_backlog = value.ok_or("--max-backlog 缺值")?.parse().map_err(|_| "--max-backlog 需整数")?
                }
                other => return Err(format!("未知参数: {other}")),
            }
        }
        Ok(args)
    }
}

#[cfg(target_os = "linux")]
use std::io::Write;
#[cfg(target_os = "linux")]
use std::net::{TcpListener, TcpStream};
#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
#[cfg(target_os = "linux")]
use std::sync::{Arc, Mutex};
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use rdc_capture::{create_source, CaptureConfig, ChromaFormat, Frame, FrameSink};
#[cfg(target_os = "linux")]
use rdc_codec::{create_encoder, CodecKind, VideoChroma, VideoConfig, VideoEncoder};
#[cfg(target_os = "linux")]
use rdc_input::{create_injector, decode_event, InputInjector, Viewport};
#[cfg(target_os = "linux")]
use rdc_linux_e2e::{clock, protocol};

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

#[cfg(target_os = "linux")]
struct HostSink {
    encoder: Arc<Mutex<Box<dyn VideoEncoder>>>,
    captured: Arc<AtomicU64>,
    pushed: Arc<AtomicU64>,
    produced: Arc<AtomicU64>,
    errors: Arc<AtomicU64>,
    backpressure: Arc<AtomicU64>,
    max_backlog: u64,
}

#[cfg(target_os = "linux")]
impl FrameSink for HostSink {
    fn on_frame(&self, frame: rdc_capture::Frame) {
        self.captured.fetch_add(1, Ordering::Relaxed);
        let pushed = self.pushed.load(Ordering::Relaxed);
        let produced = self.produced.load(Ordering::Relaxed);
        if pushed.saturating_sub(produced) >= self.max_backlog {
            self.backpressure.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let pts = clock::mono_ns();
        let Frame { buffer, .. } = frame;
        let Ok(mut encoder) = self.encoder.lock() else {
            self.errors.fetch_add(1, Ordering::Relaxed);
            return;
        };
        match encoder.push(buffer, pts, false) {
            Ok(()) => {
                self.pushed.fetch_add(1, Ordering::Relaxed);
            }
            Err(_) => {
                self.errors.fetch_add(1, Ordering::Relaxed);
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn run(args: &linux_impl::Args) -> Result<(), String> {
    // 1) 采集源与尺寸。
    let mut source = create_source().map_err(|error| format!("创建采集源失败: {error}"))?;
    let displays = source.enumerate();
    let display = displays
        .iter()
        .find(|d| d.id == args.display_id)
        .or_else(|| displays.first())
        .ok_or("未枚举到显示器")?;
    let (width, height) = (display.width, display.height);
    let display_id = display.id;
    println!(
        "DISPLAY id={} name={} {}x{}@{}",
        display.id, display.name, display.width, display.height, display.refresh_hz
    );

    // 2) 编码器。
    let mut encoder =
        create_encoder(CodecKind::H264).map_err(|error| format!("创建编码器失败: {error}"))?;
    encoder
        .configure(VideoConfig {
            codec: CodecKind::H264,
            width,
            height,
            fps: args.fps,
            bitrate_bps: args.bitrate_bps,
            chroma: VideoChroma::Yuv420,
            bit_depth: 8,
            low_latency: true,
        })
        .map_err(|error| format!("配置编码器失败: {error}"))?;
    println!("ENCODER backend={}", encoder.backend_name());
    let encoder = Arc::new(Mutex::new(encoder));

    // 3) sink 与采集启动。
    let captured = Arc::new(AtomicU64::new(0));
    let pushed = Arc::new(AtomicU64::new(0));
    let produced = Arc::new(AtomicU64::new(0));
    let errors = Arc::new(AtomicU64::new(0));
    let backpressure = Arc::new(AtomicU64::new(0));
    let sink = Arc::new(HostSink {
        encoder: Arc::clone(&encoder),
        captured: Arc::clone(&captured),
        pushed: Arc::clone(&pushed),
        produced: Arc::clone(&produced),
        errors: Arc::clone(&errors),
        backpressure: Arc::clone(&backpressure),
        max_backlog: args.max_backlog,
    });
    source.set_sink(Arc::clone(&sink) as Arc<dyn FrameSink>);
    source
        .start(CaptureConfig {
            display_id,
            target_fps: args.fps,
            include_cursor: false,
            chroma: ChromaFormat::Bgra,
            bit_depth: 8,
        })
        .map_err(|error| format!("启动采集失败: {error}"))?;
    println!("CAPTURE backend={}", source.backend_name());

    // 4) 监听控制端。
    let listener = TcpListener::bind(&args.listen).map_err(|error| format!("监听 {} 失败: {error}", args.listen))?;
    println!("HOST_LISTEN addr={} pid={}", args.listen, std::process::id());
    let (stream, peer) = listener.accept().map_err(|error| format!("accept 失败: {error}"))?;
    stream.set_nodelay(true).ok();
    println!("HOST_ACCEPT peer={peer}");
    let mut writer = stream.try_clone().map_err(|error| format!("克隆 socket 失败: {error}"))?;
    protocol::Packet {
        kind: protocol::KIND_CONFIG,
        payload: protocol::StreamConfig {
            width,
            height,
            fps: args.fps,
        }
        .to_payload(),
    }
    .write_to(&mut writer)
    .map_err(|error| format!("发送 CONFIG 失败: {error}"))?;

    // 5) 输入注入线程。
    let alive = Arc::new(AtomicBool::new(true));
    let injected = Arc::new(AtomicU64::new(0));
    let input_errors = Arc::new(AtomicU64::new(0));
    let reader_stream = stream.try_clone().map_err(|error| format!("克隆读端失败: {error}"))?;
    let inject_alive = Arc::clone(&alive);
    let inject_count = Arc::clone(&injected);
    let inject_errors = Arc::clone(&input_errors);
    let viewport = Viewport {
        x: 0,
        y: 0,
        width,
        height,
        scale: 1.0,
    };
    std::thread::Builder::new()
        .name("rdc-inject".into())
        .spawn(move || injection_loop(reader_stream, viewport, inject_alive, inject_count, inject_errors))
        .map_err(|error| format!("输入线程创建失败: {error}"))?;

    // 6) 主循环：推流。
    let start = Instant::now();
    let mut next_report = 1u64;
    let mut packets = 0u64;
    let mut bytes_total = 0u64;
    let mut write_errors = 0u64;
    while alive.load(Ordering::Relaxed) && start.elapsed().as_secs() < args.secs {
        loop {
            let packet = match encoder.lock() {
                Ok(mut guard) => guard.poll_packet(),
                Err(_) => None,
            };
            let Some(packet) = packet else {
                break;
            };
            produced.fetch_add(1, Ordering::Relaxed);
            bytes_total += packet.data.len() as u64;
            packets += 1;
            let payload = protocol::video_payload(packet.pts_ns, packet.is_keyframe, &packet.data);
            let out = protocol::Packet {
                kind: protocol::KIND_VIDEO,
                payload,
            };
            if out.write_to(&mut writer).is_err() {
                write_errors += 1;
                alive.store(false, Ordering::Relaxed);
                break;
            }
        }
        let elapsed = start.elapsed().as_secs();
        if elapsed >= next_report {
            next_report = elapsed + 1;
            println!(
                "HOST_SECOND t={elapsed}s captured={} packets={} bytes={} injected={}",
                captured.load(Ordering::Relaxed),
                packets,
                bytes_total,
                injected.load(Ordering::Relaxed)
            );
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    source.stop();
    let _ = writer.flush();
    let rest = {
        let mut guard = encoder.lock().map_err(|_| "编码器锁被污染")?;
        guard
            .flush()
            .map_err(|error| format!("冲刷编码器失败: {error}"))?
    };
    for packet in rest {
        produced.fetch_add(1, Ordering::Relaxed);
        bytes_total += packet.data.len() as u64;
        packets += 1;
        let payload = protocol::video_payload(packet.pts_ns, packet.is_keyframe, &packet.data);
        let _ = protocol::Packet {
            kind: protocol::KIND_VIDEO,
            payload,
        }
        .write_to(&mut writer);
    }
    let _ = protocol::Packet {
        kind: protocol::KIND_BYE,
        payload: Vec::new(),
    }
    .write_to(&mut writer);
    let _ = writer.flush();

    let secs = start.elapsed().as_secs_f64().max(0.001);
    let backend = {
        let guard = encoder.lock().map_err(|_| "编码器锁被污染")?;
        guard.backend_name().to_string()
    };
    println!(
        "HOST_JSON {{\"width\":{},\"height\":{},\"secs\":{:.3},\"captured\":{},\"capture_fps\":{:.2},\"pushed\":{},\"packets\":{},\"packet_fps\":{:.2},\"bytes\":{},\"bitrate_bps\":{},\"backpressure_dropped\":{},\"push_errors\":{},\"write_errors\":{},\"injected\":{},\"input_errors\":{},\"backend\":\"{}\"}}",
        width,
        height,
        secs,
        captured.load(Ordering::Relaxed),
        captured.load(Ordering::Relaxed) as f64 / secs,
        pushed.load(Ordering::Relaxed),
        packets,
        packets as f64 / secs,
        bytes_total,
        (bytes_total as f64 * 8.0 / secs) as u64,
        backpressure.load(Ordering::Relaxed),
        errors.load(Ordering::Relaxed),
        write_errors,
        injected.load(Ordering::Relaxed),
        input_errors.load(Ordering::Relaxed),
        backend
    );
    Ok(())
}

/// 输入线程：读 INPUT 包并注入。
///
/// # Arguments
/// * `stream` - 控制端连接读端。
/// * `viewport` - 被控桌面矩形（全屏）。
/// * `alive` - 连接存活标志（读到 EOF 置 false）。
/// * `injected` - 注入成功计数。
/// * `errors` - 注入失败计数。
///
/// # Returns
/// 无返回值；线程随连接结束退出。
///
/// # Errors
/// 不会失败；注入失败只计数。
#[cfg(target_os = "linux")]
fn injection_loop(
    mut stream: TcpStream,
    viewport: Viewport,
    alive: Arc<AtomicBool>,
    injected: Arc<AtomicU64>,
    errors: Arc<AtomicU64>,
) {
    let mut injector: Box<dyn InputInjector> = match create_injector() {
        Ok(injector) => injector,
        Err(error) => {
            eprintln!("创建注入器失败: {error}");
            alive.store(false, Ordering::Relaxed);
            return;
        }
    };
    injector.set_viewport(viewport);
    loop {
        match protocol::read_packet(&mut stream) {
            Ok(Some(packet)) => match packet.kind {
                protocol::KIND_INPUT => match decode_event(&packet.payload) {
                    Ok(event) => {
                        if injector.inject(event).is_ok() {
                            injected.fetch_add(1, Ordering::Relaxed);
                        } else {
                            errors.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Err(_) => {
                        errors.fetch_add(1, Ordering::Relaxed);
                    }
                },
                protocol::KIND_BYE => break,
                _ => {}
            },
            Ok(None) | Err(_) => break,
        }
    }
    alive.store(false, Ordering::Relaxed);
}
