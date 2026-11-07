// ============================================================
// 文件用途：S5-4 块 1 运行时自检（真实 SendInput 注入 + 光标回读）
//
// 层级：crates/rdc-bridge/examples（只走 api 公开入口，不碰内部实现）
// 跑法：cargo run -p rdc-bridge --example input_selftest --release
// 输出：stdout 的 CSV/摘要，由 logs 脚本重定向落盘
// 边界：只在当前会话桌面注入；不写文件、不改注册表、不留后台进程。
// ============================================================

#[cfg(target_os = "windows")]
use std::thread::sleep;
#[cfg(target_os = "windows")]
use std::time::Duration;

#[cfg(target_os = "windows")]
use rdc_bridge::api::{
    input_flush, input_push, input_start, input_status, input_stop, InputEventDto,
};

/// Win32 POINT（仅 Windows 自检使用）。
#[cfg(target_os = "windows")]
#[repr(C)]
struct Point {
    x: i32,
    y: i32,
}

/// Win32 API 声明：Linux 上无对应符号，按平台门禁隔离。
#[cfg(target_os = "windows")]
unsafe extern "system" {
    /// 取屏幕物理尺寸（0=宽，1=高）。
    fn GetSystemMetrics(index: i32) -> i32;
    /// 读取当前光标位置。
    fn GetCursorPos(point: *mut Point) -> i32;
}

/// 构造一条指针移动事件。
#[cfg(target_os = "windows")]
fn move_event(x: i32, y: i32) -> InputEventDto {
    InputEventDto {
        kind: "move".to_owned(),
        x,
        y,
        button: 0,
        pressed: false,
        delta_x: 0,
        delta_y: 0,
        scan_code: 0,
        modifiers: 0,
        delete_units: 0,
        text: String::new(),
    }
}

/// 构造一条键盘事件。
#[cfg(target_os = "windows")]
fn key_event(scan_code: u16, pressed: bool) -> InputEventDto {
    InputEventDto {
        kind: "key".to_owned(),
        x: 0,
        y: 0,
        button: 0,
        pressed,
        delta_x: 0,
        delta_y: 0,
        scan_code,
        modifiers: 0,
        delete_units: 0,
        text: String::new(),
    }
}

/// Windows 自检入口。
#[cfg(target_os = "windows")]
fn main() -> Result<(), String> {
    let width = unsafe { GetSystemMetrics(0) }.max(1);
    let height = unsafe { GetSystemMetrics(1) }.max(1);
    println!("desktop={width}x{height}");

    let stats = input_start(width as u32, height as u32)?;
    println!("start running={} injected={}", stats.running, stats.injected);

    // 20 个目标点：覆盖四角与中心，避开边缘 40 px。
    let margin = 40;
    let mut targets = Vec::new();
    for row in 0..4 {
        for column in 0..5 {
            let x = margin
                + ((width - margin * 2) as f64 * (column as f64 + 0.5) / 5.0) as i32;
            let y = margin
                + ((height - margin * 2) as f64 * (row as f64 + 0.5) / 4.0) as i32;
            targets.push((x, y));
        }
    }

    let mut success = 0usize;
    let mut max_deviation = 0i32;
    let mut total_deviation = 0i64;
    println!("point,expected_x,expected_y,actual_x,actual_y,deviation");
    for (x, y) in &targets {
        input_push(vec![move_event(*x, *y)])?;
        input_flush()?;
        sleep(Duration::from_millis(15));
        let mut point = Point { x: 0, y: 0 };
        let got = unsafe { GetCursorPos(&mut point) };
        if got == 0 {
            println!("{x},{y},{x},{y},read_failed");
            continue;
        }
        let deviation = (point.x - *x).abs().max((point.y - *y).abs());
        if deviation <= 2 {
            success += 1;
        }
        max_deviation = max_deviation.max(deviation);
        total_deviation += deviation as i64;
        println!("{x},{y},{},{},{}", point.x, point.y, deviation);
    }

    // 键盘序列：S(0x1F) 5(0x06) 4(0x05) 逐键按下抬起，记录注入结果。
    let mut last_rejected = input_status()?.rejected;
    for scan_code in [0x1Fu16, 0x06, 0x05] {
        input_push(vec![key_event(scan_code, true)])?;
        input_flush()?;
        sleep(Duration::from_millis(20));
        input_push(vec![key_event(scan_code, false)])?;
        input_flush()?;
        sleep(Duration::from_millis(20));
        let stats = input_status()?;
        println!(
            "key scan={scan_code:#06x} rejected_delta={} error={:?}",
            stats.rejected - last_rejected,
            stats.last_error
        );
        last_rejected = stats.rejected;
    }

    let stats = input_status()?;
    println!(
        "mouse points={} ok={} max_dev={} avg_dev={:.2}",
        targets.len(),
        success,
        max_deviation,
        total_deviation as f64 / targets.len() as f64
    );
    println!(
        "stats received={} emitted={} injected={} rejected={} merged={} throttled={} keys_down={} success_rate={:.3}",
        stats.received,
        stats.emitted,
        stats.injected,
        stats.rejected,
        stats.merged,
        stats.throttled,
        stats.keys_down,
        stats.success_rate
    );

    let stopped = input_stop()?;
    println!(
        "stopped running={} keys_down={} injected={} rejected={}",
        stopped.running, stopped.keys_down, stopped.injected, stopped.rejected
    );
    if stopped.running || stopped.keys_down != 0 {
        return Err("收尾未清理干净".to_owned());
    }
    Ok(())
}

/// 非 Windows 平台入口：自检依赖 Win32 光标回读，明确失败而不是伪造通过。
#[cfg(not(target_os = "windows"))]
fn main() -> Result<(), String> {
    Err("input_selftest 依赖 GetCursorPos/GetSystemMetrics，目前仅支持 Windows".to_owned())
}
