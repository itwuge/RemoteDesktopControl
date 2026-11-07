//! 桥接层 Linux 接口自检（S8-3 E1/E8 证据）
//!
//! 运行：`cargo run -p rdc-bridge --release --example linux_api_check`
//!
//! 逐项调用设备 / 设置 / 会话 / 采集编码注入 / 屏幕墙接口，把返回值打印成 KEY=VALUE，
//! 用于证明 Linux 分支「接口都在、且行为明确」，而不是靠 cfg 把功能删空。

use std::time::{Duration, Instant};

use rdc_bridge::api;

fn main() {
    println!("===PLATFORM===");
    println!("platform_name={}", api::platform_name());
    println!("bridge_status={}", api::bridge_status());
    for line in api::doctor_report() {
        println!("doctor: {line}");
    }

    println!("===DEVICES===");
    match api::devices_load() {
        Ok(list) => println!(
            "devices_load=ok count={} path={}",
            list.devices.len(),
            list.path
        ),
        Err(error) => println!("devices_load=err {error}"),
    }

    println!("===SETTINGS===");
    match api::settings_load() {
        Ok(settings) => {
            println!(
                "settings_load=ok relay={} size={}x{} fps={} bitrate={}Mbps",
                settings.relay_address,
                settings.video_width,
                settings.video_height,
                settings.video_fps,
                settings.video_bitrate_mbps
            );
            match api::settings_save(settings.clone()) {
                Ok(saved) => println!(
                    "settings_save=ok relay={} policy={}",
                    saved.relay_address, saved.connection_policy
                ),
                Err(error) => println!("settings_save=err {error}"),
            }
        }
        Err(error) => println!("settings_load=err {error}"),
    }

    println!("===SESSION===");
    match api::session_status() {
        Ok(status) => println!(
            "session_status=ok running={} role={} local_port={}",
            status.running, status.role, status.local_port
        ),
        Err(error) => println!("session_status=err {error}"),
    }

    println!("===WALL (Linux 应明确不支持)===");
    match api::wall_start() {
        Ok(status) => println!("wall_start=ok running={}", status.running),
        Err(error) => println!("wall_start=err {error}"),
    }
    match api::wall_latest() {
        Ok(frame) => println!("wall_latest=ok present={}", frame.is_some()),
        Err(error) => println!("wall_latest=err {error}"),
    }

    println!("===CONTROL LOOPBACK===");
    match api::control_start() {
        Ok(status) => {
            println!(
                "control_start=ok {}x{} path={} backend={} encoder={}",
                status.width, status.height, status.texture_path, status.backend, status.encoder
            );
            let begin = Instant::now();
            let mut frames = 0u64;
            let mut last_bytes = 0usize;
            while begin.elapsed() < Duration::from_secs(3) {
                match api::control_frame() {
                    Ok(Some(frame)) => {
                        frames += 1;
                        last_bytes = frame.bytes.len();
                    }
                    Ok(None) => {}
                    Err(error) => {
                        println!("control_frame=err {error}");
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            match api::control_status() {
                Ok(status) => println!(
                    "control_status=ok running={} backend={} fps={:.1} decoded={} dropped={} p50={}ms p99={}ms err={}",
                    status.running,
                    status.backend,
                    status.fps,
                    status.decoded_frames,
                    status.dropped_frames,
                    status.latency_p50_us / 1000,
                    status.latency_p99_us / 1000,
                    status.last_error.unwrap_or_else(|| "-".into())
                ),
                Err(error) => println!("control_status=err {error}"),
            }
            println!("control_frame=ok samples={frames} last_bytes={last_bytes}");
            match api::control_stop() {
                Ok(status) => println!("control_stop=ok running={}", status.running),
                Err(error) => println!("control_stop=err {error}"),
            }
        }
        Err(error) => println!("control_start=err {error}"),
    }

    println!("===INPUT (XTest 本机注入)===");
    match api::input_start(1280, 800) {
        Ok(stats) => {
            println!("input_start=ok running={}", stats.running);
            let events = vec![
                api::InputEventDto {
                    kind: "move".into(),
                    x: 640,
                    y: 400,
                    button: 0,
                    pressed: false,
                    delta_x: 0,
                    delta_y: 0,
                    scan_code: 0,
                    modifiers: 0,
                    delete_units: 0,
                    text: String::new(),
                },
                api::InputEventDto {
                    kind: "button".into(),
                    x: 640,
                    y: 400,
                    button: 1,
                    pressed: true,
                    delta_x: 0,
                    delta_y: 0,
                    scan_code: 0,
                    modifiers: 0,
                    delete_units: 0,
                    text: String::new(),
                },
                api::InputEventDto {
                    kind: "button".into(),
                    x: 640,
                    y: 400,
                    button: 1,
                    pressed: false,
                    delta_x: 0,
                    delta_y: 0,
                    scan_code: 0,
                    modifiers: 0,
                    delete_units: 0,
                    text: String::new(),
                },
            ];
            match api::input_push(events) {
                Ok(stats) => println!(
                    "input_push=ok received={} emitted={} injected={} rejected={} err={}",
                    stats.received,
                    stats.emitted,
                    stats.injected,
                    stats.rejected,
                    stats.last_error.unwrap_or_else(|| "-".into())
                ),
                Err(error) => println!("input_push=err {error}"),
            }
            std::thread::sleep(Duration::from_millis(200));
            match api::input_stop() {
                Ok(stats) => println!(
                    "input_stop=ok injected={} keys_down={}",
                    stats.injected, stats.keys_down
                ),
                Err(error) => println!("input_stop=err {error}"),
            }
        }
        Err(error) => println!("input_start=err {error}"),
    }

    println!("===DONE===");
}
