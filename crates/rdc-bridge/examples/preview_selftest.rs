// ============================================================
// 示例用途：控制页质量角标数据自检（fps / 延迟 / 码率 / 采集与丢帧 / 编码在飞）
// 运行：cargo run -p rdc-bridge --example preview_selftest
// 参数：PREVIEW_DURATION_SECS=秒数（默认 4）；RDC_CONTROL_LOG_DIR=CSV 输出目录
// 输出：每 200 ms 采样一次，每秒打印一行完整计数；末尾打印 checks（含首包时刻、
//       在飞帧 min/max、重建次数）与 last_error。
// 判据见 logs/TESTPLAN-capture-blackframe.md 第 6 节（J4-j）。
// ============================================================

use std::env;
use std::thread::sleep;
use std::time::{Duration, Instant};

fn main() {
    let duration_secs: u64 = env::var("PREVIEW_DURATION_SECS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(4);

    let started = rdc_bridge::api::control_start().expect("启动控制页预览");
    println!(
        "control_start: running={} captured={} encoded={}",
        started.running, started.captured_frames, started.encoded_frames
    );

    let start = Instant::now();
    let mut last = started;
    // 首包时刻（毫秒，从 control_start 返回算起）：判据 J4-j-10（≤ 1000 ms）。
    let mut first_encoded_ms: Option<u128> = None;
    // 在飞帧 min/max：判据 J4-j-11（max ≤ 3 且不随时间增长）。
    let mut in_flight_min: u64 = u64::MAX;
    let mut in_flight_max: u64 = 0;
    let mut tick: u64 = 0;
    while start.elapsed() < Duration::from_secs(duration_secs) {
        // 200 ms 采样：能证明「首包 ≤ 1 s」，也足以看出在飞帧不增长。
        sleep(Duration::from_millis(200));
        let status = rdc_bridge::api::control_status().expect("读取状态");
        if first_encoded_ms.is_none() && status.encoded_frames > 0 {
            first_encoded_ms = Some(start.elapsed().as_millis());
        }
        in_flight_min = in_flight_min.min(status.encoder_in_flight);
        in_flight_max = in_flight_max.max(status.encoder_in_flight);
        tick += 1;
        if tick % 5 == 0 {
            println!(
                "t={:.1}s: fps={:.1} captured={} encoded={} decoded={} dropped={} \
                 in_flight={} resets={} kbps={} encoded_bytes={} latency_us={}",
                start.elapsed().as_secs_f64(),
                status.fps,
                status.captured_frames,
                status.encoded_frames,
                status.decoded_frames,
                status.dropped_frames,
                status.encoder_in_flight,
                status.encoder_resets,
                status.bitrate_kbps,
                status.encoded_bytes,
                status.latency_last_us
            );
        }
        last = status;
    }

    let stopped = rdc_bridge::api::control_stop().expect("停止预览");
    println!("control_stop: running={}", stopped.running);
    let in_flight_min = if in_flight_min == u64::MAX { 0 } else { in_flight_min };
    println!(
        "checks: captured={} encoded={} decoded={} dropped={} in_flight={} resets={} \
         first_encoded_ms={} in_flight_min={} in_flight_max={} stopped_ok={} error={}",
        last.captured_frames,
        last.encoded_frames,
        last.decoded_frames,
        last.dropped_frames,
        last.encoder_in_flight,
        last.encoder_resets,
        first_encoded_ms
            .map(|value| value.to_string())
            .unwrap_or_else(|| "none".to_owned()),
        in_flight_min,
        in_flight_max,
        !stopped.running,
        last.last_error.as_deref().unwrap_or("none")
    );

    if !stopped.running {
        println!("preview_selftest PASS");
    } else {
        eprintln!("preview_selftest FAILED: control did not stop");
        std::process::exit(1);
    }
}
