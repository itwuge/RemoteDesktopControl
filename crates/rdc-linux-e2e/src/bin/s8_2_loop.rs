//! s8_2_loop · 单进程闭环：采集→软编→解码→上屏
//!
//! 用途：验证 Linux 软件编码后端能在真实 X11 会话里跑通完整链路。
//! 运行：`DISPLAY=:0 XAUTHORITY=... cargo run -p rdc-linux-e2e --bin s8_2_loop -- --secs 10 --fps 60 --bitrate 8000000`
//!
//! 输出：每秒一行 `LOOP_SECOND`，结束一行 `LOOP_JSON`（判据取证用）。
//! 说明：上屏窗口与被采集画面同屏，会出现「画中画」递归镜像，属预期现象。

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
    // Linux 编译已验证（2026-11-08，cargo build --workspace --all-targets 退出码 0、0 error）；运行语义仍未验证。
    // 本模块只在 Linux 目标编译，Windows 构建不覆盖此处的类型改动。
    use rdc_linux_e2e::error::E2eError;

    /// 参数。
    pub struct Args {
        pub secs: u64,
        pub fps: u32,
        pub bitrate_bps: u32,
        pub display_id: u32,
        pub max_backlog: u64,
        pub win_scale: u32,
        pub win_x: i16,
        pub win_y: i16,
    }

    /// 解析命令行。
    pub fn parse_args() -> Result<Args, E2eError> {
        let mut args = Args {
            secs: 10,
            fps: 60,
            bitrate_bps: 8_000_000,
            display_id: 0,
            max_backlog: 3,
            win_scale: 1,
            win_x: 0,
            win_y: 0,
        };
        let mut iter = std::env::args().skip(1);
        while let Some(arg) = iter.next() {
            let value = iter.next();
            match arg.as_str() {
                "--secs" => args.secs = value.ok_or("--secs 缺值")?.parse().map_err(|error| E2eError::with_source("--secs 需整数", error))?,
                "--fps" => args.fps = value.ok_or("--fps 缺值")?.parse().map_err(|error| E2eError::with_source("--fps 需整数", error))?,
                "--bitrate" => {
                    args.bitrate_bps = value.ok_or("--bitrate 缺值")?.parse().map_err(|error| E2eError::with_source("--bitrate 需整数", error))?
                }
                "--display-id" => {
                    args.display_id = value.ok_or("--display-id 缺值")?.parse().map_err(|error| E2eError::with_source("--display-id 需整数", error))?
                }
                "--max-backlog" => {
                    args.max_backlog = value.ok_or("--max-backlog 缺值")?.parse().map_err(|error| E2eError::with_source("--max-backlog 需整数", error))?
                }
                "--win-scale" => {
                    args.win_scale = value.ok_or("--win-scale 缺值")?.parse().map_err(|error| E2eError::with_source("--win-scale 需整数", error))?
                }
                "--win-x" => args.win_x = value.ok_or("--win-x 缺值")?.parse().map_err(|error| E2eError::with_source("--win-x 需整数", error))?,
                "--win-y" => args.win_y = value.ok_or("--win-y 缺值")?.parse().map_err(|error| E2eError::with_source("--win-y 需整数", error))?,
                other => return Err(E2eError::message(format!("未知参数: {other}"))),
            }
        }
        Ok(args)
    }
}

#[cfg(target_os = "linux")]
use std::sync::atomic::{AtomicU64, Ordering};
#[cfg(target_os = "linux")]
use std::sync::{Arc, Mutex};
#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
use rdc_capture::{create_source, CaptureConfig, ChromaFormat, Frame, FrameSink};
#[cfg(target_os = "linux")]
use rdc_codec::{create_encoder, probe_encoders, CodecKind, VideoChroma, VideoConfig, VideoEncoder};
#[cfg(target_os = "linux")]
use rdc_linux_e2e::error::lock_poisoned_text;
#[cfg(target_os = "linux")]
use rdc_linux_e2e::{clock, decoder::Decoder, display::Display};

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
    if let Err(error) = run(&args) {
        eprintln!("运行失败: {error}");
        return 2;
    }
    0
}

#[cfg(target_os = "linux")]
struct LoopSink {
    encoder: Arc<Mutex<Box<dyn VideoEncoder>>>,
    captured: Arc<AtomicU64>,
    pushed: Arc<AtomicU64>,
    produced: Arc<AtomicU64>,
    errors: Arc<AtomicU64>,
    backpressure: Arc<AtomicU64>,
    max_backlog: u64,
}

#[cfg(target_os = "linux")]
impl FrameSink for LoopSink {
    fn on_frame(&self, frame: rdc_capture::Frame) {
        self.captured.fetch_add(1, Ordering::Relaxed);
        let pushed = self.pushed.load(Ordering::Relaxed);
        let produced = self.produced.load(Ordering::Relaxed);
        if pushed.saturating_sub(produced) >= self.max_backlog {
            // 编码端积压超过阈值：丢帧，避免时延与内存随时间增长。
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

/// 计算分位数（输入已排序或未排序均可，函数内部排序）。
#[cfg(target_os = "linux")]
fn percentile(values: &mut [u64], pct: f64) -> u64 {
    if values.is_empty() {
        return 0;
    }
    values.sort_unstable();
    let idx = ((values.len() as f64 - 1.0) * pct).round() as usize;
    values[idx.min(values.len() - 1)]
}

#[cfg(target_os = "linux")]
fn run(args: &linux_impl::Args) -> Result<(), String> {
    // 1) 能力探测（E1：后端可探测）。
    let report = probe_encoders();
    println!(
        "PROBE hardware_available={} hardware={} software={} error={:?}",
        report.hardware_available,
        report.hardware.len(),
        report.software.len(),
        report.error
    );
    for cap in &report.software {
        println!(
            "CAPS name={} hardware={} codec={:?} max={}x{}@{} chroma={:?} bit_depth={} bitrate={}..{} runtime_reconfigure={} note={}",
            cap.name,
            cap.hardware,
            cap.codec,
            cap.max_width,
            cap.max_height,
            cap.max_fps,
            cap.chroma,
            cap.bit_depth,
            cap.min_bitrate_bps,
            cap.max_bitrate_bps,
            cap.runtime_reconfigure,
            cap.note
        );
    }

    // 2) 采集源。
    let mut source = create_source().map_err(|error| format!("创建采集源失败: {error}"))?;
    let displays = source.enumerate();
    for display in &displays {
        println!(
            "DISPLAY id={} name={} {}x{}@{} primary={}",
            display.id, display.name, display.width, display.height, display.refresh_hz, display.is_primary
        );
    }
    let display = displays
        .iter()
        .find(|d| d.id == args.display_id)
        .or_else(|| displays.first())
        .ok_or("未枚举到显示器")?;
    let (width, height) = (display.width, display.height);

    // 3) 编码器。
    let mut encoder = create_encoder(CodecKind::H264).map_err(|error| format!("创建编码器失败: {error}"))?;
    let config = VideoConfig {
        codec: CodecKind::H264,
        width,
        height,
        fps: args.fps,
        bitrate_bps: args.bitrate_bps,
        chroma: VideoChroma::Yuv420,
        bit_depth: 8,
        low_latency: true,
    };
    encoder
        .configure(config)
        .map_err(|error| format!("配置编码器失败: {error}"))?;
    println!("ENCODER backend={}", encoder.backend_name());
    let encoder = Arc::new(Mutex::new(encoder));

    // 4) 计数器与 sink。
    let captured = Arc::new(AtomicU64::new(0));
    let pushed = Arc::new(AtomicU64::new(0));
    let produced = Arc::new(AtomicU64::new(0));
    let errors = Arc::new(AtomicU64::new(0));
    let backpressure = Arc::new(AtomicU64::new(0));
    let sink = Arc::new(LoopSink {
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
            display_id: display.id,
            target_fps: args.fps,
            include_cursor: false,
            chroma: ChromaFormat::Bgra,
            bit_depth: 8,
        })
        .map_err(|error| format!("启动采集失败: {error}"))?;
    println!("CAPTURE backend={}", source.backend_name());

    // 5) 上屏窗口（默认全屏覆盖，会形成画中画递归镜像；--win-scale 可缩小避让）。
    let scale = args.win_scale.max(1);
    let dw = (width / scale).max(1);
    let dh = (height / scale).max(1);
    let mut display_win = Display::create(dw, dh, args.win_x, args.win_y, "rdc-s8-2-loop")?;
    println!(
        "DISPLAY window={} {}x{} at ({},{})",
        display_win.window_id(),
        dw,
        dh,
        args.win_x,
        args.win_y
    );
    let mut scaled = if scale > 1 {
        vec![0u8; dw as usize * dh as usize * 4]
    } else {
        Vec::new()
    };

    // 6) 解码进程。
    let mut decoder = Decoder::spawn(width, height)?;

    // 7) 主循环。
    let start = Instant::now();
    let mut next_report = 1u64;
    let mut bytes_total = 0u64;
    let mut packets = 0u64;
    let mut displayed = 0u64;
    let mut latencies: Vec<u64> = Vec::new();
    let mut push_errors = 0u64;

    while start.elapsed().as_secs() < args.secs {
        if !display_win.pump() {
            break;
        }
        drain_encode(
            &encoder,
            &produced,
            &mut decoder,
            &mut packets,
            &mut bytes_total,
            &mut push_errors,
        );
        while let Some(decoded) = decoder.poll() {
            let pts = decoded.pts_ns;
            let now = clock::mono_ns();
            if pts > 0 && now > pts {
                latencies.push(now - pts);
            }
            let frame = decoded.data.as_slice();
            if scale > 1 {
                rdc_linux_e2e::display::scale_nearest(frame, width, height, &mut scaled, dw, dh);
                display_win.present(&scaled)?;
            } else {
                display_win.present(frame)?;
            }
            displayed += 1;
        }
        let elapsed = start.elapsed().as_secs();
        if elapsed >= next_report {
            next_report = elapsed + 1;
            println!(
                "LOOP_SECOND t={elapsed}s captured={} pushed={} packets={} decoded={} displayed={} bp_dropped={}",
                captured.load(Ordering::Relaxed),
                pushed.load(Ordering::Relaxed),
                packets,
                decoder.frames(),
                displayed,
                backpressure.load(Ordering::Relaxed)
            );
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    let loop_elapsed = start.elapsed();
    source.stop();

    // 8) 收尾：冲刷编码器 → 解码剩余 → 上屏。
    let rest = {
        let mut guard = encoder.lock().map_err(|poison| lock_poisoned_text("编码器锁被污染", poison))?;
        guard
            .flush()
            .map_err(|error| format!("冲刷编码器失败: {error}"))?
    };
    for packet in rest {
        produced.fetch_add(1, Ordering::Relaxed);
        bytes_total += packet.data.len() as u64;
        packets += 1;
        decoder.push(&packet.data, packet.pts_ns)?;
    }
    decoder.finish();
    while let Some(decoded) = decoder.poll() {
        let pts = decoded.pts_ns;
        let now = clock::mono_ns();
        if pts > 0 && now > pts {
            latencies.push(now - pts);
        }
        let frame = decoded.data.as_slice();
        if scale > 1 {
            rdc_linux_e2e::display::scale_nearest(frame, width, height, &mut scaled, dw, dh);
            display_win.present(&scaled)?;
        } else {
            display_win.present(frame)?;
        }
        displayed += 1;
    }

    let snapshot = source.stats();
    let capture_frames = captured.load(Ordering::Relaxed);
    let decode_frames = decoder.frames();
    let secs = loop_elapsed.as_secs_f64().max(0.001);
    // latencies 以纳秒记录（CLOCK_MONOTONIC），JSON 里统一换算成微秒。
    let lat_p50 = percentile(&mut latencies.clone(), 0.50) / 1000;
    let lat_p99 = percentile(&mut latencies.clone(), 0.99) / 1000;
    let (enc_p50, enc_p99, enc_fps, enc_bitrate, backend) = {
        let guard = encoder.lock().map_err(|poison| lock_poisoned_text("编码器锁被污染", poison))?;
        let name = guard.backend_name().to_string();
        match guard.stats_snapshot() {
            Some(snapshot) => (
                snapshot.latency_p50_us,
                snapshot.latency_p99_us,
                snapshot.fps,
                snapshot.avg_bitrate_bps,
                name,
            ),
            None => (0, 0, 0.0, 0, name),
        }
    };
    println!(
        "LOOP_JSON {{\"width\":{},\"height\":{},\"secs\":{:.3},\"captured\":{},\"capture_fps\":{:.2},\"pushed\":{},\"backpressure_dropped\":{},\"push_errors\":{},\"packets\":{},\"packet_fps\":{:.2},\"decoded\":{},\"decoder_dropped\":{},\"displayed\":{},\"bytes\":{},\"bitrate_bps\":{},\"enc_fps\":{:.2},\"enc_bitrate_bps\":{},\"lat_p50_us\":{},\"lat_p99_us\":{},\"enc_p50_us\":{},\"enc_p99_us\":{},\"backend\":\"{}\"}}",
        width,
        height,
        secs,
        capture_frames,
        capture_frames as f64 / secs,
        pushed.load(Ordering::Relaxed),
        backpressure.load(Ordering::Relaxed),
        errors.load(Ordering::Relaxed) + push_errors,
        packets,
        packets as f64 / secs,
        decode_frames,
        decoder.dropped(),
        displayed,
        bytes_total,
        (bytes_total as f64 * 8.0 / secs) as u64,
        enc_fps,
        enc_bitrate,
        lat_p50,
        lat_p99,
        enc_p50,
        enc_p99,
        backend
    );
    println!("LOOP_BACKEND backend={}", backend);
    println!(
        "LOOP_CAPTURE captured={} dropped={} pool_exhausted={} cost_p50_us={} cost_p99_us={}",
        snapshot.captured, snapshot.dropped, snapshot.pool_exhausted, snapshot.cost_p50_us, snapshot.cost_p99_us
    );
    display_win.destroy();
    Ok(())
}

/// 把编码器当前可取的包全部送进解码器。
#[cfg(target_os = "linux")]
#[allow(clippy::too_many_arguments)]
fn drain_encode(
    encoder: &Arc<Mutex<Box<dyn VideoEncoder>>>,
    produced: &AtomicU64,
    decoder: &mut Decoder,
    packets: &mut u64,
    bytes_total: &mut u64,
    push_errors: &mut u64,
) {
    loop {
        let packet = match encoder.lock() {
            Ok(mut guard) => guard.poll_packet(),
            Err(_) => None,
        };
        let Some(packet) = packet else {
            return;
        };
        produced.fetch_add(1, Ordering::Relaxed);
        *bytes_total += packet.data.len() as u64;
        *packets += 1;
        if decoder.push(&packet.data, packet.pts_ns).is_err() {
            *push_errors += 1;
        }
    }
}
