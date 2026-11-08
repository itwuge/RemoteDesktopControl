// ============================================================
// 文件用途：剪贴板双向文本同步服务（S5-4 块 2）
//
// 层级：L4 桥接层（回环两端的剪贴板模拟：本机 = 控制端，远端 = 被控端缓冲区）
// 被谁调用：crates/rdc-bridge/src/api.rs 的 clipboard_* 函数
// 依据：rdc-clipboard 的 ClipboardSync（哈希去重 + 自写回环抑制）与线格式。
// 边界：回环阶段没有网络通道，远端内容保存在内存缓冲区里模拟对端剪贴板；
//       两端各用一个 ClipboardSync，回环防护由状态机本身负责，不另外造哈希表。
// ============================================================

use std::sync::{Mutex, OnceLock};

use rdc_clipboard::{create_clipboard, Clipboard, ClipboardError, ClipboardSync, SyncStats};

/// 剪贴板同步状态快照。
#[derive(Debug, Clone, Default)]
pub struct ClipboardStatusDto {
    /// 服务是否已启动（系统剪贴板可用）。
    pub running: bool,
    /// 远端缓冲区文本字符数。
    pub remote_chars: u64,
    /// 最近一次外发文本的字符数。
    pub local_chars: u64,
    /// 外发条数。
    pub sent: u64,
    /// 写回本机的条数。
    pub applied: u64,
    /// 重复内容被跳过的次数。
    pub skipped_duplicates: u64,
    /// 自写回环被跳过的次数。
    pub skipped_echoes: u64,
    /// 超限被拒绝的次数。
    pub rejected: u64,
    /// 最近一次同步内容的 SHA-256（十六进制）。
    pub last_hash: String,
    /// 最近一次写回后回读是否与写入一致。
    pub verified: bool,
    /// 最近错误。
    pub last_error: Option<String>,
}

/// 回环两端的状态。
struct SyncState {
    /// 控制端（本机）系统剪贴板。
    local_clipboard: Box<dyn Clipboard>,
    /// 控制端的同步状态机。
    local_sync: ClipboardSync,
    /// 被控端（模拟）的同步状态机。
    remote_sync: ClipboardSync,
    /// 被控端（模拟）的剪贴板内容。
    remote_text: String,
    /// 最近一次外发内容的字符数。
    local_chars: usize,
    /// 最近一次同步内容哈希。
    last_hash: Option<[u8; 32]>,
    /// 最近一次写回后的回读校验。
    verified: bool,
    /// 最近错误。
    last_error: Option<String>,
}

/// 当前会话槽位。
fn state() -> &'static Mutex<Option<SyncState>> {
    static STATE: OnceLock<Mutex<Option<SyncState>>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(None))
}

/// 启动剪贴板同步（幂等）。
///
/// # Returns
/// 启动后的状态快照。
///
/// # Errors
/// 平台剪贴板创建失败时返回错误文本。
pub fn start() -> Result<ClipboardStatusDto, String> {
    let mut guard = state().lock().map_err(|_| "剪贴板状态锁异常".to_owned())?;
    if guard.is_some() {
        return Ok(snapshot(guard.as_ref().expect("已判空")));
    }
    *guard = Some(SyncState {
        local_clipboard: create_clipboard(),
        local_sync: ClipboardSync::new(),
        remote_sync: ClipboardSync::new(),
        remote_text: String::new(),
        local_chars: 0,
        last_hash: None,
        verified: false,
        last_error: None,
    });
    Ok(snapshot(guard.as_ref().expect("刚写入")))
}

/// 把本机剪贴板文本发送到远端（模拟对端写入自己的剪贴板）。
///
/// # Returns
/// 同步后的状态快照（`remote_chars` 反映对端内容长度）。
///
/// # Errors
/// 通道未启动或本机剪贴板没有文本时返回错误文本。
pub fn send_local() -> Result<ClipboardStatusDto, String> {
    let mut guard = state().lock().map_err(|_| "剪贴板状态锁异常".to_owned())?;
    let current = guard.as_mut().ok_or_else(|| "剪贴板通道未启动".to_owned())?;
    current.last_error = None;
    let text = current
        .local_clipboard
        .read_text()
        .map_err(|error| clipboard_error("读取本机剪贴板失败", error))?
        .ok_or_else(|| "本机剪贴板没有文本".to_owned())?;
    let payload = current
        .local_sync
        .on_local_text(&text)
        .map_err(|error| clipboard_error("剪贴板编码失败", error))?;
    let Some(payload) = payload else {
        current.last_error = Some("内容重复或回环，已抑制外发".to_owned());
        return Ok(snapshot(current));
    };
    let applied = current
        .remote_sync
        .on_remote_payload(&payload)
        .map_err(|error| clipboard_error("远端写入失败", error))?;
    if let Some(remote_text) = applied {
        current.remote_text = remote_text;
    }
    current.local_chars = text.chars().count();
    current.last_hash = current.local_sync.last_sent_hash();
    current.verified = false;
    Ok(snapshot(current))
}

/// 把远端缓冲区内容取回本机剪贴板（模拟对端复制 → 本机粘贴）。
///
/// # Returns
/// 同步后的状态快照；`verified` 表示写回后回读一致。
///
/// # Errors
/// 通道未启动、远端为空或系统剪贴板写入失败时返回错误文本。
pub fn pull_remote() -> Result<ClipboardStatusDto, String> {
    let mut guard = state().lock().map_err(|_| "剪贴板状态锁异常".to_owned())?;
    let current = guard.as_mut().ok_or_else(|| "剪贴板通道未启动".to_owned())?;
    current.last_error = None;
    if current.remote_text.is_empty() {
        return Err("远端剪贴板为空，请先发送本地内容".to_owned());
    }
    let payload = current
        .remote_sync
        .on_local_text(&current.remote_text)
        .map_err(|error| clipboard_error("远端编码失败", error))?;
    let Some(payload) = payload else {
        current.last_error =
            Some("远端内容与最近一次同步相同（回环已抑制）".to_owned());
        return Ok(snapshot(current));
    };
    let applied = current
        .local_sync
        .on_remote_payload(&payload)
        .map_err(|error| clipboard_error("写回本机失败", error))?;
    let Some(text) = applied else {
        current.last_error = Some("回环防护：内容已在本机，未重复写入".to_owned());
        return Ok(snapshot(current));
    };
    current
        .local_clipboard
        .write_text(&text)
        .map_err(|error| clipboard_error("写入本机剪贴板失败", error))?;
    // 回读校验：写入成功后必须能读到同一文本。
    let read_back = current
        .local_clipboard
        .read_text()
        .map_err(|error| clipboard_error("回读本机剪贴板失败", error))?;
    current.verified = read_back.as_deref() == Some(text.as_str());
    current.last_hash = current.local_sync.last_applied_hash();
    Ok(snapshot(current))
}

/// 模拟对端剪贴板被用户修改（用于验证「远端 → 本机」方向）。
///
/// # Arguments
/// * `text` - 对端新的剪贴板文本
///
/// # Returns
/// 更新后的状态快照。
///
/// # Errors
/// 通道未启动时返回错误文本。
pub fn set_remote_text(text: String) -> Result<ClipboardStatusDto, String> {
    let mut guard = state().lock().map_err(|_| "剪贴板状态锁异常".to_owned())?;
    let current = guard.as_mut().ok_or_else(|| "剪贴板通道未启动".to_owned())?;
    current.remote_text = text;
    current.last_error = None;
    Ok(snapshot(current))
}

/// 读取远端缓冲区内容（用于 UI 展示，不写回本机）。
///
/// # Returns
/// 远端文本。
///
/// # Errors
/// 通道未启动时返回错误文本。
pub fn remote_text() -> Result<String, String> {
    let guard = state().lock().map_err(|_| "剪贴板状态锁异常".to_owned())?;
    let current = guard.as_ref().ok_or_else(|| "剪贴板通道未启动".to_owned())?;
    Ok(current.remote_text.clone())
}

/// 读取本机剪贴板文本（UI 预览与验证用）。
///
/// # Returns
/// 本机剪贴板文本；无文本时为空串。
///
/// # Errors
/// 通道未启动或读取失败时返回错误文本。
pub fn local_text() -> Result<String, String> {
    let mut guard = state().lock().map_err(|_| "剪贴板状态锁异常".to_owned())?;
    let current = guard.as_mut().ok_or_else(|| "剪贴板通道未启动".to_owned())?;
    let text = current
        .local_clipboard
        .read_text()
        .map_err(|error| clipboard_error("读取本机剪贴板失败", error))?;
    Ok(text.unwrap_or_default()) // gate-allow R-C-2: read_text 已成功，None 表示剪贴板不是文本（空/图片），属正常状态
}

/// 停止剪贴板同步并释放平台剪贴板句柄。
///
/// # Returns
/// 收尾后的状态快照（`running=false`）。
///
/// # Errors
/// 不会失败。
pub fn stop() -> Result<ClipboardStatusDto, String> {
    let taken = match state().lock() {
        Ok(mut guard) => guard.take(),
        Err(_) => None,
    };
    Ok(taken
        .as_ref()
        .map(snapshot_of)
        .unwrap_or_default()) // gate-allow R-C-2: 本机无文本时返回空快照，属“无内容可同步”的正常状态
}

/// 读取当前状态。
///
/// # Returns
/// 运行中为实时快照，停止后为空快照。
///
/// # Errors
/// 不会失败。
pub fn status() -> Result<ClipboardStatusDto, String> {
    if let Ok(guard) = state().lock() {
        if let Some(current) = guard.as_ref() {
            return Ok(snapshot(current));
        }
    }
    Ok(ClipboardStatusDto::default())
}

// =====** 内部 =====**

/// 生成状态快照。
fn snapshot(current: &SyncState) -> ClipboardStatusDto {
    let stats: SyncStats = current.local_sync.stats();
    ClipboardStatusDto {
        running: true,
        remote_chars: current.remote_text.chars().count() as u64,
        local_chars: current.local_chars as u64,
        sent: stats.sent,
        applied: stats.applied,
        skipped_duplicates: stats.skipped_duplicates,
        skipped_echoes: stats.skipped_echoes,
        rejected: stats.rejected,
        last_hash: current.last_hash.map(hex).unwrap_or_default(), // gate-allow R-C-2: 尚未同步过条目时 last_hash 为 None，空串即“无哈希”的正常取值
        verified: current.verified,
        last_error: current.last_error.clone(),
    }
}

/// 停表前后的快照（保留计数，标记 running=false）。
fn snapshot_of(current: &SyncState) -> ClipboardStatusDto {
    let mut snapshot = snapshot(current);
    snapshot.running = false;
    snapshot
}

/// 统一平台剪贴板错误文案。
fn clipboard_error(context: &str, error: ClipboardError) -> String {
    format!("{context}: {error}")
}

/// 32 字节哈希转十六进制。
fn hex(bytes: [u8; 32]) -> String {
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
    use super::hex;

    /// 哈希十六进制必须定长小写。
    #[test]
    fn hash_is_lowercase_fixed_length() {
        let mut bytes = [0u8; 32];
        bytes[0] = 0xAB;
        bytes[31] = 0x07;
        let text = hex(bytes);
        assert_eq!(text.len(), 64);
        assert!(text.starts_with("ab"));
        assert!(text.ends_with("07"));
    }
}
