// ============================================================
// 文件用途：S5-4 块 2 运行时自检（剪贴板双向 + 文件传输/续传/SHA-256）
//
// 层级：crates/rdc-bridge/examples（只走 api 公开入口）
// 跑法：先 Set-Clipboard，再 cargo run -p rdc-bridge --example transfer_selftest --release <期望文本>
// 输出：stdout 摘要，由 logs 脚本重定向落盘
// 边界：只在 %TEMP%\rdc-s54 造测试文件；接收目录 %LOCALAPPDATA%\RDC\received 保留落盘件作证据。
// ============================================================

use std::fs;

use rdc_bridge::api::{
    clipboard_local_text, clipboard_pull, clipboard_remote_text, clipboard_send,
    clipboard_set_remote, clipboard_start, clipboard_stop, file_transfer,
};

fn main() -> Result<(), String> {
    let expected = std::env::args()
        .nth(1)
        .unwrap_or_default()
        .replace("\r\n", "\n");
    println!("=== clipboard ===");
    let started = clipboard_start()?;
    println!("start running={} applied={}", started.running, started.applied);

    // 方向 1：本机 → 远端。
    let sent = clipboard_send()?;
    let remote = clipboard_remote_text()?;
    let send_ok = remote == expected;
    println!(
        "send ok={send_ok} remote_chars={} local_chars={} sent={} hash={}",
        sent.remote_chars, sent.local_chars, sent.sent, sent.last_hash
    );

    // 方向 2：远端 → 本机（模拟对端复制新文本），并回读校验。
    let remote_payload = "S54-远端-乙\n第二行 remote";
    clipboard_set_remote(remote_payload.to_owned())?;
    let pulled = clipboard_pull()?;
    let local = clipboard_local_text()?;
    let pull_ok = local == remote_payload;
    println!(
        "pull ok={pull_ok} verified={} applied={} dup={} echo={} error={:?}",
        pulled.verified,
        pulled.applied,
        pulled.skipped_duplicates,
        pulled.skipped_echoes,
        pulled.last_error
    );

    // 回环防护：刚写回本机的同一内容再次外发，必须被状态机抑制。
    let echoed = clipboard_send()?;
    let guard_ok = echoed.skipped_duplicates + echoed.skipped_echoes > 0
        || echoed.last_error.is_some();
    println!(
        "loop_guard={guard_ok} dup={} echo={} error={:?}",
        echoed.skipped_duplicates, echoed.skipped_echoes, echoed.last_error
    );

    let stopped = clipboard_stop()?;
    println!("clipboard stopped running={}", stopped.running);
    if stopped.running {
        return Err("剪贴板收尾未清理".to_owned());
    }
    if !(send_ok && pull_ok && pulled.verified && guard_ok) {
        return Err("剪贴板判据未通过".to_owned());
    }

    println!("=== file ===");
    // 10 MiB 测试文件：按 1 MiB 分块，第二轮在第 2 块后中断再续传。
    let dir = std::env::temp_dir().join("rdc-s54");
    fs::create_dir_all(&dir).map_err(|error| format!("建临时目录失败: {error}"))?;
    let source = dir.join("s54-file.bin");
    let mut data = Vec::with_capacity(10 * 1024 * 1024);
    for index in 0..(10 * 1024 * 1024) {
        data.push(((index * 31) % 251) as u8);
    }
    fs::write(&source, &data).map_err(|error| format!("写测试文件失败: {error}"))?;
    let source_text = source.display().to_string();

    let full = file_transfer(source_text.clone(), 0, 1024 * 1024)?;
    println!(
        "full equal={} bytes={} chunks={} source_sha={} dest_sha={} dest={}",
        full.equal, full.bytes, full.chunks, full.source_sha256, full.dest_sha256, full.dest
    );

    let resumed = file_transfer(source_text, 2, 1024 * 1024)?;
    println!(
        "resume equal={} interrupted={} resumed={} resumed_bytes={} dest={}",
        resumed.equal, resumed.interrupted, resumed.resumed, resumed.resumed_bytes, resumed.dest
    );

    if !full.equal || !resumed.equal || !resumed.resumed || resumed.resumed_bytes == 0 {
        return Err("文件判据未通过".to_owned());
    }
    println!("ALL PASS");
    Ok(())
}
