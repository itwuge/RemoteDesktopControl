//! rdc-cli / S3b-4 端到端玻璃到玻璃实测
//!
//! 层级：L4 验证入口（Windows）
//! 用途：本机两进程（host / ctrl）经 UDP 回环传 1080p H.264 码流；
//!       host 在画内写入递增时间码（7 段数码管 + 24 位机读条码），
//!       ctrl 解码上屏后从**刚上屏的背缓冲**回读条码，与本机单调时钟相减得端到端延迟。
//! 调用方：`rdc-cli s3b4`（驱动，拉起 host 与 ctrl 两个子进程）

use std::error::Error;

/// 自检错误类型。
type S3bResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

/// 默认持续秒数。
const DEFAULT_SECS: u64 = 10;
/// host 监听端口。
const HOST_PORT: u16 = 45000;
/// ctrl 绑定端口。
const CTRL_PORT: u16 = 45100;
/// 目标码率（bps）。
const TARGET_BITRATE: u32 = 8_000_000;

/// 命令行参数。
struct Options {
    /// 持续秒数。
    secs: u64,
    /// 日志目录。
    out: String,
}

/// 解析 `--secs` 与 `--out`。
fn parse(args: &mut impl Iterator<Item = String>) -> Options {
    let mut options = Options {
        secs: DEFAULT_SECS,
        out: String::new(),
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--secs" => {
                if let Some(value) = args.next().and_then(|v| v.parse::<u64>().ok()) {
                    options.secs = value.clamp(1, 60);
                }
            }
            "--out" => {
                if let Some(value) = args.next() {
                    options.out = value;
                }
            }
            _ => {}
        }
    }
    if options.out.is_empty() {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        options.out = format!("logs/s3b4-{secs}");
    }
    options
}

/// 入口：按子命令分发 `host` / `ctrl` / 其余视为驱动。
///
/// # Arguments
/// * `args` - `s3b4` 之后的参数。
///
/// # Returns
/// 全部子步骤成功返回 `Ok(())`。
///
/// # Errors
/// 子进程失败、端口占用、编解码不可用时返回错误。
pub fn run(args: &mut impl Iterator<Item = String>) -> S3bResult<()> {
    // 子命令是可选的：`s3b4 host/ctrl ...` 走子进程，其余参数直接当驱动选项解析。
    let collected: Vec<String> = args.collect();
    let (role, rest) = match collected.first().map(String::as_str) {
        Some("host") => ("host", collected[1..].to_vec()),
        Some("ctrl") => ("ctrl", collected[1..].to_vec()),
        Some("exp") => ("exp", collected[1..].to_vec()),
        _ => ("", collected),
    };
    let options = parse(&mut rest.into_iter());
    #[cfg(not(target_os = "windows"))]
    {
        let _unused = (role, options);
        return Err("S3b-4 只实现于 Windows".into());
    }
    #[cfg(target_os = "windows")]
    {
        match role {
            "host" => windows_impl::run_host(&options),
            "ctrl" => windows_impl::run_ctrl(&options),
            // D2 受控实验：不经过网络，直接驱动同一个解码器。
            "exp" => windows_impl::run_exp(&options),
            _ => windows_impl::run_driver(&options),
        }
    }
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use std::collections::VecDeque;
    use std::io::Write;
    use std::net::SocketAddr;
    use std::path::Path;
    use std::time::{Duration, Instant};

    use rdc_codec::{create_encoder, CodecKind, VideoChroma, VideoConfig};
    use rdc_decode::{open_video_decoder, MfH264Decoder, PresentWindow};
    use rdc_protocol::{PacketHeader, StreamKind};
    use rdc_runtime::pool::FramePool;
    use rdc_transport::{FragmentHeader, Pacer, Reassembler, UdpTransport};
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    use super::{Options, S3bResult, CTRL_PORT, HOST_PORT, TARGET_BITRATE};

    /// 帧尺寸。
    const WIDTH: u32 = 1920;
    /// 帧尺寸。
    const HEIGHT: u32 = 1080;
    /// 每帧字节数（RGBA）。
    const FRAME_BYTES: usize = (WIDTH as usize) * (HEIGHT as usize) * 4;
    /// 帧间隔（微秒）。
    const FRAME_PERIOD_US: u64 = 16_667;
    /// 机读条码位数。
    const BITS: usize = 24;
    /// 条码首格左上角坐标。
    const STRIPE_X: u32 = 48;
    /// 条码纵坐标。
    const STRIPE_Y: u32 = 96;
    /// 单格边长。
    const CELL: u32 = 24;
    /// 单格步进。
    const CELL_STEP: u32 = 36;

    /// 迟到阈值（微秒）：访问单元到达时已比采集时刻晚这么多，就不再送解码。
    const STALE_US: u64 = 150_000;

    /// pacer 目标码率上限（bps）：回环没有真实瓶颈，只需防抖，不需要更大预算。
    const MAX_PACER_BPS: f64 = 64_000_000.0;

    /// 编码器实测码率的滑动窗口（1 秒）。
    ///
    /// 只统计「每帧实际编码字节」再乘以目标帧率，**不能**用墙钟吞吐：
    /// 发送一旦被限速，墙钟吞吐会跟着掉，形成「越限越慢」的正反馈死锁。
    struct RateWindow {
        /// (记录时刻, 该帧编码字节数)；0 字节也要记（编码器可能尚未吐帧）。
        entries: std::collections::VecDeque<(Instant, u64)>,
        /// 编码推送帧率：把每帧字节折算成码率。
        fps: u32,
    }

    impl RateWindow {
        /// 新建窗口（`fps` 为编码推送帧率）。
        fn new(fps: u32) -> Self {
            Self {
                entries: std::collections::VecDeque::new(),
                fps: fps.max(1),
            }
        }

        /// 记录一帧编码输出字节数，并淘汰 1 秒之前的记录。
        fn push(&mut self, now: Instant, bytes: u64) {
            self.entries.push_back((now, bytes));
            while let Some(&(at, _)) = self.entries.front() {
                if now.duration_since(at).as_secs_f64() > 1.0 {
                    self.entries.pop_front();
                } else {
                    break;
                }
            }
        }

        /// 窗口内实测输出码率（bit/s）；样本不足 8 帧时返回 `None`（首帧 IDR 会严重高估）。
        fn measured_bps(&self) -> Option<f64> {
            if self.entries.len() < 8 {
                return None;
            }
            let total: u64 = self.entries.iter().map(|(_, bytes)| *bytes).sum();
            Some(total as f64 * 8.0 * f64::from(self.fps) / self.entries.len() as f64)
        }
    }

    /// 单帧端到端样本（同时保留原始值以便复核）。
    struct SampleRecord {
        /// 帧号（来自包头）。
        frame_id: u16,
        /// 该访问单元到达时的包头时间戳（host 采集时刻）。
        au_ts_us: u64,
        /// 解码器吐出的帧时间戳（pts，仅用于排序核查）。
        decoded_ts_us: u64,
        /// 从画面回读出的 24 位时间码。
        tcode: u64,
        /// 期望值：包头时间戳的 24 位截断。
        expected: u64,
        /// 回读时刻的 QPC 24 位截断。
        now24: u64,
        /// 端到端延迟（微秒）。
        e2e_us: u64,
        /// 从送入该访问单元到解出该帧的耗时（微秒）：解码 + 整理拷贝。
        feed_to_out_us: u64,
        /// 采集时刻 → 完整访问单元重组完成的耗时（微秒）：编码排队 + 打包 + 传输 + 重组。
        transport_us: u64,
        /// 本帧的上屏耗时（微秒）。
        present_us: u64,
        /// 本帧的回读耗时（微秒，含 GPU 回读与时间码解码）。
        probe_us: u64,
        /// 回读时间码相对解码帧 pts 的偏移（微秒，有符号）：反映呈现/回读的固定滞后。
        readback_lag_us: i64,
    }

    /// 本机单调时钟（微秒）：QueryPerformanceCounter 同机跨进程同源。
    fn qpc_us() -> u64 {
        let mut counter = 0i64;
        let mut frequency = 0i64;
        unsafe {
            let _ = QueryPerformanceCounter(&mut counter); // gate-allow R-C-2: 失败时 counter 保持 0，紧随的 frequency<=0 分支返回 0
            let _ = QueryPerformanceFrequency(&mut frequency); // gate-allow R-C-2: 失败时 frequency 保持 0，紧随分支返回 0
        }
        if frequency <= 0 {
            return 0;
        }
        (counter as u128 * 1_000_000 / frequency as u128) as u64
    }

    /// 进程累计 CPU 时间（微秒，所有核累加）；失败时返回 0。
    fn process_cpu_us() -> u64 {
        let mut create = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        let process = unsafe { GetCurrentProcess() };
        let ok =
            unsafe { GetProcessTimes(process, &mut create, &mut exit, &mut kernel, &mut user) };
        if ok.is_err() {
            return 0;
        }
        let to_u64 =
            |time: FILETIME| (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime);
        // FILETIME 以 100ns 为单位。
        (to_u64(kernel) + to_u64(user)) / 10
    }

    /// 驱动：拉起 host 与 ctrl 两个子进程，各自落日志。
    pub(super) fn run_driver(options: &Options) -> S3bResult<()> {
        let _ = std::fs::create_dir_all(&options.out); // gate-allow R-C-2: 紧随其后的 std::fs::write(...)? 会在目录不可写时返回错误
        let exe = std::env::current_exe()?;
        println!(
            "== RDC S3b-4 端到端实测（UDP 回环，1080p，{} s）==",
            options.secs
        );
        println!("日志目录             : {}", options.out);
        let host = std::process::Command::new(&exe)
            .args([
                "s3b4",
                "host",
                "--secs",
                &options.secs.to_string(),
                "--out",
                &options.out,
            ])
            .spawn()?;
        // ctrl 多跑 2 s，等 host 收尾后再退，避免截断尾部样本。
        let ctrl = std::process::Command::new(&exe)
            .args([
                "s3b4",
                "ctrl",
                "--secs",
                &(options.secs + 2).to_string(),
                "--out",
                &options.out,
            ])
            .spawn()?;
        let host_status = host.wait_with_output()?;
        let ctrl_status = ctrl.wait_with_output()?;
        if !host_status.status.success() || !ctrl_status.status.success() {
            return Err("host 或 ctrl 子进程失败，详见各自日志".into());
        }
        for name in ["host.json", "report.json"] {
            let path = Path::new(&options.out).join(name);
            if let Ok(text) = std::fs::read_to_string(&path) {
                println!("== {name} ==\n{text}");
            }
        }
        Ok(())
    }

    /// D2 受控实验：同一段码流，两种送法驱动同一个解码器，量清「17 帧」到底是谁的。
    ///
    /// - 连续送入：一直送到解码器返回 NeedInput 才停（等价生产链路的送包节奏上限）。
    /// - 严格串行：送一帧、等到有输出、再送下一帧。
    /// - 自有队列：从最近一次端到端实测的 decode-trace.csv 里取「首次拿到输出时已送几帧」。
    pub(super) fn run_exp(options: &Options) -> S3bResult<()> {
        let window = PresentWindow::create(WIDTH / 2, HEIGHT / 2, "RDC S3d-D2 解码受控实验")?;
        let device = window.device().clone();

        // 码流准备：120 帧合成画面，与 s3b4 host 完全同一编码路径与参数。
        let mut encoder = create_encoder(CodecKind::H264)?;
        encoder.configure(VideoConfig {
            codec: CodecKind::H264,
            width: WIDTH,
            height: HEIGHT,
            fps: 60,
            bitrate_bps: TARGET_BITRATE,
            chroma: VideoChroma::Yuv420,
            bit_depth: 8,
            low_latency: true,
        })?;
        let pool = FramePool::new(4, FRAME_BYTES);
        let base = build_base();
        let mut aus: Vec<Vec<u8>> = Vec::new();
        let mut ts_us = 1_000_000u64;
        for index in 0..120u64 {
            let mut buffer = pool.acquire().ok_or("帧池已空")?;
            buffer.as_mut_slice().copy_from_slice(&base);
            draw_frame(buffer.as_mut_slice(), ts_us, index);
            encoder.push(buffer, index * 1_000_000_000 / 60, index % 120 == 0)?;
            while let Some(packet) = encoder.poll_packet() {
                let mut annexb = Vec::with_capacity(packet.data.len() + 64);
                if packet.is_keyframe {
                    let header = encoder.sequence_header();
                    let header = if header.is_empty() {
                        rdc_codec::extract_sps_pps(&packet.data)
                    } else {
                        header.to_vec()
                    };
                    annexb.extend_from_slice(&header);
                }
                annexb.extend_from_slice(&packet.data);
                aus.push(annexb);
                ts_us += FRAME_PERIOD_US;
            }
        }
        println!("[exp] 码流准备完成：{} 个访问单元", aus.len());
        if aus.len() < 40 {
            return Err("编码输出过少，无法做受控实验".into());
        }

        // 实验 1：连续送入，直到需要更多输入。
        let mut decoder = MfH264Decoder::open_with_device(&device, WIDTH, HEIGHT, 60, true)?;
        let mut outputs = 0u64;
        let mut first_out_feeds = 0u64;
        let mut first_out_ms = 0f64;
        let mut feed_ms: Vec<u64> = Vec::new();
        let mut max_in_flight = 0u64;
        let exp_start = Instant::now();
        for (index, au) in aus.iter().enumerate() {
            let feeds = index as u64 + 1;
            let feed_start = Instant::now();
            decoder.feed_each(au, ts_us, |_frame| {
                if outputs == 0 {
                    first_out_feeds = feeds;
                    first_out_ms = exp_start.elapsed().as_secs_f64() * 1000.0;
                }
                outputs += 1;
                Ok(())
            })?;
            feed_ms.push(feed_start.elapsed().as_micros() as u64);
            max_in_flight = max_in_flight.max(decoder.in_flight());
        }
        // 送完后再把管线里剩下的帧取干净，用于统计总产出与末态 in_flight。
        let continuous_steady_in_flight = decoder.in_flight();
        for _ in 0..32 {
            let pumped = decoder.pump()?;
            if pumped.is_empty() {
                break;
            }
            outputs += pumped.len() as u64;
        }
        let continuous_hw = decoder.hardware_active();
        let feed_p50_us = percentile(&feed_ms, 0.50);
        let feed_p99_us = percentile(&feed_ms, 0.99);
        println!(
            "[exp] 连续送入：首次输出在送入第 {first_out_feeds} 帧后（{first_out_ms:.1} ms），峰值 in_flight={max_in_flight}，末态 {continuous_steady_in_flight}，共输出 {outputs}/{}",
            aus.len()
        );

        // 实验 2：送一帧、等一帧，严格串行。
        let mut decoder = MfH264Decoder::open_with_device(&device, WIDTH, HEIGHT, 60, true)?;
        let mut feeds = 0u64;
        let mut serial_outputs = 0u64;
        let mut first_out_feeds_serial = 0u64;
        let mut first_out_ms_serial = 0f64;
        let mut wait_ms: Vec<u64> = Vec::new();
        let mut timeouts = 0u64;
        let mut stalled_at = 0u64;
        let mut stall_message = String::new();
        let serial_start = Instant::now();
        let serial_result: S3bResult<()> = (|decoder: &mut MfH264Decoder| -> S3bResult<()> {
            for (index, au) in aus.iter().take(60).enumerate() {
                feeds = index as u64 + 1;
                let before_feed = serial_outputs;
                let feed_start = Instant::now();
                decoder.feed_each(au, ts_us, |_frame| {
                    if serial_outputs == 0 {
                        first_out_feeds_serial = feeds;
                        first_out_ms_serial = serial_start.elapsed().as_secs_f64() * 1000.0;
                    }
                    serial_outputs += 1;
                    Ok(())
                })?;
                // 本次送入期间没出帧，就一直探测到出帧为止（最多 50 ms/帧）。
                while serial_outputs == before_feed {
                    let pumped = decoder.pump()?;
                    serial_outputs += pumped.len() as u64;
                    if serial_outputs > before_feed {
                        if first_out_feeds_serial == 0 {
                            first_out_feeds_serial = feeds;
                            first_out_ms_serial = serial_start.elapsed().as_secs_f64() * 1000.0;
                        }
                        break;
                    }
                    if feed_start.elapsed().as_millis() >= 50 {
                        timeouts += 1;
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
                wait_ms.push(feed_start.elapsed().as_millis() as u64);
                if serial_outputs >= 40 {
                    break;
                }
            }
            Ok(())
        })(&mut decoder);
        if let Err(error) = serial_result {
            // MFT 不再接收输入（输出表面池/内部队列已满）本身就是结论，不要直接崩掉实验。
            stalled_at = feeds;
            stall_message = error.to_string();
            eprintln!("[exp] 串行送入在第 {stalled_at} 帧停摆：{stall_message}");
        }
        let serial_wait_p50 = percentile(&wait_ms, 0.50);
        let serial_wait_p99 = percentile(&wait_ms, 0.99);
        println!(
            "[exp] 严格串行：首次输出在送入第 {first_out_feeds_serial} 帧后（{first_out_ms_serial:.1} ms），等待 p50/p99 {serial_wait_p50}/{serial_wait_p99} ms，超时 {timeouts} 次，共输出 {serial_outputs}"
        );

        // 实验 3：自有队列预填深度——取最近一次端到端实测的逐帧 trace。
        let trace_path = Path::new("logs/s3d-d1-run8/decode-trace.csv");
        let mut our_prefill_feeds = 0u64;
        let mut our_prefill_ms = 0u64;
        let mut trace_source = "缺失".to_owned();
        if let Ok(text) = std::fs::read_to_string(trace_path) {
            trace_source = trace_path.display().to_string();
            for line in text.lines().skip(1) {
                let cols: Vec<&str> = line.split(',').collect();
                if cols.len() >= 4 && cols[2].parse::<u64>().unwrap_or(0) > 0 {
                    our_prefill_ms = cols[0].parse().unwrap_or(0);
                    // trace 的 in_flight 就是「已送 − 已出」，即首次拿到输出时的送入帧数。
                    our_prefill_feeds = cols[1].parse().unwrap_or(0);
                    break;
                }
            }
        }
        println!(
            "[exp] 自有队列：首次拿到输出时 in_flight={our_prefill_feeds}（{our_prefill_ms} ms），来源 {trace_source}"
        );

        let json = format!(
            "{{\"mode\":\"d2\",\"aus\":{},\
             \"continuous\":{{\"first_output_after_feeds\":{first_out_feeds},\"first_output_ms\":{first_out_ms:.1},\
             \"feed_p50_us\":{feed_p50_us},\"feed_p99_us\":{feed_p99_us},\"outputs\":{outputs},\
             \"max_in_flight\":{max_in_flight},\"end_in_flight\":{continuous_steady_in_flight},\
             \"hardware\":{continuous_hw}}},\
             \"serial\":{{\"first_output_after_feeds\":{first_out_feeds_serial},\"first_output_ms\":{first_out_ms_serial:.1},\
             \"wait_p50_ms\":{serial_wait_p50},\"wait_p99_ms\":{serial_wait_p99},\"timeouts\":{timeouts},\
             \"outputs\":{serial_outputs},\"stalled_at_feed\":{stalled_at},\"stall\":\"{stall_message}\"}},\
             \"our_queue\":{{\"first_output_in_flight\":{our_prefill_feeds},\"first_output_at_ms\":{our_prefill_ms},\"source\":\"{trace_source}\"}}}}",
            aus.len()
        );
        let _ = std::fs::create_dir_all(&options.out); // gate-allow R-C-2: 紧随其后的 std::fs::write(d2.json)? 会在目录不可写时返回错误
        std::fs::write(Path::new(&options.out).join("d2.json"), json)?;
        println!("[exp] 已写 {}/d2.json", options.out);
        Ok(())
    }

    /// host：合成带时间码的画面 → H.264 编码 → 分片 → UDP 发送。
    pub(super) fn run_host(options: &Options) -> S3bResult<()> {
        let udp = UdpTransport::bind(&format!("127.0.0.1:{HOST_PORT}"))?;
        println!("[host] 监听 127.0.0.1:{HOST_PORT}，等待 ctrl 握手");
        let mut peer: Option<SocketAddr> = None;
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline && peer.is_none() {
            let received = udp.recv_from(Duration::from_millis(20));
            // ctrl 可能先于 host 绑定就发 hello，ICMP 会把 recv 打成连接重置，忽略即可。
            let Ok(Some((addr, header, _))) = received else {
                continue;
            };
            if header.stream == StreamKind::Control as u8 {
                // 回一个同类型的空包作为握手确认，给 ctrl 一个明确的「可以开始」信号。
                udp.send_to(addr, &header, &[])?;
                peer = Some(addr);
            }
        }
        let peer = peer.ok_or("等待 ctrl 握手超时")?;
        println!("[host] ctrl 地址 {peer}");

        let mut encoder = create_encoder(CodecKind::H264)?;
        encoder.configure(VideoConfig {
            codec: CodecKind::H264,
            width: WIDTH,
            height: HEIGHT,
            fps: 60,
            bitrate_bps: TARGET_BITRATE,
            chroma: VideoChroma::Yuv420,
            bit_depth: 8,
            low_latency: true,
        })?;
        let pool = FramePool::new(4, FRAME_BYTES);
        let base = build_base();
        let mut pacer = Pacer::new(TARGET_BITRATE, 200_000)?;

        let mut seq = 0u32;
        let mut frame_id = 0u16;
        let mut frames = 0u64;
        let mut packets = 0u64;
        let mut bytes = 0u64;
        let mut encoded_bytes = 0u64;
        let mut rate_window = RateWindow::new(60);
        let mut pacer_bps = TARGET_BITRATE;
        let mut priority_skips = 0u64;
        let mut send_waits: Vec<u64> = Vec::new();
        // 待配对队列：(送入编码器的 pts, 该帧真实的采集时刻)。
        // 编码器有固有延迟，poll 取到的包可能属于更早的帧，必须按包的 pts 回查。
        // capacity: 32（下方 len>32 会挤掉最旧项）。
        let mut pending_captures: VecDeque<(u64, u64)> = VecDeque::with_capacity(32);
        let mut pair_misses = 0u64;
        let started = Instant::now();

        while started.elapsed().as_secs() < options.secs {
            let target_us = frames * FRAME_PERIOD_US;
            while (started.elapsed().as_micros() as u64) < target_us {
                std::thread::sleep(Duration::from_millis(1));
            }
            let captured_us = qpc_us();
            let mut buffer = pool.acquire().ok_or("帧池已空")?;
            buffer.as_mut_slice().copy_from_slice(&base);
            draw_frame(buffer.as_mut_slice(), captured_us, frames);
            let pts_ns = frames * 1_000_000_000 / 60;
            encoder.push(buffer, pts_ns, frames % 120 == 0)?;
            frames += 1;
            // 编码器内部按 100ns 保存采样时间（`pts_ns / 100` 截断），
            // 配对键必须用同一精度，否则非整百纳秒的帧永远对不上。
            pending_captures.push_back((pts_ns / 100, captured_us));
            // 编码器最多残留 1~2 帧；这里留 32 帧余量，避免主机抖动时把待配对项挤掉。
            // 数量本身不影响配对正确性：键是包自带的 pts，只有被挤掉才会退化成兜底。
            while pending_captures.len() > 32 {
                pending_captures.pop_front();
            }
            // 帧率优先：本帧所有分片的发送等待不得越过一个帧周期的末尾。
            let frame_deadline = Instant::now() + Duration::from_micros(FRAME_PERIOD_US);
            let mut frame_bytes = 0u64;
            let mut frame_wait = 0u64;

            while let Some(packet) = encoder.poll_packet() {
                let mut annexb = Vec::with_capacity(packet.data.len() + 64);
                if packet.is_keyframe {
                    let header = encoder.sequence_header();
                    let header = if header.is_empty() {
                        rdc_codec::extract_sps_pps(&packet.data)
                    } else {
                        header.to_vec()
                    };
                    annexb.extend_from_slice(&header);
                }
                annexb.extend_from_slice(&packet.data);
                frame_bytes += annexb.len() as u64;
                // 关键：包头时间戳必须取「产出这个包的帧」的采集时刻，而不是本轮刚送进去那帧的。
                let packet_capture_us =
                    match take_capture(&mut pending_captures, packet.pts_ns / 100) {
                        Some(capture) => capture,
                        None => {
                            pair_misses += 1;
                            // 个别输出样本的采样时间为 0（MFT 未回填），此时它仍属于最早的待配对帧，
                            // 取队首比取本轮采集时刻更接近真实归属。
                            pending_captures
                                .pop_front()
                                .map_or(captured_us, |(_, capture)| capture)
                        }
                    };
                bytes += send_frame(
                    &udp,
                    peer,
                    &annexb,
                    &mut seq,
                    &mut frame_id,
                    &mut pacer,
                    packet_capture_us,
                    &mut packets,
                    frame_deadline,
                    &mut frame_wait,
                    &mut priority_skips,
                )?;
            }
            if packets > 0 {
                send_waits.push(frame_wait);
            }
            encoded_bytes += frame_bytes;
            // 每帧都记一次（含 0 字节帧）：码率按「每帧字节 × 60fps」折算，与发送节奏解耦。
            rate_window.push(Instant::now(), frame_bytes);
            if let Some(measured) = rate_window.measured_bps() {
                let target = (measured * 1.05)
                    .min(MAX_PACER_BPS)
                    .max(f64::from(TARGET_BITRATE));
                pacer_bps = target as u32;
                pacer.set_target_bitrate(pacer_bps)?;
            }
        }
        // 收尾：把编码器缓存吐干净。
        let drain_deadline = Instant::now() + Duration::from_millis(10);
        for packet in encoder.flush()? {
            let mut wait = 0u64;
            let packet_capture_us = match take_capture(&mut pending_captures, packet.pts_ns / 100) {
                Some(capture) => capture,
                None => {
                    pair_misses += 1;
                    pending_captures
                        .pop_front()
                        .map_or_else(qpc_us, |(_, capture)| capture)
                }
            };
            bytes += send_frame(
                &udp,
                peer,
                &packet.data,
                &mut seq,
                &mut frame_id,
                &mut pacer,
                packet_capture_us,
                &mut packets,
                drain_deadline,
                &mut wait,
                &mut priority_skips,
            )?;
        }
        let elapsed = started.elapsed().as_secs_f64().max(0.001);
        let measured_bps = rate_window.measured_bps().unwrap_or(0.0);
        let avg_frame_bytes = if frames == 0 {
            0.0
        } else {
            encoded_bytes as f64 / frames as f64
        };
        let send_wait_p50 = percentile(&send_waits, 0.50);
        let send_wait_p99 = percentile(&send_waits, 0.99);
        let fps = frames as f64 / elapsed;
        let json = format!(
            "{{\"role\":\"host\",\"seconds\":{:.2},\"frames\":{frames},\"packets\":{packets},\
             \"bytes\":{bytes},\"bitrate_bps\":{:.0},\"fps\":{fps:.2},\"encoded_bytes\":{encoded_bytes},\
             \"avg_frame_bytes\":{avg_frame_bytes:.0},\"measured_bps\":{measured_bps:.0},\
             \"pacer_target_bps\":{pacer_bps},\"fps_priority_skips\":{priority_skips},\"pair_misses\":{pair_misses},\
             \"send_wait_p50_us\":{send_wait_p50},\"send_wait_p99_us\":{send_wait_p99}}}",
            elapsed,
            bytes as f64 * 8.0 / elapsed
        );
        let _ = std::fs::create_dir_all(&options.out); // gate-allow R-C-2: 紧随其后的 std::fs::write(host.json)? 会在目录不可写时返回错误
        std::fs::write(Path::new(&options.out).join("host.json"), json)?;
        println!(
            "[host] 完成：{frames} 帧 / {packets} 包 / {:.1} Mbps / {fps:.1} fps / pacer {} Mbps / 实测 {:.1} Mbps / 帧率优先跳过 {priority_skips}",
            bytes as f64 * 8.0 / elapsed / 1_000_000.0,
            pacer_bps / 1_000_000,
            measured_bps / 1_000_000.0
        );
        Ok(())
    }

    /// 按编码包自带的 pts 回查它对应的采集时刻（键为编码器内部的 100ns 采样时间）。
    ///
    /// 编码器下一次 poll 取到的包通常属于上一帧（in-box MFT 固有 1 帧延迟），
    /// 用本轮 `captured_us` 会把时间戳配超前整整一帧——S3f 修的就是这处。
    fn take_capture(pending: &mut VecDeque<(u64, u64)>, pts_100ns: u64) -> Option<u64> {
        let index = pending.iter().position(|(pts, _)| *pts == pts_100ns)?;
        pending.remove(index).map(|(_, capture_us)| capture_us)
    }

    /// 把一个访问单元分片发送，返回发送字节数。
    #[allow(clippy::too_many_arguments)]
    fn send_frame(
        udp: &UdpTransport,
        peer: SocketAddr,
        annexb: &[u8],
        seq: &mut u32,
        frame_id: &mut u16,
        pacer: &mut Pacer,
        captured_us: u64,
        packets: &mut u64,
        frame_deadline: Instant,
        wait_us: &mut u64,
        priority_skips: &mut u64,
    ) -> S3bResult<u64> {
        // 媒体分片头占 8 字节，必须从载荷上限里扣掉，否则包头长度校验会拒绝。
        const SHARD: usize = rdc_protocol::FRAGMENT_PAYLOAD_MAX - 8;
        let shard_count = annexb.len().div_ceil(SHARD) as u16;
        let total_len = annexb.len() as u32;
        let mut sent = 0u64;
        for (index, chunk) in annexb.chunks(SHARD).enumerate() {
            let fragment = FragmentHeader {
                shard_index: index as u16,
                shard_count,
                total_len,
            };
            let mut payload = Vec::with_capacity(8 + chunk.len());
            payload.extend_from_slice(&fragment.encode());
            payload.extend_from_slice(chunk);
            let header = PacketHeader::new(
                1,
                StreamKind::VideoMain as u8,
                *seq,
                captured_us,
                *frame_id,
                payload.len() as u16,
            )?;
            // 发送节奏：追不上目标码率就等，避免把回环打爆掩盖真实延迟。
            // 但帧率优先：等到本帧周期末尾就不再等，宁可瞬时超码率也不许把帧拖慢。
            let delay = pacer.poll_delay_for(payload.len() + 32, Instant::now());
            if delay > Duration::ZERO {
                if Instant::now() >= frame_deadline {
                    *priority_skips += 1;
                } else {
                    let wait_start = Instant::now();
                    let wait = delay.min(Duration::from_millis(4));
                    std::thread::sleep(wait);
                    *wait_us += wait_start.elapsed().as_micros() as u64;
                }
            }
            udp.send_to(peer, &header, &payload)?;
            pacer.on_sent(payload.len() + 32, Instant::now());
            *seq += 1;
            *packets += 1;
            sent += (payload.len() + 32) as u64;
        }
        *frame_id = frame_id.wrapping_add(1);
        Ok(sent)
    }

    /// ctrl：收包重组 → 解码 → 上屏 → 回读条码 → 计算端到端延迟。
    pub(super) fn run_ctrl(options: &Options) -> S3bResult<()> {
        let udp = UdpTransport::bind(&format!("127.0.0.1:{CTRL_PORT}"))?;
        let peer: SocketAddr = format!("127.0.0.1:{HOST_PORT}").parse()?;
        let hello = PacketHeader::new(1, StreamKind::Control as u8, 0, 0, 0, 0)?;
        // 重试 hello 直到 host 回确认：两进程是并发拉起的，谁先绑定不确定。
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut acked = false;
        while Instant::now() < deadline && !acked {
            udp.send_to(peer, &hello, &[])?;
            // 10054（连接重置）在 UDP 下是 ICMP 的副产物，忽略并重试即可。
            if let Ok(Some(_)) = udp.recv_from(Duration::from_millis(150)) {
                acked = true;
            }
        }
        if !acked {
            return Err("等待 host 握手确认超时".into());
        }
        println!("[ctrl] 已握手 {peer}，开始接收");
        println!(
            "[ctrl] 解码选择 RDC_DECODER={:?} RDC_FFMPEG_MODE={:?}",
            std::env::var("RDC_DECODER").unwrap_or_default(),
            std::env::var("RDC_FFMPEG_MODE").unwrap_or_default()
        );

        let mut window = PresentWindow::create(WIDTH, HEIGHT, "RDC S3b-4 端到端")?;
        let scale = window.client_size().0 as f32 / WIDTH as f32;
        // 采样点：机读条码每格中心，按「帧坐标 → 客户区坐标」缩放。
        let mut points = Vec::with_capacity(BITS);
        for index in 0..BITS {
            let x = (STRIPE_X + index as u32 * CELL_STEP + CELL / 2) as f32 * scale;
            let y = (STRIPE_Y + CELL / 2) as f32 * scale;
            points.push((x as u32, y as u32));
        }
        window.set_probe_points(points);
        // 输入帧率≈刷新率时，每帧都等垂直同步会把呈现开销累积成秒级积压；
        // 这里不阻塞 Present，显示器仍按刷新率取最新画面（可见帧率上限仍是刷新率）。
        window.set_sync_interval(0);

        // 枚举证据 + 驱动侧 profile 证据（D28 需可复核）。
        let (hardware_names, all_names) = rdc_decode::decoder_names();
        println!("[ctrl] 硬件解码 MFT: {hardware_names:?}");
        println!("[ctrl] 全部解码 MFT: {all_names:?}");
        let h264_profiles = rdc_decode::h264_decoder_profile_count(window.device());
        println!("[ctrl] 适配器 H.264 解码 profile 数: {h264_profiles}");
        // 解码后端按 RDC_DECODER 选择（默认 MF；ffmpeg 需以 ffmpeg feature 构建）。
        // 解码器与呈现窗口共用同一 D3D11 设备：硬解输出直接是 GPU 纹理（零拷贝）。
        let mut decoder = open_video_decoder(Some(window.device()), WIDTH, HEIGHT, 60, true)?;
        let decoder_info = decoder.info();
        let decoder_name = decoder_info.name.clone();
        let decoder_dxva = decoder_info.dxva_aware;
        let decoder_hw_mft = decoder_info.hw_mft;
        println!(
            "[ctrl] 解码后端 {}：{}，hw={}，output={}，{}",
            decoder_info.backend,
            decoder_name,
            decoder_info.hardware,
            decoder_info.output.as_str(),
            decoder_info.detail
        );
        let mut reassembler = Reassembler::new(4);
        let mut samples: Vec<u64> = Vec::new();
        let mut records: Vec<SampleRecord> = Vec::new();
        let mut feed_times: Vec<u64> = Vec::new();
        let mut present_times: Vec<u64> = Vec::new();
        let mut probe_times: Vec<u64> = Vec::new();
        let mut skipped_stale = 0u64;
        let mut inflight_peak = 0u64;
        let mut decoded_frames = 0u64;
        let mut shown_before = 0u64;
        let mut trace: Vec<(u64, u64, usize, u64)> = Vec::new();
        let mut packets = 0u64;
        let mut bytes = 0u64;
        let mut frames = 0u64;
        // 首帧输出延迟（送入第一个访问单元 → 拿到第一帧输出），E1 的核心判据。
        let mut first_feed_at: Option<Instant> = None;
        let mut first_output_at: Option<Instant> = None;
        let mut first_output_after_feeds = 0u64;
        // 真实接收窗口：首帧到末帧。帧率必须以它为分母，不能混入握手/开窗/收尾时间。
        let mut stream_start: Option<Instant> = None;
        let mut stream_end = Instant::now();
        let cpu_start_us = process_cpu_us();
        let started = Instant::now();

        while started.elapsed().as_secs() < options.secs {
            let received = udp.recv_from(Duration::from_millis(2))?;
            let Some((_, header, payload)) = received else {
                window.pump();
                continue;
            };
            if payload.is_empty() {
                continue;
            }
            packets += 1;
            bytes += (payload.len() + 32) as u64;
            let Some(annexb) = reassembler.push(header.frame_id, &payload[..])? else {
                continue;
            };
            frames += 1;
            if stream_start.is_none() {
                stream_start = Some(Instant::now());
            }
            stream_end = Instant::now();
            let now_us = qpc_us();
            // 采集时刻 → 完整 AU 到达，这一段包含 host 编码排队/打包/UDP/重组。
            let transport_us = now_us.saturating_sub(header.ts_us);
            // 只丢「到达即已过期」的访问单元（编码/传输堵住时会触发）；
            // 本轮不实现丢帧追赶，解码器积压会如实反映在端到端延迟里。
            if now_us.saturating_sub(header.ts_us) > STALE_US {
                skipped_stale += 1;
                continue;
            }
            let feed_start = Instant::now();
            if first_feed_at.is_none() {
                first_feed_at = Some(feed_start);
            }
            // 逐帧上屏：解码器每解出一帧，立刻按呈现顺序上屏并释放输出样本。
            // 不能先攒成一批再上屏——in-box MFT 的输出表面池很浅，同时持有 4 帧会永久
            // MF_E_NOTACCEPTING（D1 诊断实测 fed=19 后停摆）。
            let frame_id_for_show = header.frame_id;
            decoder.feed_each(&annexb, header.ts_us, &mut |frame| {
                decoded_frames += 1;
                if first_output_at.is_none() {
                    first_output_at = Some(Instant::now());
                    first_output_after_feeds = frames;
                }
                let feed_to_out_us = feed_start.elapsed().as_micros() as u64;
                let present_start = Instant::now();
                if let Some(gpu) = frame.texture.as_ref() {
                    window.present_texture(&gpu.texture, gpu.array_slice)?;
                } else {
                    window.present_nv12(&frame.data, frame.stride)?;
                }
                present_times.push(present_start.elapsed().as_micros() as u64);
                let present_us = present_start.elapsed().as_micros() as u64;
                let probe_start = Instant::now();
                let probe = window.take_probe_samples();
                let probe_us = probe_start.elapsed().as_micros() as u64;
                probe_times.push(probe_us);
                if probe.len() != BITS {
                    return Ok(());
                }
                let mut value = 0u64;
                for (index, pixel) in probe.iter().enumerate() {
                    // 白格=1，黑格=0；亮度用绿色通道近似。
                    if u16::from(pixel[1]) > 128 {
                        value |= 1 << index;
                    }
                }
                let now = qpc_us();
                // 两端都用 24 位截断的 QPC，做模 2^24 差值即可还原（回绕差 <±8.38 s）。
                let now24 = now & 0xFF_FFFF;
                let diff = (now24 as i64 - value as i64 + (1 << 23)) & 0xFF_FFFF;
                let e2e = (diff - (1 << 23)).max(0) as u64;
                samples.push(e2e);
                // 回读时间码相对「刚解出的这一帧」的偏移：负值表示回读到的是更早的帧，
                // 即呈现/回读链路的固定落后量（报告把它作为分段证据，不改 e2e 口径）。
                let lag24 =
                    ((frame.ts_us & 0xFF_FFFF) as i64 - value as i64 + (1 << 23)) & 0xFF_FFFF;
                let readback_lag_us = lag24 - (1 << 23);
                records.push(SampleRecord {
                    frame_id: frame_id_for_show,
                    au_ts_us: header.ts_us,
                    decoded_ts_us: frame.ts_us,
                    tcode: value,
                    // 期望值取该解码帧自身时间码：对不上说明回读的不是这一帧。
                    expected: frame.ts_us & 0xFF_FFFF,
                    now24,
                    e2e_us: e2e,
                    feed_to_out_us,
                    transport_us,
                    present_us,
                    probe_us,
                    readback_lag_us,
                });
                Ok(())
            })?;
            let feed_us = feed_start.elapsed().as_micros() as u64;
            feed_times.push(feed_us);
            let inflight_now = decoder.in_flight();
            trace.push((
                started.elapsed().as_millis() as u64,
                inflight_now,
                (decoded_frames - shown_before) as usize,
                feed_us,
            ));
            shown_before = decoded_frames;
            inflight_peak = inflight_peak.max(inflight_now);
            window.pump();
            if window.closing() {
                break;
            }
        }
        // 接收窗口到此结束：耗时必须在销毁窗口/等收尾之前定格，否则会把窗口销毁时间
        // 算进帧率分母，把真实上屏帧率压低（实测曾因此 48 fps 而非 58 fps）。
        let elapsed = started.elapsed().as_secs_f64().max(0.001);
        let cpu_end_us = process_cpu_us();
        // 响应性判定必须在销毁窗口之前采集（与 S3a/S3b-2 口径一致）。
        let hung = is_hung(window.hwnd());
        let presented = window.presented_frames();
        drop(window);
        // 解码结束后再取一次快照：硬解是否真正生效、每帧解码耗时只有跑完才知道。
        let decoder_info_final = decoder.info();
        let decoder_output = decoder_info_final.output.as_str();
        // 硬件判定按「设备管理器绑定 + DXGI 纹理输出」双条件，而不是枚举命中。
        let decoder_hw = decoder_info_final.hardware;
        println!(
            "[ctrl] decoder_hw={decoder_hw}，output={decoder_output}，h264_profiles={h264_profiles}，{}",
            decoder_info_final.detail
        );
        let (p50, p99) = (percentile(&samples, 0.50), percentile(&samples, 0.99));
        let mismatched = records.iter().filter(|r| r.tcode != r.expected).count();
        let inflight_end = decoder.in_flight();
        let first_frame_latency_us = match (first_feed_at, first_output_at) {
            (Some(first_feed), Some(first_out)) => {
                first_out.duration_since(first_feed).as_micros() as u64
            }
            _ => 0,
        };
        // 进程 CPU（所有核累加）/ 实测时长 → 占单核百分比。
        let cpu_pct = (cpu_end_us.saturating_sub(cpu_start_us)) as f64 / (elapsed * 1e6) * 100.0;
        let dropped_frames = reassembler.dropped();
        let stream_secs = stream_start
            .map(|start| stream_end.duration_since(start).as_secs_f64())
            .unwrap_or(elapsed)
            .max(0.001);
        let decode_fps = frames as f64 / stream_secs;
        // 真实上屏帧率：窗口实际 Present 次数，与解码输入帧率严格区分。
        let present_fps = presented as f64 / stream_secs;
        let batch_dropped = decoded_frames.saturating_sub(presented);
        let decoder_path = decoder_info_final.backend;
        let decoder_detail = decoder_info_final.detail.clone();
        let feed_p50 = percentile(&feed_times, 0.50);
        let feed_p99 = percentile(&feed_times, 0.99);
        let present_p50 = percentile(&present_times, 0.50);
        let present_p99 = percentile(&present_times, 0.99);
        let transport_values: Vec<u64> = records.iter().map(|r| r.transport_us).collect();
        let transport_p50 = percentile(&transport_values, 0.50);
        let transport_p99 = percentile(&transport_values, 0.99);
        let probe_p50 = percentile(&probe_times, 0.50);
        let probe_p99 = percentile(&probe_times, 0.99);
        let readback_lag_median_us = median_i64(records.iter().map(|r| r.readback_lag_us));
        let json = format!(
            "{{\"role\":\"ctrl\",\"seconds\":{:.2},\"frames\":{frames},\"packets\":{packets},\
             \"bytes\":{bytes},\"bitrate_bps\":{:.0},\"samples\":{},\"e2e_p50_us\":{p50},\
             \"e2e_p99_us\":{p99},\"stream_seconds\":{stream_secs:.2},\
             \"decode_fps\":{decode_fps:.2},\"present_fps\":{present_fps:.2},\
             \"presented\":{},\"decoded_frames\":{decoded_frames},\
             \"batch_dropped\":{batch_dropped},\"dropped\":{dropped_frames},\"skipped_stale\":{skipped_stale},\
             \"tcode_mismatch\":{mismatched},\"feed_p50_us\":{feed_p50},\"feed_p99_us\":{feed_p99},\
             \"present_p50_us\":{present_p50},\"present_p99_us\":{present_p99},\"retransmits\":0,\
             \"transport_p50_us\":{transport_p50},\"transport_p99_us\":{transport_p99},\
             \"probe_p50_us\":{probe_p50},\"probe_p99_us\":{probe_p99},\
             \"readback_lag_median_us\":{readback_lag_median_us},\
             \"decoder\":\"{decoder_name}\",\"decoder_path\":\"{decoder_path}\",\"decoder_hw\":{decoder_hw},\"decoder_hw_mft\":{decoder_hw_mft},\
             \"decoder_dxva_aware\":{decoder_dxva},\"decoder_output\":\"{decoder_output}\",\
             \"decoder_detail\":\"{decoder_detail}\",\"first_frame_latency_us\":{first_frame_latency_us},\
             \"first_output_after_feeds\":{first_output_after_feeds},\"cpu_pct_of_core\":{cpu_pct:.1},\
             \"h264_profiles\":{h264_profiles},\"inflight_peak\":{inflight_peak},\
             \"inflight_end\":{inflight_end},\"hung\":{hung}}}",
            elapsed,
            bytes as f64 * 8.0 / elapsed,
            samples.len(),
            presented
        );
        let _ = std::fs::create_dir_all(&options.out); // gate-allow R-C-2: 紧随其后的 std::fs::write(report.json)? 会在目录不可写时返回错误
        let report = Path::new(&options.out).join("report.json");
        std::fs::write(&report, json)?;
        let mut trace_csv =
            std::fs::File::create(Path::new(&options.out).join("decode-trace.csv"))?;
        writeln!(trace_csv, "t_ms,in_flight,outs,feed_us")?;
        for &(t_ms, inflight, outs, feed_us) in &trace {
            writeln!(trace_csv, "{t_ms},{inflight},{outs},{feed_us}")?;
        }
        let mut csv = std::fs::File::create(Path::new(&options.out).join("e2e-samples.csv"))?;
        writeln!(
            csv,
            "sample_index,frame_id,au_ts_us,decoded_ts_us,tcode,expected,now24,e2e_us,feed_to_out_us,transport_us,present_us,probe_us,readback_lag_us"
        )?;
        for (index, record) in records.iter().enumerate() {
            writeln!(
                csv,
                "{index},{},{},{},{},{},{},{},{},{},{},{},{}",
                record.frame_id,
                record.au_ts_us,
                record.decoded_ts_us,
                record.tcode,
                record.expected,
                record.now24,
                record.e2e_us,
                record.feed_to_out_us,
                record.transport_us,
                record.present_us,
                record.probe_us,
                record.readback_lag_us
            )?;
        }
        println!(
            "[ctrl] 完成：解码输入 {frames} 帧 ({decode_fps:.1} fps) / 上屏 {presented} 帧 ({present_fps:.1} fps) / {} 样本 / p50 {p50} us / p99 {p99} us",
            samples.len()
        );
        println!(
            "[ctrl] 分段 p50：传输 {transport_p50} us / 解码+整理 {feed_p50} us / 上屏 {present_p50} us / 回读 {probe_p50} us；回读滞后中位 {readback_lag_median_us} us"
        );
        Ok(())
    }

    /// 判定窗口是否「未响应」。
    ///
    /// # Returns
    /// Windows 判定未响应返回 `true`；查询失败按未响应处理（宁可误报也不放过）。
    fn is_hung(hwnd: windows::Win32::Foundation::HWND) -> bool {
        unsafe { windows::Win32::UI::WindowsAndMessaging::IsHungAppWindow(hwnd).as_bool() }
    }

    /// 线性插值分位。
    fn percentile(values: &[u64], q: f64) -> u64 {
        if values.is_empty() {
            return 0;
        }
        let mut sorted = values.to_vec();
        sorted.sort_unstable();
        let pos = (sorted.len() - 1) as f64 * q;
        let low = pos.floor() as usize;
        let high = pos.ceil() as usize;
        if low == high {
            sorted[low]
        } else {
            let weight = pos - low as f64;
            (sorted[low] as f64 * (1.0 - weight) + sorted[high] as f64 * weight) as u64
        }
    }

    /// 有符号中位数（空集合返回 0）。
    fn median_i64(values: impl Iterator<Item = i64>) -> i64 {
        let mut sorted: Vec<i64> = values.collect();
        if sorted.is_empty() {
            return 0;
        }
        sorted.sort_unstable();
        sorted[sorted.len() / 2]
    }

    /// 桌面型底图（浅灰 + 顶栏）。
    fn build_base() -> Vec<u8> {
        let mut buf = vec![0u8; FRAME_BYTES];
        for pixel in buf.chunks_exact_mut(4) {
            pixel[0] = 240;
            pixel[1] = 240;
            pixel[2] = 240;
            pixel[3] = 255;
        }
        for y in 0..48usize {
            for x in 0..WIDTH as usize {
                let offset = (y * WIDTH as usize + x) * 4;
                buf[offset] = 36;
                buf[offset + 1] = 40;
                buf[offset + 2] = 48;
            }
        }
        buf
    }

    /// 画时间码：24 位机读条码 + 6 位 7 段数码管（毫秒）。
    fn draw_frame(buf: &mut [u8], captured_us: u64, frame_index: u64) {
        let value = captured_us & 0xFF_FFFF;
        for bit in 0..BITS {
            let on = (value >> bit) & 1 == 1;
            let color = if on {
                [255u8, 255u8, 255u8]
            } else {
                [0u8, 0u8, 0u8]
            };
            fill_rect(
                buf,
                STRIPE_X + bit as u32 * CELL_STEP,
                STRIPE_Y,
                CELL,
                CELL,
                color,
            );
        }
        // 人类可读：自启动起的毫秒数，6 位。
        let millis = frame_index * 16_667 / 1000;
        for digit in 0..6u32 {
            let value = (millis / 10u64.pow(5 - digit)) % 10;
            draw_digit(buf, 48 + digit * 64, 200, 44, 72, value as u8);
        }
    }

    /// 填充矩形（RGBA）。
    fn fill_rect(buf: &mut [u8], x: u32, y: u32, w: u32, h: u32, color: [u8; 3]) {
        let y_end = (y + h).min(HEIGHT);
        let x_end = (x + w).min(WIDTH);
        for row in y..y_end {
            for column in x..x_end {
                let offset = (row as usize * WIDTH as usize + column as usize) * 4;
                buf[offset] = color[0];
                buf[offset + 1] = color[1];
                buf[offset + 2] = color[2];
            }
        }
    }

    /// 7 段数码管单字符（仅 0~9）。
    fn draw_digit(buf: &mut [u8], x: u32, y: u32, w: u32, h: u32, digit: u8) {
        // 段位顺序：a(top), b(top-right), c(bottom-right), d(bottom), e(bottom-left),
        //          f(top-left), g(middle)
        const SEGMENTS: [[bool; 7]; 10] = [
            [true, true, true, true, true, true, false],
            [false, true, true, false, false, false, false],
            [true, true, false, true, true, false, true],
            [true, true, true, true, false, false, true],
            [false, true, true, false, false, true, true],
            [true, false, true, true, false, true, true],
            [true, false, true, true, true, true, true],
            [true, true, true, false, false, false, false],
            [true, true, true, true, true, true, true],
            [true, true, true, true, false, true, true],
        ];
        let segments = SEGMENTS[digit as usize % 10];
        let thickness = (w / 6).max(4);
        let color = [0u8, 0u8, 0u8];
        let half = h / 2;
        if segments[0] {
            fill_rect(buf, x, y, w, thickness, color);
        }
        if segments[1] {
            fill_rect(buf, x + w - thickness, y, thickness, half, color);
        }
        if segments[2] {
            fill_rect(buf, x + w - thickness, y + half, thickness, half, color);
        }
        if segments[3] {
            fill_rect(buf, x, y + h - thickness, w, thickness, color);
        }
        if segments[4] {
            fill_rect(buf, x, y + half, thickness, half, color);
        }
        if segments[5] {
            fill_rect(buf, x, y, thickness, half, color);
        }
        if segments[6] {
            fill_rect(buf, x, y + half - thickness / 2, w, thickness, color);
        }
    }
}
