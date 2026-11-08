//! rdc-cli / S3b-3 并行编码压力测试
//!
//! 层级：L4 验证入口（Windows）
//! 用途：用 K 个独立 MF 编码会话并行提交 1080p 帧，测量单帧 `push` 成本与
//!       push→输出时延的 p50/p99，并核对实测码率是否落在目标 ±20%。
//! 调用方：`rdc-cli s3b3 [--instances K] [--frames N] [--bitrate B]`
//!
//! 目的：给 D18（编码 p99 18.262ms > 8ms）一个可复核的对照实验：
//! 单实例基线 vs 双实例并行；达不到就在数据里说清楚，不改门槛。

use std::error::Error;

/// 自检错误类型。
type S3bResult<T> = Result<T, Box<dyn Error + Send + Sync>>;

// 以下判据只被 `windows_impl`（`#[cfg(target_os = "windows")]`）引用；按同一条件
// 裁剪定义，避免非 Windows 构建报 dead_code，同时保留判据数值本身。
/// 判据：单帧提交成本 p99 上限（微秒），与 S3a 相同，用户明确不改。
#[cfg(target_os = "windows")]
const PUSH_P99_LIMIT_US: u64 = 8_000;
/// 判据：实测码率相对目标的最大偏差。
#[cfg(target_os = "windows")]
const BITRATE_TOLERANCE: f64 = 0.20;

/// 命令行参数。
struct Options {
    /// 并行实例数。
    instances: usize,
    /// 每实例提交帧数。
    frames: u32,
    /// 目标码率（bps）。
    bitrate: u32,
}

/// 解析参数。
///
/// # Arguments
/// * `args` - 子命令后剩余参数。
///
/// # Returns
/// 解析后的参数；非法值回退到默认（2 实例 / 300 帧 / 8 Mbps）。
fn parse(args: &mut impl Iterator<Item = String>) -> Options {
    let mut options = Options {
        instances: 2,
        frames: 300,
        bitrate: 8_000_000,
    };
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--instances" => {
                if let Some(value) = args.next().and_then(|v| v.parse::<usize>().ok()) {
                    options.instances = value.clamp(1, 4);
                }
            }
            "--frames" => {
                if let Some(value) = args.next().and_then(|v| v.parse::<u32>().ok()) {
                    options.frames = value.max(1);
                }
            }
            "--bitrate" => {
                if let Some(value) = args.next().and_then(|v| v.parse::<u32>().ok()) {
                    options.bitrate = value.max(1_000_000);
                }
            }
            _ => {}
        }
    }
    options
}

/// 执行 S3b-3 并行编码实测。
///
/// # Arguments
/// * `args` - 子命令参数：`--instances K` / `--frames N` / `--bitrate B`。
///
/// # Returns
/// 判据满足返回 `Ok(())`；否则返回带实测值的错误字符串。
///
/// # Errors
/// 编码器不可用、线程 panic、写日志失败时返回错误。
pub fn run(args: &mut impl Iterator<Item = String>) -> S3bResult<()> {
    let options = parse(args);
    #[cfg(not(target_os = "windows"))]
    {
        let _options = options;
        Err("S3b-3 只实现于 Windows（MF 硬件编码）".into())
    }
    #[cfg(target_os = "windows")]
    {
        windows_impl::run(options)
    }
}

#[cfg(target_os = "windows")]
mod windows_impl {
    use std::io::Write;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use rdc_codec::{create_encoder, CodecKind, VideoChroma, VideoConfig};
    use rdc_runtime::pool::FramePool;

    use super::{Options, BITRATE_TOLERANCE, PUSH_P99_LIMIT_US, S3bResult};

    /// 帧尺寸。
    const WIDTH: u32 = 1920;
    /// 帧尺寸。
    const HEIGHT: u32 = 1080;
    /// 每帧字节数（RGBA）。
    const FRAME_BYTES: usize = (WIDTH as usize) * (HEIGHT as usize) * 4;
    /// 帧间隔（微秒）。
    const FRAME_PERIOD_US: u64 = 16_667;

    /// 单实例结果。
    struct InstanceResult {
        /// 实例序号。
        index: usize,
        /// 编码器名称。
        encoder: String,
        /// 成功提交帧数。
        pushed: u64,
        /// push 失败计数。
        push_failed: u64,
        /// 输出包数。
        packets: u64,
        /// 关键帧数。
        keyframes: u64,
        /// 输出总字节数。
        bytes: u64,
        /// push 成本样本（微秒）。
        push_costs: Vec<u64>,
        /// push→输出时延样本（微秒）。
        latencies: Vec<u64>,
        /// 实测时长（毫秒）。
        elapsed_ms: u64,
    }

    /// 执行实测：依次跑 K=1 与 K=请求实例数两组，给出对照。
    pub(super) fn run(options: Options) -> S3bResult<()> {
        println!(
            "== RDC S3b-3 并行编码实测（{} 帧/实例，目标码率 {:.1} Mbps）==",
            options.frames,
            options.bitrate as f64 / 1_000_000.0
        );
        let log_dir = create_log_dir();
        println!("日志目录             : {}", log_dir.display());
        // 编码器来源必须写进报告：并行编码能否压时延，取决于底层是硬编还是软编。
        let probe = rdc_codec::probe_encoders();
        println!(
            "硬件编码可用         : {}（{} 个候选）",
            probe.hardware_available,
            probe.hardware.len()
        );
        for cap in &probe.hardware {
            println!("  硬编候选           : {}", cap.name);
        }
        if !probe.software.is_empty() {
            println!(
                "  软编候选           : {}",
                probe
                    .software
                    .iter()
                    .map(|c| c.name.as_str())
                    .collect::<Vec<_>>()
                    .join(" / ")
            );
        }
        let mut csv = std::fs::File::create(log_dir.join("instances.csv"))?;
        writeln!(
            csv,
            "instances,index,encoder,pushed,push_failed,packets,keyframes,bytes,\
             push_p50_us,push_p99_us,latency_p50_us,latency_p99_us,elapsed_ms"
        )?;

        // 对照组：单实例基线（复现 S3a 量级）。
        let baseline = run_instances(1, &options)?;
        // 实验组：请求的实例数。
        let parallel = if options.instances > 1 {
            run_instances(options.instances, &options)?
        } else {
            Vec::new()
        };

        let mut report = String::new();
        report.push_str("\n== S3b-3 实测结果 ==\n");
        for (label, results) in [("单实例", &baseline), ("并行", &parallel)] {
            if results.is_empty() {
                continue;
            }
            let aggregate = summarize(results, options.bitrate);
            report.push_str(&format!(
                "{label}（{} 实例）\n\
                 \x20 push 成本 p50/p99 : {} / {} us（门槛 ≤{} us）\n\
                 \x20 输出时延 p50/p99 : {} / {} us\n\
                 \x20 提交 / 输出包    : {} / {}\n\
                 \x20 单会话码率        : {:.2} Mbps（目标 {:.1} Mbps，偏差 {:+.1}%）\n",
                results.len(),
                aggregate.push_p50,
                aggregate.push_p99,
                PUSH_P99_LIMIT_US,
                aggregate.latency_p50,
                aggregate.latency_p99,
                aggregate.pushed,
                aggregate.packets,
                aggregate.bitrate_bps / 1_000_000.0,
                options.bitrate as f64 / 1_000_000.0,
                aggregate.deviation_pct,
            ));
            for result in results {
                writeln!(
                    csv,
                    "{},{},{},{},{},{},{},{},{},{},{},{},{}",
                    results.len(),
                    result.index,
                    result.encoder,
                    result.pushed,
                    result.push_failed,
                    result.packets,
                    result.keyframes,
                    result.bytes,
                    percentile(&result.push_costs, 0.50),
                    percentile(&result.push_costs, 0.99),
                    percentile(&result.latencies, 0.50),
                    percentile(&result.latencies, 0.99),
                    result.elapsed_ms,
                )?;
            }
        }

        // 判据：并行组（K>1）push p99 ≤ 8ms；码率偏差 ≤±20%；无 push 失败。
        let experiment = if parallel.is_empty() { &baseline } else { &parallel };
        let aggregate = summarize(experiment, options.bitrate);
        let latency_ok = aggregate.push_p99 <= PUSH_P99_LIMIT_US;
        let bitrate_ok = aggregate.deviation_pct.abs() <= BITRATE_TOLERANCE * 100.0;
        let push_ok = experiment.iter().all(|r| r.push_failed == 0);
        let passed = latency_ok && bitrate_ok && push_ok;
        report.push_str(&format!(
            "判定：{}（push p99 {} us / 码率偏差 {:+.1}% / push 失败 {}）\n",
            if passed { "PASS" } else { "FAIL" },
            aggregate.push_p99,
            aggregate.deviation_pct,
            experiment.iter().map(|r| r.push_failed).sum::<u64>(),
        ));
        println!("{report}");
        csv.flush()?;

        let json = format!(
            "{{\n\
             \x20 \"phase\": \"S3b-3\",\n\
             \x20 \"target_bitrate_bps\": {},\n\
             \x20 \"baseline\": {{\"instances\": 1, \"push_p99_us\": {}, \"latency_p99_us\": {}, \"push_failed\": {}}},\n\
             \x20 \"experiment\": {{\"instances\": {}, \"push_p99_us\": {}, \"latency_p99_us\": {}, \"push_failed\": {}, \"bitrate_bps\": {:.0}, \"bitrate_deviation_pct\": {:.2}}},\n\
             \x20 \"gate\": {{\"push_p99_limit_us\": {}, \"bitrate_tolerance\": {}, \"latency_ok\": {latency_ok}, \"bitrate_ok\": {bitrate_ok}, \"push_ok\": {push_ok}, \"passed\": {passed}}}\n\
             }}",
            options.bitrate,
            summarize(&baseline, options.bitrate).push_p99,
            summarize(&baseline, options.bitrate).latency_p99,
            baseline.iter().map(|r| r.push_failed).sum::<u64>(),
            experiment.len(),
            aggregate.push_p99,
            aggregate.latency_p99,
            experiment.iter().map(|r| r.push_failed).sum::<u64>(),
            aggregate.bitrate_bps,
            aggregate.deviation_pct,
            PUSH_P99_LIMIT_US,
            BITRATE_TOLERANCE,
        );
        std::fs::write(log_dir.join("report.json"), json)?;

        if passed {
            Ok(())
        } else {
            Err(format!(
                "S3b-3 判据未通过：push p99 {} us（门槛 ≤{} us）、码率偏差 {:+.1}%（门槛 ±{}%）",
                aggregate.push_p99,
                PUSH_P99_LIMIT_US,
                aggregate.deviation_pct,
                BITRATE_TOLERANCE * 100.0
            )
            .into())
        }
    }

    /// 汇总结构。
    struct Summary {
        /// 提交帧数。
        pushed: u64,
        /// 输出包数。
        packets: u64,
        /// push 成本 p50。
        push_p50: u64,
        /// push 成本 p99。
        push_p99: u64,
        /// 输出时延 p50。
        latency_p50: u64,
        /// 输出时延 p99。
        latency_p99: u64,
        /// 聚合码率（bps）。
        bitrate_bps: f64,
        /// 相对目标码率偏差（%）。
        deviation_pct: f64,
    }

    /// 合并多实例样本并计算统计量。
    ///
    /// # Arguments
    /// * `results` - 各实例结果
    /// * `target_bps` - 目标码率（bps），用于计算偏差
    fn summarize(results: &[InstanceResult], target_bps: u32) -> Summary {
        let mut costs: Vec<u64> = Vec::new();
        let mut latencies: Vec<u64> = Vec::new();
        let mut pushed = 0u64;
        let mut packets = 0u64;
        let mut bytes = 0u64;
        let mut elapsed_ms = 0u64;
        for result in results {
            costs.extend_from_slice(&result.push_costs);
            latencies.extend_from_slice(&result.latencies);
            pushed += result.pushed;
            packets += result.packets;
            bytes += result.bytes;
            elapsed_ms = elapsed_ms.max(result.elapsed_ms);
        }
        let seconds = (elapsed_ms as f64 / 1000.0).max(0.001);
        // 码率按「单会话」口径：目标码率是每会话的均值，聚合值会随实例数放大。
        let instances = results.len().max(1) as f64;
        let bitrate_bps = bytes as f64 * 8.0 / seconds / instances;
        let deviation_pct = (bitrate_bps - f64::from(target_bps)) / f64::from(target_bps) * 100.0;
        Summary {
            pushed,
            packets,
            push_p50: percentile(&costs, 0.50),
            push_p99: percentile(&costs, 0.99),
            latency_p50: percentile(&latencies, 0.50),
            latency_p99: percentile(&latencies, 0.99),
            bitrate_bps,
            deviation_pct,
        }
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

    /// 启动 `count` 个实例线程，等待全部返回。
    fn run_instances(count: usize, options: &Options) -> S3bResult<Vec<InstanceResult>> {
        let (sender, receiver) = mpsc::channel();
        for index in 0..count {
            let sender = sender.clone();
            let frames = options.frames;
            let bitrate = options.bitrate;
            std::thread::spawn(move || {
                let result = run_instance(index, frames, bitrate);
                let _ = sender.send(result); // gate-allow R-C-2: 接收端在任一实例超时/报错后已提前返回，主错误随后返回
            });
        }
        drop(sender);
        let mut results = Vec::new();
        for _ in 0..count {
            match receiver.recv_timeout(Duration::from_secs(60)) {
                Ok(Ok(result)) => results.push(result),
                Ok(Err(err)) => return Err(err),
                Err(err) => return Err(format!("等待实例结果超时: {err}").into()),
            }
        }
        results.sort_by_key(|r| r.index);
        Ok(results)
    }

    /// 单个实例：建会话、按 60fps 提交、逐帧计时。
    fn run_instance(index: usize, frames: u32, bitrate: u32) -> S3bResult<InstanceResult> {
        let mut encoder = create_encoder(CodecKind::H264)?;
        encoder.configure(VideoConfig {
            codec: CodecKind::H264,
            width: WIDTH,
            height: HEIGHT,
            fps: 60,
            bitrate_bps: bitrate,
            chroma: VideoChroma::Yuv420,
            bit_depth: 8,
            low_latency: true,
        })?;
        let encoder_name = encoder.backend_name().to_owned();

        let pool = FramePool::new(4, FRAME_BYTES);
        let base = build_base();

        let mut push_costs = Vec::with_capacity(frames as usize);
        let mut latencies = Vec::with_capacity(frames as usize);
        let mut push_times = vec![0u64; frames as usize];
        let mut pushed = 0u64;
        let mut push_failed = 0u64;
        let mut packets = 0u64;
        let mut keyframes = 0u64;
        let mut bytes = 0u64;
        let started = Instant::now();

        for index_frame in 0..frames {
            let pts_ns = u64::from(index_frame) * 1_000_000_000 / 60;
            // 节拍到 60fps：与真实采集节奏一致，避免把「机器最快吞吐」当成时延。
            let target_us = u64::from(index_frame) * FRAME_PERIOD_US;
            while (started.elapsed().as_micros() as u64) < target_us {
                std::thread::sleep(Duration::from_millis(1));
            }
            let Some(mut buffer) = pool.acquire() else {
                push_failed += 1;
                continue;
            };
            buffer.as_mut_slice().copy_from_slice(&base);
            draw_frame(buffer.as_mut_slice(), index_frame);
            let push_abs_us = started.elapsed().as_micros() as u64;
            let push_start = Instant::now();
            let result = encoder.push(buffer, pts_ns, index_frame % 120 == 0);
            let cost_us = push_start.elapsed().as_micros() as u64;
            push_costs.push(cost_us);
            if result.is_err() {
                push_failed += 1;
                continue;
            }
            pushed += 1;
            push_times[index_frame as usize] = push_abs_us;

            while let Some(packet) = encoder.poll_packet() {
                packets += 1;
                bytes += packet.data.len() as u64;
                if packet.is_keyframe {
                    keyframes += 1;
                }
                let frame_index = (packet.pts_ns * 60 / 1_000_000_000) as usize;
                if frame_index < push_times.len() && push_times[frame_index] > 0 {
                    let now_us = started.elapsed().as_micros() as u64;
                    latencies.push(now_us.saturating_sub(push_times[frame_index]));
                }
            }
        }
        for packet in encoder.flush()? {
            packets += 1;
            bytes += packet.data.len() as u64;
            if packet.is_keyframe {
                keyframes += 1;
            }
        }

        Ok(InstanceResult {
            index,
            encoder: encoder_name,
            pushed,
            push_failed,
            packets,
            keyframes,
            bytes,
            push_costs,
            latencies,
            elapsed_ms: started.elapsed().as_millis() as u64,
        })
    }

    /// 合成桌面型基础图案（大片纯色 + 少量矩形），贴近真实桌面采集内容。
    ///
    /// 为什么不用逐像素噪声：噪声会把每一帧都变成高频细节，压测结果反映的是
    /// 「编码器最坏情况」而不是桌面场景，无法与 S3a 的 18ms 基线对照。
    fn build_base() -> Vec<u8> {
        let mut buf = vec![0u8; FRAME_BYTES];
        // 底色：浅灰桌面。
        for pixel in buf.chunks_exact_mut(4) {
            pixel[0] = 244;
            pixel[1] = 244;
            pixel[2] = 244;
            pixel[3] = 255;
        }
        let rects = [
            (0usize, 0usize, 1920usize, 56usize, [40u8, 44u8, 52u8]),
            (80, 140, 760, 520, [255, 255, 255]),
            (880, 140, 960, 320, [250, 250, 252]),
            (880, 500, 960, 400, [30, 32, 36]),
        ];
        for (x0, y0, w, h, color) in rects {
            for y in y0..(y0 + h).min(HEIGHT as usize) {
                for x in x0..(x0 + w).min(WIDTH as usize) {
                    let offset = (y * WIDTH as usize + x) * 4;
                    buf[offset] = color[0];
                    buf[offset + 1] = color[1];
                    buf[offset + 2] = color[2];
                }
            }
        }
        buf
    }

    /// 在基础图案上画一条移动竖条，模拟桌面局部更新（每帧只改约 0.6% 像素）。
    fn draw_frame(buf: &mut [u8], index: u32) {
        let bar_w = 96usize;
        let x0 = (index as usize * 32) % (WIDTH as usize);
        let height = HEIGHT as usize;
        for y in 0..height {
            let end = (x0 + bar_w).min(WIDTH as usize);
            for x in x0..end {
                let offset = (y * WIDTH as usize + x) * 4;
                buf[offset] = 20;
                buf[offset + 1] = 120;
                buf[offset + 2] = 220;
            }
        }
    }

    /// 创建 `logs/s3b3-<epoch 秒>` 日志目录。
    fn create_log_dir() -> PathBuf {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let dir = PathBuf::from("logs").join(format!("s3b3-{secs}"));
        if let Err(error) = std::fs::create_dir_all(&dir) {
            eprintln!(
                "[s3b3] 创建日志目录 {} 失败，诊断输出将缺失: {error}",
                dir.display()
            );
        }
        dir
    }
}
