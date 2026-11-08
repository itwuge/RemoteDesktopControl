// ============================================================
// 文件用途：文件传输服务（S5-4 块 2）
//
// 层级：L4 桥接层（回环文件传输：源文件 → 分块 → 落盘到接收目录）
// 被谁调用：crates/rdc-bridge/src/api.rs 的 file_transfer 函数
// 依据：rdc-filetransfer 的分块计划、逐块校验、整文件 SHA-256 与续传位图。
// 边界：回环阶段没有网络通道，分块在内存里直接交给接收端；中断用位图落盘
//       模拟断线，再从位图恢复，验证「续传后两端 SHA-256 一致」。
// ============================================================

use std::fs;
use std::path::{Path, PathBuf};

use rdc_filetransfer::{
    decode_state, encode_state, hash_file, ChunkPlan, FileReceiver, FileSender,
    DEFAULT_CHUNK_SIZE,
};

/// 文件传输结果快照。
#[derive(Debug, Clone, Default)]
pub struct FileTransferDto {
    /// 源文件绝对路径。
    pub source: String,
    /// 落盘路径（接收目录内）。
    pub dest: String,
    /// 文件字节数。
    pub bytes: u64,
    /// 分块总数。
    pub chunks: u32,
    /// 源文件 SHA-256（十六进制）。
    pub source_sha256: String,
    /// 落盘文件 SHA-256（十六进制）。
    pub dest_sha256: String,
    /// 两端哈希是否一致。
    pub equal: bool,
    /// 是否发生过中断并续传。
    pub resumed: bool,
    /// 续传时跳过的字节数。
    pub resumed_bytes: u64,
    /// 是否在指定块后中断（测试/演示用）。
    pub interrupted: bool,
}

/// 传输一个文件到接收目录。
///
/// # Arguments
/// * `path` - 源文件路径
/// * `interrupt_after_chunks` - 大于 0 时在该块后中断并立即续传（验证断点续传）
/// * `chunk_size` - 分块大小，`0` 表示默认 4 MiB
///
/// # Returns
/// 两端哈希、字节数、续传信息。
///
/// # Errors
/// 源文件不存在、分块读写失败或校验不一致时返回错误文本。
pub fn send(
    path: String,
    interrupt_after_chunks: u32,
    chunk_size: u64,
) -> Result<FileTransferDto, String> {
    let source = PathBuf::from(&path);
    if !source.is_file() {
        return Err(format!("源文件不存在: {path}"));
    }
    let chunk_size = if chunk_size == 0 {
        DEFAULT_CHUNK_SIZE
    } else {
        chunk_size.min(DEFAULT_CHUNK_SIZE)
    };
    let dest = unique_dest(&source)?;
    let mut sender =
        FileSender::open(&source, chunk_size).map_err(|error| format!("打开源文件失败: {error}"))?;
    let source_hash = sender.file_hash();
    let total_bytes = sender.plan().total_bytes();
    let total_chunks = sender.plan().chunk_count();
    let mut receiver = FileReceiver::create(&dest, total_bytes, chunk_size)
        .map_err(|error| format!("创建接收文件失败: {error}"))?;

    let mut sent_chunks = 0u32;
    let mut interrupted = false;
    let state_path = resume_state_path(&dest);
    while let Some(chunk) = sender
        .next_chunk()
        .map_err(|error| format!("读取分块失败: {error}"))?
    {
        receiver
            .accept(&chunk)
            .map_err(|error| format!("接收分块失败: {error}"))?;
        sender
            .confirm(chunk.index, true)
            .map_err(|error| format!("确认分块失败: {error}"))?;
        sent_chunks += 1;
        if interrupt_after_chunks > 0 && sent_chunks >= interrupt_after_chunks {
            // 模拟断线：把已收位图落盘，随后丢弃两端状态。
            let bitmap = encode_state(receiver.received());
            fs::write(&state_path, bitmap).map_err(|error| format!("写续传位图失败: {error}"))?;
            interrupted = true;
            break;
        }
    }

    let (dest_hash, resumed, resumed_bytes) = if interrupted {
        let bitmap = fs::read(&state_path).map_err(|error| format!("读续传位图失败: {error}"))?;
        let confirmed = decode_state(&bitmap, total_chunks)
            .map_err(|error| format!("续传位图损坏: {error}"))?;
        let resumed_bytes = confirmed_bytes(sender.plan(), &confirmed);
        // 丢弃旧的发送/接收状态，模拟重连后重新打开。
        drop(sender);
        drop(receiver);
        let mut sender = FileSender::open(&source, chunk_size)
            .map_err(|error| format!("重连后打开源文件失败: {error}"))?;
        sender
            .resume_with(&confirmed)
            .map_err(|error| format!("恢复发送进度失败: {error}"))?;
        let mut receiver =
            FileReceiver::resume(&dest, total_bytes, chunk_size, &confirmed)
                .map_err(|error| format!("恢复接收进度失败: {error}"))?;
        while let Some(chunk) = sender
            .next_chunk()
            .map_err(|error| format!("续传读取分块失败: {error}"))?
        {
            receiver
                .accept(&chunk)
                .map_err(|error| format!("续传接收分块失败: {error}"))?;
            sender
                .confirm(chunk.index, true)
                .map_err(|error| format!("续传确认分块失败: {error}"))?;
        }
        let hash = receiver
            .finish(Some(source_hash))
            .map_err(|error| format!("整文件校验失败: {error}"))?;
        if let Err(err) = fs::remove_file(&state_path) {
            tracing::debug!(
                path = %state_path.display(),
                error = %err,
                "续传成功后清理断点状态文件失败，下次传输可能回退到完整校验"
            );
        }
        (hash, true, resumed_bytes)
    } else {
        let hash = receiver
            .finish(Some(source_hash))
            .map_err(|error| format!("整文件校验失败: {error}"))?;
        (hash, false, 0)
    };

    // 独立回读落盘文件，避免只看接收端返回值。
    let on_disk = hash_file(&dest).map_err(|error| format!("回读落盘文件失败: {error}"))?;
    let equal = on_disk == source_hash && dest_hash == source_hash;
    Ok(FileTransferDto {
        source: source.display().to_string(),
        dest: dest.display().to_string(),
        bytes: total_bytes,
        chunks: total_chunks,
        source_sha256: hex32(source_hash),
        dest_sha256: hex32(on_disk),
        equal,
        resumed,
        resumed_bytes,
        interrupted,
    })
}

/// 接收目录：`%LOCALAPPDATA%\RDC\received`（与块 3 的落盘约定一致）。
fn received_root() -> Result<PathBuf, String> {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let dir = base.join("RDC").join("received");
    fs::create_dir_all(&dir).map_err(|error| format!("创建接收目录失败: {error}"))?;
    Ok(dir)
}

/// 生成不覆盖已有文件的落盘路径（同名加序号）。
fn unique_dest(source: &Path) -> Result<PathBuf, String> {
    let root = received_root()?;
    let stem = source
        .file_stem()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_owned());
    let extension = source
        .extension()
        .map(|value| value.to_string_lossy().to_string());
    for index in 0..1000u32 {
        let name = match (&extension, index) {
            (Some(ext), 0) => format!("{stem}.{ext}"),
            (Some(ext), n) => format!("{stem} ({n}).{ext}"),
            (None, 0) => stem.clone(),
            (None, n) => format!("{stem} ({n})"),
        };
        let candidate = root.join(name);
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err("接收目录内同名文件过多".to_owned())
}

/// 续传位图落盘路径。
fn resume_state_path(dest: &Path) -> PathBuf {
    let mut name = dest
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_owned());
    name.push_str(".rdcstate");
    dest.with_file_name(name)
}

/// 已确认位图对应的字节数。
fn confirmed_bytes(plan: &ChunkPlan, confirmed: &[bool]) -> u64 {
    confirmed
        .iter()
        .enumerate()
        .filter(|(_, hit)| **hit)
        .map(|(index, _)| plan.chunk_len(index as u32).unwrap_or(0))
        .sum()
}

/// 32 字节哈希转十六进制。
fn hex32(bytes: [u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for byte in bytes {
        out.push(char::from_digit((byte >> 4) as u32, 16).unwrap_or('0'));
        out.push(char::from_digit((byte & 0x0F) as u32, 16).unwrap_or('0'));
    }
    out
}

// =====** 自检 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 续传字节数只统计置位的块。
    #[test]
    fn confirmed_bytes_sums_selected_chunks() {
        let plan = ChunkPlan::new(10, 4).expect("计划");
        // 10 字节按 4 字节分块：4 + 4 + 2，首末两块合计 6。
        assert_eq!(confirmed_bytes(&plan, &[true, false, true]), 6);
        assert_eq!(confirmed_bytes(&plan, &[false, false, false]), 0);
    }

    /// 接收目录必须是绝对路径且以 received 结尾。
    #[test]
    fn received_root_is_received_dir() {
        let root = received_root().expect("目录");
        assert!(root.is_absolute());
        assert!(root.ends_with("received"));
    }
}
