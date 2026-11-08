//! rdc-cli / S3b-2 解码与上屏自检
//!
//! 层级：L4 验证入口（Windows）
//! 用途：合成 1080p60 图案 → MF 硬件 H.264 编码 → rdc-decode 解码 → D3D11 窗口上屏，
//!       校验「能解、能按时间码出队、能上屏、主线程不未响应」，并产出帧统计。
//! 调用方：`rdc-cli s3b [--frames N]`
//!
//! 线程模型：全部在调用线程内完成（D3D11 交换链必须由创建它的线程 Present），
//! 每帧迭代都泵一次窗口消息，保证 Windows 不会判定「未响应」。

use std::error::Error;

/// 自检错误类型。
type S3bResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

/// 默认帧数（300 帧 ≈ 5 s @60fps）。
const DEFAULT_FRAMES: u32 = 300;
// 以下判据只在 Windows 实现（windows_impl）里使用；非 Windows 走不支持分支，
// 不加 cfg 会在 Linux 上被判 dead_code（-D warnings 下直接失败）。
/// 目标帧率。
#[cfg(target_os = "windows")]
const TARGET_FPS: u32 = 60;
/// 目标码率（bps）。
#[cfg(target_os = "windows")]
const TARGET_BITRATE: u32 = 8_000_000;
/// 单轮消息泵最大间隔门槛（毫秒）：超过即视为主线程被长时间占用。
#[cfg(target_os = "windows")]
const PUMP_GAP_LIMIT_MS: u64 = 100;

/// 解析 `--frames N`，缺省用 [`DEFAULT_FRAMES`]。
///
/// # Arguments
/// * `args` - 子命令后剩余参数。
///
/// # Returns
/// 帧数；非法值回退到默认值。
fn parse_frames(args: &mut impl Iterator<Item = String>) -> u32 {
    let mut frames = DEFAULT_FRAMES;
    while let Some(arg) = args.next() {
        if arg == "--frames" {
            if let Some(value) = args.next() {
                if let Ok(parsed) = value.parse::<u32>() {
                    frames = parsed.max(1);
                }
            }
        }
    }
    frames
}

/// 执行 S3b-2 解码与上屏自检。
///
/// # Arguments
/// * `args` - 子命令参数（支持 `--frames N`）。
///
/// # Returns
/// 全部判据满足返回 `Ok(())`；否则返回带失败原因的字符串错误。
///
/// # Errors
/// 平台不支持、编码器/解码器不可用、窗口创建失败或判据不满足时返回错误。
pub fn run(args: &mut impl Iterator<Item = String>) -> S3bResult<()> {
    let frames = parse_frames(args);
    #[cfg(not(target_os = "windows"))]
    {
        let _frames = frames;
        Err("S3b-2 上屏自检只实现于 Windows".into())
    }
    #[cfg(target_os = "windows")]
    {
        windows_impl::run(frames)
    }
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use std::io::Write;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    use rdc_codec::{create_encoder, extract_sps_pps, CodecKind, VideoChroma, VideoConfig};
    use rdc_decode::{DecodeQueue, DecodeStats, MfH264Decoder, PresentWindow, DEFAULT_WAIT_LIMIT_US};
    use rdc_runtime::pool::FramePool;

    use super::{S3bResult, PUMP_GAP_LIMIT_MS, TARGET_BITRATE, TARGET_FPS};

    /// 帧尺寸。
    const WIDTH: u32 = 1920;
    /// 帧尺寸。
    const HEIGHT: u32 = 1080;
    /// 每帧字节数（RGBA）。
    const FRAME_BYTES: usize = (WIDTH as usize) * (HEIGHT as usize) * 4;
    /// 目标帧间隔（微秒）。
    const FRAME_PERIOD_US: u64 = 16_667;

    /// 执行完整自检。
    pub(super) fn run(frames: u32) -> S3bResult<()> {
        println!("== RDC S3b-2 解码与上屏自检（{frames} 帧，{WIDTH}x{HEIGHT}@{TARGET_FPS}）==");
        let log_dir = create_log_dir();
        println!("日志目录             : {}", log_dir.display());

        let mut writer = std::fs::File::create(log_dir.join("decode-frames.csv"))?;
        writeln!(writer, "frame_index,ts_us,decode_us,queue_depth,present_us,status")?;

        // 1) 呈现窗口与解码器（硬件优先，失败显式回退并在报告里标注）。
        let mut window = PresentWindow::create(WIDTH, HEIGHT, "RDC S3b-2 解码与上屏")?;
        println!("呈现窗口             : 已创建（{}x{} 客户区）", window.client_size().0, window.client_size().1);
        // 与呈现窗口共用 D3D11 设备：解码走 DXVA 硬件路径（D28）。
        let mut decoder = MfH264Decoder::open_with_device(
            window.device(),
            WIDTH,
            HEIGHT,
            TARGET_FPS,
            true,
        )?;
        println!(
            "解码器               : {}（硬件={}）",
            decoder.name(),
            decoder.is_hardware()
        );

        // 2) 编码器：只用于产生真实码流，不作为被测对象。
        let mut encoder = create_encoder(CodecKind::H264)?;
        encoder.configure(VideoConfig {
            codec: CodecKind::H264,
            width: WIDTH,
            height: HEIGHT,
            fps: TARGET_FPS,
            bitrate_bps: TARGET_BITRATE,
            chroma: VideoChroma::Yuv420,
            bit_depth: 8,
            low_latency: true,
        })?;

        let pool = FramePool::new(4, FRAME_BYTES);
        let mut base = vec![0u8; FRAME_BYTES];
        fill_base(&mut base);

        let mut queue = DecodeQueue::new(4, DEFAULT_WAIT_LIMIT_US);
        let mut stats = DecodeStats::new();
        let mut decoded = 0u64;
        let mut presented = 0u64;
        let mut max_pump_gap = Duration::ZERO;
        let mut last_pump = Instant::now();
        let started = Instant::now();

        // 3) 逐帧：合成 → 编码 → 解码 → 入队 → 出队上屏 → 泵消息 → 节流。
        for index in 0..frames {
            let pts_ns = u64::from(index) * 1_000_000_000 / u64::from(TARGET_FPS);
            let mut buffer = pool
                .acquire()
                .ok_or("帧池已空：编码器未及时归还缓冲")?;
            buffer
                .as_mut_slice()
                .copy_from_slice(&base);
            draw_frame(buffer.as_mut_slice(), index);
            encoder.push(buffer, pts_ns, index % 60 == 0)?;

            // 编码器是异步 MFT：每帧都排空一次输出，但不忙等。
            while let Some(packet) = encoder.poll_packet() {
                let mut annexb = Vec::with_capacity(packet.data.len() + 64);
                if packet.is_keyframe {
                    let header = encoder.sequence_header();
                    let header = if header.is_empty() {
                        extract_sps_pps(&packet.data)
                    } else {
                        header.to_vec()
                    };
                    annexb.extend_from_slice(&header);
                }
                annexb.extend_from_slice(&packet.data);

                let feed_start = Instant::now();
                let decoded_frames = decoder.feed(&annexb, packet.pts_ns / 1_000)?;
                let cost_us = feed_start.elapsed().as_micros() as u64;
                let per_frame = cost_us / (decoded_frames.len().max(1) as u64);
                for frame in decoded_frames {
                    stats.record_decode(per_frame);
                    decoded += 1;
                    let ts_us = frame.ts_us;
                    let accepted = queue.push(ts_us, frame.data);
                    // 逐帧记录入队结果，用于解释「解码数 - 呈现数」的缺口。
                    writeln!(
                        writer,
                        "{index},{ts_us},{per_frame},{},{},{}",
                        queue.depth(),
                        0,
                        if accepted { "queued" } else { "drop" }
                    )?;
                }
                // 每个访问单元之后立刻出队上屏，避免队列在编码器突发时溢出丢帧。
                let now_us = started.elapsed().as_micros() as u64;
                presented += drain_and_present(&mut queue, &mut window, &mut stats, &mut writer, now_us)?;
            }

            // 按时间码出队并上屏；队列空时也算一次泵。
            // 注意：此处不调用 flush_stale——自检用流的 pts 是「流内时间」，
            // 与墙钟存在追赶差；迟到丢弃策略由 DecodeQueue 单测覆盖。
            let now_us = started.elapsed().as_micros() as u64;
            stats.observe_depth(queue.depth());
            presented += drain_and_present(&mut queue, &mut window, &mut stats, &mut writer, now_us)?;

            window.pump();
            let gap = last_pump.elapsed();
            last_pump = Instant::now();
            max_pump_gap = max_pump_gap.max(gap);
            if window.closing() {
                return Err("窗口被关闭，自检中止".into());
            }

            // 节流到 60fps：等待期间持续泵消息，避免窗口未响应。
            let target_us = u64::from(index + 1) * FRAME_PERIOD_US;
            while (started.elapsed().as_micros() as u64) < target_us {
                window.pump();
                std::thread::sleep(Duration::from_millis(1));
            }
        }

        // 4) 冲刷：编码器与解码器各滞留 1~2 帧。
        let tail = encoder.flush()?;
        for packet in tail {
            let mut annexb = Vec::with_capacity(packet.data.len() + 64);
            if packet.is_keyframe {
                let header = encoder.sequence_header();
                let header = if header.is_empty() {
                    extract_sps_pps(&packet.data)
                } else {
                    header.to_vec()
                };
                annexb.extend_from_slice(&header);
            }
            annexb.extend_from_slice(&packet.data);
            for frame in decoder.feed(&annexb, packet.pts_ns / 1_000)? {
                stats.record_decode(1);
                decoded += 1;
                queue.push(frame.ts_us, frame.data);
            }
        }
        for frame in decoder.flush()? {
            stats.record_decode(1);
            decoded += 1;
            queue.push(frame.ts_us, frame.data);
        }
        while let Some((ts_us, data)) = queue.pop_ready(u64::MAX, 0) {
            let present_start = Instant::now();
            window.present_nv12(&data, WIDTH)?;
            let present_us = present_start.elapsed().as_micros() as u64;
            stats.record_present(present_us);
            presented += 1;
            writeln!(writer, "tail,{ts_us},0,{},{present_us},presented", queue.depth())?;
        }
        window.pump();

        // 5) 响应性判定必须在销毁窗口之前采集。
        let hung = is_hung(window.hwnd());
        let elapsed = started.elapsed();
        let snapshot = stats.snapshot();
        let hung_ok = !hung;
        let pump_ok = max_pump_gap.as_millis() as u64 <= PUMP_GAP_LIMIT_MS;
        let depth_ok = snapshot.max_depth <= 4;
        let deliver_ok = decoded > 0 && presented * 100 >= decoded * 95;
        let hardware_ok = decoder.is_hardware();
        let stats_ok = snapshot.decode_p50_us > 0
            && snapshot.decode_p99_us > 0
            && snapshot.present_p99_us > 0;
        // 帧数缺口必须可解释：分别统计「迟到/重复时间码」与「队列溢出」丢弃。
        let dropped_late = queue.dropped_late();
        let dropped_overflow = queue.dropped_overflow();
        let passed = hung_ok && pump_ok && depth_ok && deliver_ok && hardware_ok && stats_ok;

        let report = format!(
            "\n== S3b-2 实测结果 ==\n\
             解码器               : {}（硬件={}，DXVA 能力={}）\n\
             呈现帧数 / 解码帧数  : {} / {}\n\
             实测时长             : {:.2} s（{:.2} fps）\n\
             解码耗时 p50 / p99    : {} / {} us\n\
             呈现延迟 p50 / p99    : {} / {} us\n\
             队列最大深度         : {}（门槛 ≤4）\n\
             入队丢弃 迟到/溢满   : {} / {}\n\
             消息泵最大间隔       : {} ms（门槛 <{} ms）\n\
             窗口未响应           : {}\n\
             判定                 : {}",
            decoder.name(),
            decoder.is_hardware(),
            decoder.dxva_aware(),
            presented,
            decoded,
            elapsed.as_secs_f64(),
            presented as f64 / elapsed.as_secs_f64().max(0.001),
            snapshot.decode_p50_us,
            snapshot.decode_p99_us,
            snapshot.present_p50_us,
            snapshot.present_p99_us,
            snapshot.max_depth,
            dropped_late,
            dropped_overflow,
            max_pump_gap.as_millis(),
            PUMP_GAP_LIMIT_MS,
            if hung { "是" } else { "否" },
            if passed { "PASS" } else { "FAIL" },
        );
        println!("{report}");

        let json = format!(
            "{{\n\
             \x20 \"phase\": \"S3b-2\",\n\
             \x20 \"decoder\": {{\"name\": \"{}\", \"hardware\": {hardware_ok}}},\n\
             \x20 \"frames\": {{\"decoded\": {decoded}, \"presented\": {presented}, \"elapsed_ms\": {}}},\n\
             \x20 \"decode_us\": {{\"p50\": {}, \"p99\": {}}},\n\
             \x20 \"present_us\": {{\"p50\": {}, \"p99\": {}}},\n\
             \x20 \"queue_max_depth\": {},\n\
             \x20 \"dropped_late\": {},\n\
             \x20 \"dropped_overflow\": {},\n\
             \x20 \"pump_max_gap_ms\": {},\n\
             \x20 \"hung\": {hung},\n\
             \x20 \"gate\": {{\"hardware_ok\": {hardware_ok}, \"deliver_ok\": {deliver_ok}, \"stats_ok\": {stats_ok}, \"depth_ok\": {depth_ok}, \"pump_ok\": {pump_ok}, \"hung_ok\": {hung_ok}, \"passed\": {passed}}}\n\
             }}",
            decoder.name(),
            elapsed.as_millis(),
            snapshot.decode_p50_us,
            snapshot.decode_p99_us,
            snapshot.present_p50_us,
            snapshot.present_p99_us,
            snapshot.max_depth,
            dropped_late,
            dropped_overflow,
            max_pump_gap.as_millis(),
        );
        std::fs::write(log_dir.join("report.json"), json)?;
        writer.flush()?;
        // 窗口在 Drop 时销毁；此处显式销毁以便退出前即可观察窗口消失。
        drop(window);

        if passed {
            Ok(())
        } else {
            Err(format!(
                "S3b-2 判据未通过：硬件={hardware_ok} 交付={deliver_ok} 统计={stats_ok} \
                 队列深度={depth_ok}({}) 泵间隔={pump_ok}({}ms) 未响应={hung_ok}",
                snapshot.max_depth,
                max_pump_gap.as_millis()
            )
            .into())
        }
    }

    /// 把队列里到点的帧全部出队并上屏，返回本批呈现帧数。
    ///
    /// # Arguments
    /// * `queue` - 解码帧队列（按时间码排序）
    /// * `window` - 呈现窗口
    /// * `stats` - 统计采样器
    /// * `writer` - 逐帧 CSV
    /// * `now_us` - 当前单调时钟（微秒）
    fn drain_and_present(
        queue: &mut DecodeQueue,
        window: &mut PresentWindow,
        stats: &mut DecodeStats,
        writer: &mut std::fs::File,
        now_us: u64,
    ) -> S3bResult<u64> {
        let mut count = 0u64;
        while let Some((ts_us, data)) = queue.pop_ready(now_us, 0) {
            let present_start = Instant::now();
            window.present_nv12(&data, WIDTH)?;
            let present_us = present_start.elapsed().as_micros() as u64;
            stats.record_present(present_us);
            count += 1;
            writeln!(writer, "batch,{ts_us},0,{},{present_us},presented", queue.depth())?;
        }
        Ok(count)
    }

    /// 创建本次自检日志目录 `logs/s3b2-<epoch 秒>`。
    fn create_log_dir() -> PathBuf {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let dir = PathBuf::from("logs").join(format!("s3b2-{secs}"));
        if let Err(error) = std::fs::create_dir_all(&dir) {
            eprintln!(
                "[s3b] 创建日志目录 {} 失败，诊断输出将缺失: {error}",
                dir.display()
            );
        }
        dir
    }

    /// 填充一次基础图案（横向渐变 + 纵向明暗），后续每帧只在其上画移动元素。
    fn fill_base(buf: &mut [u8]) {
        for y in 0..HEIGHT as usize {
            for x in 0..WIDTH as usize {
                let offset = (y * WIDTH as usize + x) * 4;
                buf[offset] = (x * 255 / WIDTH as usize) as u8;
                buf[offset + 1] = (y * 255 / HEIGHT as usize) as u8;
                buf[offset + 2] = ((x + y) % 256) as u8;
                buf[offset + 3] = 255;
            }
        }
    }

    /// 在基础图案上画移动竖条与帧号色块（保证每帧都有明显变化，编码器不会跳过）。
    fn draw_frame(buf: &mut [u8], index: u32) {
        let bar_w = 64usize;
        let x0 = (index as usize * 24) % (WIDTH as usize + bar_w);
        let height = HEIGHT as usize;
        for y in 0..height {
            let start = x0.min(WIDTH as usize);
            let end = (x0 + bar_w).min(WIDTH as usize);
            for x in start..end {
                let offset = (y * WIDTH as usize + x) * 4;
                buf[offset] = 255;
                buf[offset + 1] = 255;
                buf[offset + 2] = 255;
            }
        }
        // 帧号色块：左上角 8x8 网格，按二进制显示 index。
        let cell = 16usize;
        let origin_x = 16usize;
        let origin_y = 16usize;
        for bit in 0..16u32 {
            let on = (index >> bit) & 1 == 1;
            if !on {
                continue;
            }
            let gx = (bit % 4) as usize;
            let gy = (bit / 4) as usize;
            for dy in 0..cell {
                for dx in 0..cell {
                    let x = origin_x + gx * cell + dx;
                    let y = origin_y + gy * cell + dy;
                    if x < WIDTH as usize && y < height {
                        let offset = (y * WIDTH as usize + x) * 4;
                        buf[offset] = 0;
                        buf[offset + 1] = 255;
                        buf[offset + 2] = 0;
                    }
                }
            }
        }
    }

    /// 判定窗口是否「未响应」。
    ///
    /// # Returns
    /// Windows 判定未响应返回 `true`；查询失败按未响应处理（宁可误报也不放过）。
    fn is_hung(hwnd: windows::Win32::Foundation::HWND) -> bool {
        unsafe { windows::Win32::UI::WindowsAndMessaging::IsHungAppWindow(hwnd).as_bool() }
    }
}
