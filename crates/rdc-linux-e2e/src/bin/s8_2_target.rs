//! 用途：s8_2_target · 目标控件（被点击验证用）
//!
//! 运行：`DISPLAY=:0 cargo run -p rdc-linux-e2e --bin s8_2_target -- --x 200 --y 150 --out /tmp/s8-2-target.log`
//!
//! 行为：建一个 override-redirect 窗口，居中画一块纯绿方块；每次 ButtonPress 把
//!       `x_root/y_root` 追加到 `--out`，供外部脚本与控制端请求坐标对比（判据 ≤2px）。

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
        pub x: i16,
        pub y: i16,
        pub width: u32,
        pub height: u32,
        pub green: u32,
        pub secs: u64,
        pub out: String,
    }

    /// 解析命令行。
    pub fn parse_args() -> Result<Args, E2eError> {
        let mut args = Args {
            x: 200,
            y: 150,
            width: 400,
            height: 300,
            green: 100,
            secs: 0,
            out: "/tmp/s8-2-target.log".into(),
        };
        let mut iter = std::env::args().skip(1);
        while let Some(arg) = iter.next() {
            let value = iter.next();
            match arg.as_str() {
                "--x" => args.x = value.ok_or("--x 缺值")?.parse().map_err(|error| E2eError::with_source("--x 需整数", error))?,
                "--y" => args.y = value.ok_or("--y 缺值")?.parse().map_err(|error| E2eError::with_source("--y 需整数", error))?,
                "--w" => args.width = value.ok_or("--w 缺值")?.parse().map_err(|error| E2eError::with_source("--w 需整数", error))?,
                "--h" => args.height = value.ok_or("--h 缺值")?.parse().map_err(|error| E2eError::with_source("--h 需整数", error))?,
                "--green" => args.green = value.ok_or("--green 缺值")?.parse().map_err(|error| E2eError::with_source("--green 需整数", error))?,
                "--secs" => args.secs = value.ok_or("--secs 缺值")?.parse().map_err(|error| E2eError::with_source("--secs 需整数", error))?,
                "--out" => args.out = value.ok_or("--out 缺值")?,
                other => return Err(E2eError::message(format!("未知参数: {other}"))),
            }
        }
        Ok(args)
    }
}

#[cfg(target_os = "linux")]
use std::time::{Duration, Instant};

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
    match run(&args) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("运行失败: {error}");
            2
        }
    }
}

#[cfg(target_os = "linux")]
fn run(args: &linux_impl::Args) -> Result<(), String> {
    use std::io::Write;

    use x11rb::connection::Connection;
    use x11rb::protocol::xproto::{
        ConnectionExt, CreateGCAux, CreateWindowAux, EventMask, Rectangle, WindowClass,
    };

    let (conn, screen_num) = x11rb::connect(None).map_err(|error| format!("连接 X11 失败: {error}"))?;
    let screen = &conn.setup().roots[screen_num];
    let root = screen.root;
    let depth = screen.root_depth;
    let visual = screen.root_visual;

    let window = conn.generate_id().map_err(|error| format!("分配窗口 id 失败: {error}"))?;
    let gc = conn.generate_id().map_err(|error| format!("分配 GC id 失败: {error}"))?;
    let aux = CreateWindowAux::new()
        .background_pixel(0x0000_0000u32)
        .event_mask(
            EventMask::EXPOSURE
                | EventMask::BUTTON_PRESS
                | EventMask::BUTTON_RELEASE
                | EventMask::STRUCTURE_NOTIFY,
        )
        .override_redirect(1u32);
    conn.create_window(
        depth,
        window,
        root,
        args.x,
        args.y,
        args.width as u16,
        args.height as u16,
        0,
        WindowClass::INPUT_OUTPUT,
        visual,
        &aux,
    )
    .map_err(|error| format!("创建窗口失败: {error}"))?;
    // 纯绿 0x00FF00 在标准 24 位 TrueColor 下即为目标色。
    conn.create_gc(
        gc,
        window,
        &CreateGCAux::new().foreground(0x0000_FF00u32),
    )
    .map_err(|error| format!("创建 GC 失败: {error}"))?;
    conn.map_window(window).map_err(|error| format!("映射窗口失败: {error}"))?;
    conn.flush().map_err(|error| format!("刷新 X11 失败: {error}"))?;

    let green = args.green.min(args.width).min(args.height);
    let gx = (args.width as i16 - green as i16) / 2;
    let gy = (args.height as i16 - green as i16) / 2;
    let rect = Rectangle {
        x: gx,
        y: gy,
        width: green as u16,
        height: green as u16,
    };
    conn.poly_fill_rectangle(window, gc, &[rect])
        .map_err(|error| format!("绘制绿块失败: {error}"))?;
    conn.flush().map_err(|error| format!("刷新 X11 失败: {error}"))?;

    let center_x = i32::from(args.x) + i32::from(gx) + green as i32 / 2;
    let center_y = i32::from(args.y) + i32::from(gy) + green as i32 / 2;
    println!(
        "TARGET_READY pid={} window={} rect=({},{} {}x{}) green_center=({},{}) out={}",
        std::process::id(),
        window,
        args.x,
        args.y,
        args.width,
        args.height,
        center_x,
        center_y,
        args.out
    );

    let start = Instant::now();
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&args.out)
        .map_err(|error| format!("打开输出文件 {} 失败: {error}", args.out))?;
    loop {
        match conn.poll_for_event() {
            Ok(Some(event)) => match event {
                x11rb::protocol::Event::Expose(expose) => {
                    if expose.count == 0 {
                        conn.poly_fill_rectangle(window, gc, &[rect])
                            .map_err(|error| format!("绘制目标窗口失败: {error}"))?;
                        conn.flush()
                            .map_err(|error| format!("刷新 X11 请求失败: {error}"))?;
                    }
                }
                x11rb::protocol::Event::ButtonPress(press) => {
                    let ms = start.elapsed().as_millis() as u64;
                    let line = format!(
                        "PRESS t_ms={} x_root={} y_root={} button={}\n",
                        ms,
                        i32::from(press.root_x),
                        i32::from(press.root_y),
                        press.detail
                    );
                    file.write_all(line.as_bytes())
                        .map_err(|error| format!("写入点击记录失败: {error}"))?;
                    file.flush()
                        .map_err(|error| format!("刷新点击记录失败: {error}"))?;
                    print!("TARGET_PRESS {line}");
                }
                _ => {}
            },
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(_) => break,
        }
        if args.secs > 0 && start.elapsed().as_secs() >= args.secs {
            break;
        }
    }
    let _ = conn.destroy_window(window); // gate-allow R-C-2: 收尾释放无返回值，连接关闭后资源由 X server 回收
    let _ = conn.free_gc(gc); // gate-allow R-C-2: 收尾释放无返回值，GC 随连接关闭回收
    let _ = conn.flush(); // gate-allow R-C-2: 收尾冲刷无返回值，主流程已返回 Ok
    Ok(())
}
