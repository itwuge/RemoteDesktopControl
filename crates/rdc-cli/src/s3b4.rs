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
        _ => ("", collected),
    };
    let options = parse(&mut rest.into_iter());
    #[cfg(not(target_os = "windows"))]
    {
        let _ = (role, options);
        return Err("S3b-4 只实现于 Windows".into());
    }
    #[cfg(target_os = "windows")]
    {
        match role {
            "host" => windows_impl::run_host(&options),
            "ctrl" => windows_impl::run_ctrl(&options),
            _ => windows_impl::run_driver(&options),
        }
    }
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use std::io::Write;
    use std::net::SocketAddr;
    use std::path::Path;
    use std::time::{Duration, Instant};

    use rdc_codec::{create_encoder, CodecKind, VideoChroma, VideoConfig};
    use rdc_decode::{MfH264Decoder, PresentWindow};
    use rdc_protocol::{PacketHeader, StreamKind};
    use rdc_runtime::pool::FramePool;
    use rdc_transport::{FragmentHeader, Pacer, Reassembler, UdpTransport};
    use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

    use super::{Options, CTRL_PORT, HOST_PORT, S3bResult, TARGET_BITRATE};

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
    }

    /// 本机单调时钟（微秒）：QueryPerformanceCounter 同机跨进程同源。
    fn qpc_us() -> u64 {
        let mut counter = 0i64;
        let mut frequency = 0i64;
        unsafe {
            let _ = QueryPerformanceCounter(&mut counter);
            let _ = QueryPerformanceFrequency(&mut frequency);
        }
        if frequency <= 0 {
            return 0;
        }
        (counter as u128 * 1_000_000 / frequency as u128) as u64
    }

    /// 驱动：拉起 host 与 ctrl 两个子进程，各自落日志。
    pub(super) fn run_driver(options: &Options) -> S3bResult<()> {
        let _ = std::fs::create_dir_all(&options.out);
        let exe = std::env::current_exe()?;
        println!("== RDC S3b-4 端到端实测（UDP 回环，1080p，{} s）==", options.secs);
        println!("日志目录             : {}", options.out);
        let host = std::process::Command::new(&exe)
            .args(["s3b4", "host", "--secs", &options.secs.to_string(), "--out", &options.out])
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
            encoder.push(buffer, frames * 1_000_000_000 / 60, frames % 120 == 0)?;
            frames += 1;

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
                bytes += send_frame(
                    &udp,
                    peer,
                    &annexb,
                    &mut seq,
                    &mut frame_id,
                    &mut pacer,
                    captured_us,
                    &mut packets,
                )?;
            }
        }
        // 收尾：把编码器缓存吐干净。
        for packet in encoder.flush()? {
            bytes += send_frame(
                &udp,
                peer,
                &packet.data,
                &mut seq,
                &mut frame_id,
                &mut pacer,
                qpc_us(),
                &mut packets,
            )?;
        }
        let elapsed = started.elapsed().as_secs_f64().max(0.001);
        let json = format!(
            "{{\"role\":\"host\",\"seconds\":{:.2},\"frames\":{frames},\"packets\":{packets},\
             \"bytes\":{bytes},\"bitrate_bps\":{:.0}}}",
            elapsed,
            bytes as f64 * 8.0 / elapsed
        );
        let _ = std::fs::create_dir_all(&options.out);
        std::fs::write(Path::new(&options.out).join("host.json"), json)?;
        println!(
            "[host] 完成：{frames} 帧 / {packets} 包 / {:.1} Mbps",
            bytes as f64 * 8.0 / elapsed / 1_000_000.0
        );
        Ok(())
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
            let delay = pacer.poll_delay_for(payload.len() + 32, Instant::now());
            if delay > Duration::ZERO {
                std::thread::sleep(delay.min(Duration::from_millis(4)));
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

        // 枚举证据 + 驱动侧 profile 证据（D28 需可复核）。
        let (hardware_names, all_names) = rdc_decode::decoder_names();
        println!("[ctrl] 硬件解码 MFT: {hardware_names:?}");
        println!("[ctrl] 全部解码 MFT: {all_names:?}");
        let h264_profiles = rdc_decode::h264_decoder_profile_count(window.device());
        println!("[ctrl] 适配器 H.264 解码 profile 数: {h264_profiles}");
        // 解码器与呈现窗口共用同一 D3D11 设备：硬解输出直接是 GPU 纹理（零拷贝）。
        let mut decoder = MfH264Decoder::open_with_device(window.device(), WIDTH, HEIGHT, 60, true)?;
        let decoder_name = decoder.name().to_owned();
        let decoder_dxva = decoder.dxva_aware();
        let decoder_hw_mft = decoder.uses_hardware_mft();
        println!(
            "[ctrl] 解码器 {decoder_name}，dxva_aware={decoder_dxva}，独立硬件 MFT={decoder_hw_mft}"
        );
        let mut reassembler = Reassembler::new(4);
        let mut samples: Vec<u64> = Vec::new();
        let mut records: Vec<SampleRecord> = Vec::new();
        let mut feed_times: Vec<u64> = Vec::new();
        let mut present_times: Vec<u64> = Vec::new();
        let mut skipped_stale = 0u64;
        let mut inflight_peak = 0u64;
        let mut dropped_latest = 0u64;
        let mut trace: Vec<(u64, u64, usize, u64)> = Vec::new();
        let mut packets = 0u64;
        let mut bytes = 0u64;
        let mut frames = 0u64;
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
            let now_us = qpc_us();
            // 只丢「到达即已过期」的访问单元（编码/传输堵住时会触发）；
            // 本轮不实现丢帧追赶，解码器积压会如实反映在端到端延迟里。
            if now_us.saturating_sub(header.ts_us) > STALE_US {
                skipped_stale += 1;
                continue;
            }
            let feed_start = Instant::now();
            let decoded = decoder.feed(&annexb, header.ts_us)?;
            let feed_us = feed_start.elapsed().as_micros() as u64;
            feed_times.push(feed_us);
            let inflight_now = decoder.in_flight();
            trace.push((
                started.elapsed().as_millis() as u64,
                inflight_now,
                decoded.len(),
                feed_us,
            ));
            inflight_peak = inflight_peak.max(inflight_now);
            // 解码后 latest-frame-wins：同一批里只上屏最新帧，旧帧立即归还样本池。
            if decoded.len() > 1 {
                dropped_latest += (decoded.len() - 1) as u64;
            }
            if let Some(frame) = decoded.last() {
                let present_start = Instant::now();
                if let Some(gpu) = frame.texture.as_ref() {
                    window.present_texture(&gpu.texture, gpu.array_slice)?;
                } else {
                    window.present_nv12(&frame.data, frame.stride)?;
                }
                let probe = window.take_probe_samples();
                present_times.push(present_start.elapsed().as_micros() as u64);
                if probe.len() != BITS {
                    continue;
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
                records.push(SampleRecord {
                    frame_id: header.frame_id,
                    au_ts_us: header.ts_us,
                    decoded_ts_us: frame.ts_us,
                    tcode: value,
                    // 期望值取该解码帧自身时间码：对不上说明回读的不是这一帧。
                    expected: frame.ts_us & 0xFF_FFFF,
                    now24,
                    e2e_us: e2e,
                });
            }
            window.pump();
            if window.closing() {
                break;
            }
        }
        // 响应性判定必须在销毁窗口之前采集（与 S3a/S3b-2 口径一致）。
        let hung = is_hung(window.hwnd());
        let presented = window.presented_frames();
        drop(window);

        let elapsed = started.elapsed().as_secs_f64().max(0.001);
        let decoder_output = decoder.output_kind().as_str();
        // 硬件判定按「设备管理器绑定 + DXGI 纹理输出」双条件，而不是枚举命中。
        let decoder_hw = decoder.hardware_active();
        println!(
            "[ctrl] decoder_hw={decoder_hw}，output={decoder_output}，h264_profiles={h264_profiles}"
        );
        let (p50, p99) = (percentile(&samples, 0.50), percentile(&samples, 0.99));
        let mismatched = records.iter().filter(|r| r.tcode != r.expected).count();
        let inflight_end = decoder.in_flight();
        let dropped_frames = reassembler.dropped();
        let fps = frames as f64 / elapsed;
        let feed_p50 = percentile(&feed_times, 0.50);
        let feed_p99 = percentile(&feed_times, 0.99);
        let present_p50 = percentile(&present_times, 0.50);
        let present_p99 = percentile(&present_times, 0.99);
        let json = format!(
            "{{\"role\":\"ctrl\",\"seconds\":{:.2},\"frames\":{frames},\"packets\":{packets},\
             \"bytes\":{bytes},\"bitrate_bps\":{:.0},\"samples\":{},\"e2e_p50_us\":{p50},\
             \"e2e_p99_us\":{p99},\"fps\":{fps:.2},\"presented\":{},\"dropped\":{dropped_frames},\"skipped_stale\":{skipped_stale},\
             \"tcode_mismatch\":{mismatched},\"feed_p50_us\":{feed_p50},\"feed_p99_us\":{feed_p99},\
             \"present_p50_us\":{present_p50},\"present_p99_us\":{present_p99},\"retransmits\":0,\
             \"decoder\":\"{decoder_name}\",\"decoder_hw\":{decoder_hw},\"decoder_hw_mft\":{decoder_hw_mft},\
             \"decoder_dxva_aware\":{decoder_dxva},\"decoder_output\":\"{decoder_output}\",\
             \"h264_profiles\":{h264_profiles},\"inflight_peak\":{inflight_peak},\
             \"inflight_end\":{inflight_end},\"dropped_latest\":{dropped_latest},\"hung\":{hung}}}",
            elapsed,
            bytes as f64 * 8.0 / elapsed,
            samples.len(),
            presented
        );
        let _ = std::fs::create_dir_all(&options.out);
        let report = Path::new(&options.out).join("report.json");
        std::fs::write(&report, json)?;
        let mut trace_csv = std::fs::File::create(Path::new(&options.out).join("decode-trace.csv"))?;
        writeln!(trace_csv, "t_ms,in_flight,outs,feed_us")?;
        for &(t_ms, inflight, outs, feed_us) in &trace {
            writeln!(trace_csv, "{t_ms},{inflight},{outs},{feed_us}")?;
        }
        let mut csv = std::fs::File::create(Path::new(&options.out).join("e2e-samples.csv"))?;
        writeln!(
            csv,
            "sample_index,frame_id,au_ts_us,decoded_ts_us,tcode,expected,now24,e2e_us"
        )?;
        for (index, record) in records.iter().enumerate() {
            writeln!(
                csv,
                "{index},{},{},{},{},{},{},{}",
                record.frame_id,
                record.au_ts_us,
                record.decoded_ts_us,
                record.tcode,
                record.expected,
                record.now24,
                record.e2e_us
            )?;
        }
        println!(
            "[ctrl] 完成：{frames} 帧 / {} 样本 / p50 {p50} us / p99 {p99} us / {fps:.1} fps",
            samples.len()
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
            let color = if on { [255u8, 255u8, 255u8] } else { [0u8, 0u8, 0u8] };
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
            draw_digit(
                buf,
                48 + digit * 64,
                200,
                44,
                72,
                value as u8,
            );
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
