//! rdc-clipboard / 文本同步状态机（去重 + 环路防护）
//!
//! 层级：L3 交互层
//! 用途：判断本机剪贴板变化是否需要发往对端、对端内容是否需要写回本机；
//!       用内容哈希去重，并识别“自己写入引起的回环”。
//! 调用方：rdc-core 的剪贴板轮询线程（本机变化）与 `rdc-ctl` 接收循环（远端载荷）
//!
//! 环路是怎么被挡住的：
//! 1. 收到远端内容 → 记 `last_applied = hash` → 写本机剪贴板；
//! 2. 本机剪贴板随即触发一次变化事件 → [`ClipboardSync::on_local_text`] 发现
//!    `last_applied == hash`，判定为自写回环，直接跳过（只计数，不外发）；
//! 3. 平台层再用 `GetClipboardSequenceNumber` 做第二道确认（见 `platform::windows`）。
//!
//! 同一内容重复复制（用户多次按 Ctrl+C）也因 `last_sent == hash` 被去重，
//! 不会周期性重发。

use crate::error::ClipboardError;
use crate::text::{self, MAX_TEXT_BYTES};
use crate::wire::{self, HASH_LEN};

// =====** SyncStats =====**

/// 同步计数，供日志落盘（每会话输出一次或按需采样）。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncStats {
    /// 成功发出的文本条数。
    pub sent: u64,
    /// 成功应用（写回本机）的远端文本条数。
    pub applied: u64,
    /// 因内容重复被跳过的次数（本地重复复制 / 远端重复下发）。
    pub skipped_duplicates: u64,
    /// 因自写回环被跳过的次数。
    pub skipped_echoes: u64,
    /// 因超限被拒绝的本地变化次数。
    pub rejected: u64,
}

// =====** ClipboardSync =====**

/// 剪贴板文本双向同步状态机（单线程持有，无锁）。
#[derive(Debug, Default)]
pub struct ClipboardSync {
    /// 最近一次成功外发内容的哈希。
    last_sent: Option<[u8; HASH_LEN]>,
    /// 最近一次应用远端内容后写入本机剪贴板的哈希。
    last_applied: Option<[u8; HASH_LEN]>,
    /// 计数。
    stats: SyncStats,
}

impl ClipboardSync {
    /// 构造空状态机。
    ///
    /// # Returns
    /// 无历史内容的实例（首次本机变化会正常外发）。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new() -> Self {
        Self::default()
    }

    /// 处理本机剪贴板的一次文本变化。
    ///
    /// # Arguments
    /// * `text` - 本机剪贴板读到的原始文本（可含 CRLF）
    ///
    /// # Returns
    /// 需要外发时返回编码好的载荷；重复/回环/超限时返回 `None`。
    ///
    /// # Errors
    /// 归一化换行后超过 [`MAX_TEXT_BYTES`] 时返回 [`ClipboardError::TooLarge`]
    /// （同时计入 `rejected`），调用方应记日志并丢弃本次变化。
    pub fn on_local_text(&mut self, text: &str) -> Result<Option<Vec<u8>>, ClipboardError> {
        let normalized = text::to_network(text);
        let size = normalized.len();
        if size > MAX_TEXT_BYTES {
            self.stats.rejected += 1;
            return Err(ClipboardError::TooLarge {
                size,
                limit: MAX_TEXT_BYTES,
            });
        }
        let hash = wire::digest(normalized.as_bytes());
        if self.last_applied == Some(hash) {
            // 本机剪贴板刚被我们自己写回：这是回环，不是用户复制。
            self.stats.skipped_echoes += 1;
            return Ok(None);
        }
        if self.last_sent == Some(hash) {
            self.stats.skipped_duplicates += 1;
            return Ok(None);
        }
        self.last_sent = Some(hash);
        self.stats.sent += 1;
        wire::encode_text_with_hash(normalized.as_ref(), &hash).map(Some)
    }

    /// 处理从对端收到的剪贴板载荷。
    ///
    /// # Arguments
    /// * `bytes` - 单条剪贴板载荷（含 kind/hash/长度）
    ///
    /// # Returns
    /// 需要写回本机剪贴板时返回 LF 归一化的文本；重复或回环时返回 `None`。
    ///
    /// # Errors
    /// 载荷损坏、类型未实现或哈希不符时返回对应错误；调用方记录后丢弃本条。
    pub fn on_remote_payload(&mut self, bytes: &[u8]) -> Result<Option<String>, ClipboardError> {
        let (text, hash) = wire::decode_text(bytes)?;
        if self.last_applied == Some(hash) {
            // 对端重复下发同一内容。
            self.stats.skipped_duplicates += 1;
            return Ok(None);
        }
        if self.last_sent == Some(hash) {
            // 对端把本机内容原样回传：不写回本机，避免两端来回弹。
            self.stats.skipped_echoes += 1;
            return Ok(None);
        }
        self.last_applied = Some(hash);
        self.stats.applied += 1;
        Ok(Some(text))
    }

    /// 最近一次外发内容的哈希（调试用）。
    pub fn last_sent_hash(&self) -> Option<[u8; HASH_LEN]> {
        self.last_sent
    }

    /// 最近一次应用内容的哈希（调试用）。
    pub fn last_applied_hash(&self) -> Option<[u8; HASH_LEN]> {
        self.last_applied
    }

    /// 取当前计数快照。
    pub fn stats(&self) -> SyncStats {
        self.stats
    }
}

// =====** 自检 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 本机简单变化 → 载荷可解出同样的 LF 文本。
    #[test]
    fn local_change_is_sent_normalized() {
        let mut sync = ClipboardSync::new();
        let payload = sync.on_local_text("第一行\r\n第二行\r\n").expect("发送");
        let payload = payload.expect("应外发");
        let (text, _) = wire::decode_text(&payload).expect("解码");
        assert_eq!(text, "第一行\n第二行\n");
        assert_eq!(sync.stats().sent, 1);
    }

    /// 相同内容再次复制只去重，不重复外发。
    #[test]
    fn repeated_local_copy_is_deduplicated() {
        let mut sync = ClipboardSync::new();
        assert!(sync.on_local_text("same").expect("首次").is_some());
        assert!(sync.on_local_text("same").expect("重复").is_none());
        assert_eq!(sync.stats().skipped_duplicates, 1);
        assert_eq!(sync.stats().sent, 1);
        // 内容变化后恢复外发。
        assert!(sync.on_local_text("changed").expect("变化").is_some());
        assert_eq!(sync.stats().sent, 2);
    }

    /// 远端应用后，本机剪贴板回写同内容 → 判定为自写回环，不外发。
    #[test]
    fn local_echo_after_remote_apply_is_ignored() {
        let mut sync = ClipboardSync::new();
        let payload = wire::encode_text("来自对端").expect("编码");
        let applied = sync.on_remote_payload(&payload).expect("应用");
        assert_eq!(applied.as_deref(), Some("来自对端"));
        assert_eq!(sync.stats().applied, 1);

        // 模拟平台层把刚写入的内容又当成“本机变化”报上来。
        let echo = sync.on_local_text("来自对端").expect("回环");
        assert!(echo.is_none(), "自写回环不得外发");
        assert_eq!(sync.stats().skipped_echoes, 1);
        assert_eq!(sync.stats().sent, 0);
    }

    /// 对端重复下发同一内容 → 只应用一次。
    #[test]
    fn repeated_remote_payload_is_deduplicated() {
        let mut sync = ClipboardSync::new();
        let payload = wire::encode_text("远端内容").expect("编码");
        assert!(sync.on_remote_payload(&payload).expect("首次").is_some());
        assert!(sync.on_remote_payload(&payload).expect("重复").is_none());
        assert_eq!(sync.stats().skipped_duplicates, 1);
        assert_eq!(sync.stats().applied, 1);
    }

    /// 对端回传本机刚发出的内容 → 不写回本机（两端不来回弹）。
    #[test]
    fn peer_echo_of_local_content_is_ignored() {
        let mut sync = ClipboardSync::new();
        let payload = sync
            .on_local_text("本机内容")
            .expect("发送")
            .expect("应外发");
        assert!(sync.on_remote_payload(&payload).expect("回传").is_none());
        assert_eq!(sync.stats().skipped_echoes, 1);
        assert_eq!(sync.stats().applied, 0);
    }

    /// 超限内容被拒绝并计数，不截断、不外发。
    #[test]
    fn oversize_local_change_is_rejected() {
        let mut sync = ClipboardSync::new();
        let huge = "a".repeat(MAX_TEXT_BYTES + 1);
        let err = sync.on_local_text(&huge).expect_err("应拒绝");
        assert!(matches!(err, ClipboardError::TooLarge { .. }));
        assert_eq!(sync.stats().rejected, 1);
        assert_eq!(sync.stats().sent, 0);
        // 归一化后才超限同样拒绝：每 2 字节 CRLF 折叠为 1 字节，重复后仍超限。
        let mut crlf = String::new();
        while crlf.len() <= MAX_TEXT_BYTES * 2 {
            crlf.push_str("ab\r\n");
        }
        assert!(matches!(
            sync.on_local_text(&crlf),
            Err(ClipboardError::TooLarge { .. })
        ));
        assert_eq!(sync.stats().rejected, 2);
    }

    /// 远端内容应用后，用户复制另一段内容仍能正常外发（不误挡）。
    #[test]
    fn different_local_content_is_not_blocked() {
        let mut sync = ClipboardSync::new();
        sync.on_remote_payload(&wire::encode_text("远端").expect("编码"))
            .expect("应用");
        assert!(sync.on_local_text("新的本机内容").expect("发送").is_some());
        assert_eq!(sync.stats().sent, 1);
        assert_eq!(sync.stats().applied, 1);
    }
}
