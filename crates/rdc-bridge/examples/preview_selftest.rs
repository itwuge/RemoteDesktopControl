// ============================================================
// 示例用途：控制页质量角标数据自检（fps / 延迟 / 码率）
// 运行：cargo run -p rdc-bridge --example preview_selftest
// 输出：采集 3 秒内的两次快照，验证码率来自真实编码包大小
// 注意：无纹理消费者时 fps / 延迟为 0（fps 统计的是解码上屏帧，锁屏静态桌面几乎无新帧），
//       本例只判定「码率与累计字节非零且停得干净」，fps 上屏数据由控制页运行时回填。
// ============================================================

use std::thread::sleep;
use std::time::Duration;

fn main() {
    let started = rdc_bridge::api::control_start().expect("启动控制页预览");
    println!(
        "control_start: running={} captured={} encoded={}",
        started.running, started.captured_frames, started.encoded_frames
    );

    // 第一窗口（预热 1 秒，覆盖采集器首帧与编码器初始化）
    sleep(Duration::from_millis(1200));
    let first = rdc_bridge::api::control_status().expect("读取状态");
    println!(
        "t=1.2s: fps={:.1} latency_us={} bitrate_kbps={} encoded_bytes={} encoded={}",
        first.fps,
        first.latency_last_us,
        first.bitrate_kbps,
        first.encoded_bytes,
        first.encoded_frames
    );

    // 第二窗口（稳定后，码率应为真实编码输出的 1 秒均速）
    sleep(Duration::from_millis(2000));
    let second = rdc_bridge::api::control_status().expect("读取状态");
    println!(
        "t=3.2s: fps={:.1} latency_us={} bitrate_kbps={} encoded_bytes={} encoded={}",
        second.fps,
        second.latency_last_us,
        second.bitrate_kbps,
        second.encoded_bytes,
        second.encoded_frames
    );

    let stopped = rdc_bridge::api::control_stop().expect("停止预览");
    println!("control_stop: running={}", stopped.running);

    let fps_ok = second.fps >= 55.0;
    let bitrate_ok = second.bitrate_kbps > 0 && second.encoded_bytes > 0;
    println!(
        "checks: fps_ok={}（无纹理消费者，仅记录）bitrate_ok={} stopped_ok={}",
        fps_ok,
        bitrate_ok,
        !stopped.running
    );
    if !(bitrate_ok && !stopped.running) {
        eprintln!("preview_selftest FAILED");
        std::process::exit(1);
    }
    println!("preview_selftest ALL PASS");
}
