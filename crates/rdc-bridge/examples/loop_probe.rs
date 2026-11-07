//! loop_probe · 本机回环链路无头探针（S8-3 / E5）
//!
//! 直接调用 `rdc_bridge::api` 的同一套控制页接口（与 Flutter 客户端共用），
//! 每秒输出一行 CSV，便于记录 fps / 延迟 p50·p99 / 码率；不依赖桌面壳。
//!
//! 运行（在 VM 上）：
//! `DISPLAY=:0 cargo run -p rdc-bridge --release --example loop_probe -- 30`
//!
//! 输出列：`sec,fps,latency_p50_ms,latency_p99_ms,bitrate_kbps,decoded,dropped,frame_bytes,latency_last_ms,frame_hash`

use std::thread::sleep;
use std::time::Duration;

use rdc_bridge::api;

/// 轻量帧内容指纹（抽样 FNV-1a，用于判断画面是否在变化）。
fn frame_hash(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    let mut index = 0usize;
    while index < bytes.len() {
        hash ^= u64::from(bytes[index]);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        index += 97;
    }
    hash
}

fn main() {
    let seconds: u64 = std::env::args()
        .nth(1)
        .and_then(|value| value.parse().ok())
        .unwrap_or(30);

    let status = match api::control_start() {
        Ok(status) => status,
        Err(error) => {
            eprintln!("START_FAIL {error}");
            std::process::exit(2);
        }
    };
    println!(
        "# START backend={} encoder={} decoder={} path={} {}x{}",
        status.backend, status.encoder, status.decoder, status.texture_path, status.width, status.height
    );
    println!(
        "sec,fps,latency_p50_ms,latency_p99_ms,bitrate_kbps,decoded,dropped,frame_bytes,latency_last_ms,frame_hash"
    );

    let mut hashes: Vec<u64> = Vec::new();
    for second in 1..=seconds {
        sleep(Duration::from_millis(1000));
        let snapshot = match api::control_status() {
            Ok(snapshot) => snapshot,
            Err(error) => {
                eprintln!("STATUS_FAIL {error}");
                break;
            }
        };
        let frame = api::control_frame().ok().flatten();
        let (frame_bytes, hash, last_ms) = match &frame {
            Some(frame) => (
                frame.bytes.len(),
                frame_hash(&frame.bytes),
                frame.latency_us as f64 / 1000.0,
            ),
            None => (0, 0, 0.0),
        };
        if hash != 0 {
            hashes.push(hash);
        }
        println!(
            "{},{:.2},{:.2},{:.2},{},{},{},{},{:.2},{}",
            second,
            snapshot.fps,
            snapshot.latency_p50_us as f64 / 1000.0,
            snapshot.latency_p99_us as f64 / 1000.0,
            snapshot.bitrate_kbps,
            snapshot.decoded_frames,
            snapshot.dropped_frames,
            frame_bytes,
            last_ms,
            hash
        );
        if let Some(error) = snapshot.last_error.as_deref() {
            if !error.is_empty() {
                eprintln!("STATUS_ERROR {error}");
                break;
            }
        }
    }

    let unique = {
        let mut sorted = hashes.clone();
        sorted.sort_unstable();
        sorted.dedup();
        sorted.len()
    };
    println!("# UNIQUE_FRAMES {unique} of {}", hashes.len());

    match api::control_stop() {
        Ok(snapshot) => println!(
            "# STOP captured={} encoded={} decoded={} dropped={} bytes={} error={}",
            snapshot.captured_frames,
            snapshot.encoded_frames,
            snapshot.decoded_frames,
            snapshot.dropped_frames,
            snapshot.encoded_bytes,
            snapshot.last_error.as_deref().unwrap_or("-")
        ),
        Err(error) => println!("# STOP_FAIL {error}"),
    }
}
