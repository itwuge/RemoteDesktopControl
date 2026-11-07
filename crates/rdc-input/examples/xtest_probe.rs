//! rdc-input / Linux XTest 注入实测探针（示例程序，不进入生产路径）
//!
//! 用途：
//! - `move`：把指针移动到若干目标坐标，回读实际位置，输出偏差（判据 I1）；
//! - `keys`：按顺序注入 20 个键，配合 tkinter 窗口标题探针验证前台接收（判据 I2/I3）；
//! - 备用通道：`RDC_INPUT_FORCE_UINPUT=1` 时走 uinput，再用 `move` 校验（判据 I5）。
//!
//! 运行示例：
//! `DISPLAY=:0 cargo run -p rdc-input --example xtest_probe -- move --width 1280 --height 800`
//! `DISPLAY=:0 cargo run -p rdc-input --example xtest_probe -- keys --text abcdefghijklmnopqrst`

// 该示例只用 Linux/X11 的 XTest 与 Xlib 回读，非 Linux 目标下整份实现不参与编译，
// 仅保留一个空壳 main，避免 cargo test / cargo clippy --all-targets 在 Windows 上失败。
#[cfg(target_os = "linux")]
use std::time::Duration;

#[cfg(target_os = "linux")]
use rdc_input::{
    create_injector, InputEvent, KeyEvent, PointerEvent, Viewport,
};
#[cfg(target_os = "linux")]
use tracing_subscriber::EnvFilter;
#[cfg(target_os = "linux")]
use x11rb::connection::Connection;
#[cfg(target_os = "linux")]
use x11rb::protocol::xproto::ConnectionExt as _;
#[cfg(target_os = "linux")]
use x11rb::rust_connection::RustConnection;

// =====** 指针回读 =====**

/// 用独立 X11 连接回读指针位置（不能用 XTest 连接自证）。
#[cfg(target_os = "linux")]
struct PointerReader {
    /// 独立连接。
    conn: RustConnection,
    /// root 窗口。
    root: u32,
}

#[cfg(target_os = "linux")]
impl PointerReader {
    /// 建立回读连接。
    ///
    /// # Returns
    /// 回读器。
    ///
    /// # Errors
    /// X11 连接失败时返回说明文本。
    fn new() -> Result<Self, String> {
        let (conn, screen) = x11rb::connect(None).map_err(|e| format!("X11 连接失败: {e}"))?;
        let root = conn.setup().roots[screen].root;
        Ok(Self { conn, root })
    }

    /// 读取当前指针 root 坐标。
    ///
    /// # Returns
    /// `(x, y)`。
    ///
    /// # Errors
    /// X 请求失败时返回说明文本。
    fn position(&self) -> Result<(i32, i32), String> {
        let reply = self
            .conn
            .query_pointer(self.root)
            .map_err(|e| format!("QueryPointer 请求失败: {e}"))?
            .reply()
            .map_err(|e| format!("QueryPointer 回复失败: {e}"))?;
        Ok((i32::from(reply.root_x), i32::from(reply.root_y)))
    }
}

// =====** 扫描码表 =====**

/// 小写字母 → PC/AT Set 1 扫描码（只覆盖探针所需字符）。
///
/// # Arguments
/// * `ch` - ASCII 字符。
///
/// # Returns
/// 扫描码；未收录返回 `None`。
///
/// # Errors
/// 不会失败。
#[cfg(target_os = "linux")]
fn letter_scan_code(ch: char) -> Option<u16> {
    Some(match ch {
        'q' => 0x10,
        'w' => 0x11,
        'e' => 0x12,
        'r' => 0x13,
        't' => 0x14,
        'y' => 0x15,
        'u' => 0x16,
        'i' => 0x17,
        'o' => 0x18,
        'p' => 0x19,
        'a' => 0x1E,
        's' => 0x1F,
        'd' => 0x20,
        'f' => 0x21,
        'g' => 0x22,
        'h' => 0x23,
        'j' => 0x24,
        'k' => 0x25,
        'l' => 0x26,
        'z' => 0x2C,
        'x' => 0x2D,
        'c' => 0x2E,
        'v' => 0x2F,
        'b' => 0x30,
        'n' => 0x31,
        'm' => 0x32,
        _ => return None,
    })
}

// =====** 入口 =====**

/// 探针入口（仅 Linux/X11）。
///
/// # Returns
/// 判据满足返回 0，否则返回 2。
///
/// # Errors
/// 不会 panic；错误以退出码表达。
#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .with_target(false)
        .try_init();

    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().map(String::as_str).unwrap_or("move");
    match mode {
        "move" => run_move(&args),
        "keys" => run_keys(&args),
        other => {
            eprintln!("未知模式: {other}（可选 move / keys）");
            std::process::ExitCode::from(2)
        }
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("xtest_probe 仅支持 Linux/X11（XTest），当前平台不适用");
}

/// 解析 `--key value` 形式的参数。
///
/// # Arguments
/// * `args` - 原始参数。
/// * `key` - 参数名（含 `--`）。
///
/// # Returns
/// 参数值。
///
/// # Errors
/// 缺失时返回 `None`。
#[cfg(target_os = "linux")]
fn arg_value<'a>(args: &'a [String], key: &str) -> Option<&'a str> {
    args.iter()
        .position(|item| item == key)
        .and_then(|index| args.get(index + 1))
        .map(String::as_str)
}

/// 指针定位实测（判据 I1）。
///
/// # Arguments
/// * `args` - 命令行参数（支持 `--width`/`--height`）。
///
/// # Returns
/// 最大偏差 ≤2px 返回 0。
///
/// # Errors
/// 注入或回读失败时返回 2。
#[cfg(target_os = "linux")]
fn run_move(args: &[String]) -> std::process::ExitCode {
    let width: u32 = arg_value(args, "--width")
        .and_then(|value| value.parse().ok())
        .unwrap_or(1280);
    let height: u32 = arg_value(args, "--height")
        .and_then(|value| value.parse().ok())
        .unwrap_or(800);
    let mut injector = match create_injector() {
        Ok(injector) => injector,
        Err(error) => {
            eprintln!("创建注入器失败: {error}");
            return std::process::ExitCode::from(2);
        }
    };
    injector.set_viewport(Viewport {
        x: 0,
        y: 0,
        width,
        height,
        scale: 1.0,
    });
    let reader = match PointerReader::new() {
        Ok(reader) => reader,
        Err(error) => {
            eprintln!("{error}");
            return std::process::ExitCode::from(2);
        }
    };
    // 覆盖四角 + 中心；四角留 1px 边距，避免落在边缘被窗口管理器抢焦点。
    let targets = [
        (1, 1),
        (width as i32 - 2, 1),
        (width as i32 - 2, height as i32 - 2),
        (1, height as i32 - 2),
        (width as i32 / 2, height as i32 / 2),
    ];
    let mut max_dev = 0i32;
    for (index, (x, y)) in targets.iter().enumerate() {
        let event = InputEvent::Pointer(PointerEvent {
            x: *x,
            y: *y,
            button: None,
            pressed: false,
        });
        if let Err(error) = injector.inject(event) {
            eprintln!("注入失败: {error}");
            return std::process::ExitCode::from(2);
        }
        // 给 X 服务器与合成器时间处理 MotionNotify。
        std::thread::sleep(Duration::from_millis(80));
        let actual = match reader.position() {
            Ok(actual) => actual,
            Err(error) => {
                eprintln!("{error}");
                return std::process::ExitCode::from(2);
            }
        };
        let dx = (actual.0 - *x).abs();
        let dy = (actual.1 - *y).abs();
        let dev = dx.max(dy);
        max_dev = max_dev.max(dev);
        println!(
            "MOVE i={index} target=({x},{y}) actual=({},{}) dx={dx} dy={dy} dev={dev}",
            actual.0, actual.1
        );
    }
    let ok = max_dev <= 2;
    println!(
        "MOVE_JSON {{\"points\":{},\"max_dev\":{},\"ok\":{ok},\"width\":{width},\"height\":{height}}}",
        targets.len(),
        max_dev
    );
    if ok {
        println!("MOVE_VERDICT pass max_dev={max_dev}");
        std::process::ExitCode::SUCCESS
    } else {
        println!("MOVE_VERDICT fail max_dev={max_dev}");
        std::process::ExitCode::from(2)
    }
}

/// 键盘顺序注入实测（判据 I2/I3，窗口标题由外部探针回读）。
///
/// # Arguments
/// * `args` - 命令行参数（支持 `--text`、`--gap-ms`）。
///
/// # Returns
/// 全部注入成功返回 0。
///
/// # Errors
/// 创建注入器失败或字符无扫描码时返回 2。
#[cfg(target_os = "linux")]
fn run_keys(args: &[String]) -> std::process::ExitCode {
    let text = arg_value(args, "--text").unwrap_or("abcdefghijklmnopqrst");
    let gap_ms: u64 = arg_value(args, "--gap-ms")
        .and_then(|value| value.parse().ok())
        .unwrap_or(120);
    let mut injector = match create_injector() {
        Ok(injector) => injector,
        Err(error) => {
            eprintln!("创建注入器失败: {error}");
            return std::process::ExitCode::from(2);
        }
    };
    println!("KEYS_START count={} gap_ms={gap_ms}", text.chars().count());
    let mut sent = 0usize;
    for (index, ch) in text.chars().enumerate() {
        let Some(scan_code) = letter_scan_code(ch) else {
            eprintln!("字符 {ch:?} 未收录扫描码，测试中止");
            return std::process::ExitCode::from(2);
        };
        for pressed in [true, false] {
            let event = InputEvent::Key(KeyEvent {
                scan_code,
                pressed,
                modifiers: 0,
            });
            if let Err(error) = injector.inject(event) {
                eprintln!("注入失败: {error}");
                return std::process::ExitCode::from(2);
            }
            // 按下与抬起之间留一点间隔，避免被目标程序合并。
            std::thread::sleep(Duration::from_millis(20));
        }
        sent += 1;
        println!("KEY i={index} ch={ch} scan=0x{scan_code:02X}");
        std::thread::sleep(Duration::from_millis(gap_ms));
    }
    println!("KEYS_JSON {{\"sent\":{sent},\"ok\":true}}");
    std::process::ExitCode::SUCCESS
}
