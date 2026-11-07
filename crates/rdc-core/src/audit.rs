//! rdc-core / 审计日志
//!
//! 层级：L3 编排层
//! 用途：仅记录元数据的本地环形日志（docs/02 §3.7 审计日志）
//! 调用方：rdc-core 各决策点、rdc-bridge 只读查询
//!
//! 红线：审计字段由 Rust 定义，禁止 Dart 追加自由文本以防注入；不得含媒体内容。

use serde::{Deserialize, Serialize};

// =====** AuditEntry =====**

/// 字段级审计条目（docs/07 §4「审计日志」）。
///
/// 为什么与 [`AuditEvent`] 并存：旧结构只有类别字符串，S5-5 的授权审计需要
/// 「旧值 → 新值」可核对；新结构只扩展字段，不改旧调用方。
///
/// 红线：**不记录任何媒体内容**（画面、文件字节、剪贴板文本），只存元数据。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Unix 毫秒时间戳。
    pub ts_ms: u64,
    /// 事件类别：`grant_created` / `grant_updated` / `grant_revoked` / `takeover` /
    /// `kick` / `remote_access_stop` / `pairing` 等。
    pub kind: String,
    /// 主体：授权 ID、会话 ID 或设备指纹（调用方决定）。
    pub subject: String,
    /// 动作名（英语短标识，便于脚本断言）。
    pub action: String,
    /// 变更前值（可为空；只允许权限位/角色/状态等元数据）。
    pub old_value: Option<String>,
    /// 变更后值（可为空）。
    pub new_value: Option<String>,
    /// 结果：`allowed` / `denied` / `pending` / `ok` 等。
    pub outcome: String,
}

/// 本地审计日志（内存环形 + JSONL 落盘）。
///
/// 为什么默认上限用条数而不是字节：审计条目字段固定，条数上限更可预测；
/// 真实产品按 docs/07 的 30 天/10 MB 双上限滚动，这里先实现条数上限。
#[derive(Debug, Clone)]
pub struct AuditLog {
    /// 条目（按写入顺序）。
    entries: Vec<AuditEntry>,
    /// 最大保留条数；`0` 视为不限制（仅测试用）。
    max_entries: usize,
}

impl Default for AuditLog {
    /// 默认保留 2000 条（约等于权限页历史一屏的多页数据）。
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            max_entries: 2_000,
        }
    }
}

impl AuditLog {
    /// 构造指定上限的审计日志。
    ///
    /// # Arguments
    /// * `max_entries` - 最大条数
    ///
    /// # Returns
    /// 空日志。
    ///
    /// # Errors
    /// 不会失败。
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: Vec::new(),
            max_entries,
        }
    }

    /// 记录一条审计。
    ///
    /// # Arguments
    /// * `ts_ms` - Unix 毫秒
    /// * `kind` - 事件类别
    /// * `subject` - 主体
    /// * `action` - 动作
    /// * `old_value` - 旧值
    /// * `new_value` - 新值
    /// * `outcome` - 结果
    ///
    /// # Returns
    /// 新条目引用。
    ///
    /// # Errors
    /// 不会失败。
    pub fn record(
        &mut self,
        ts_ms: u64,
        kind: &str,
        subject: &str,
        action: &str,
        old_value: Option<&str>,
        new_value: Option<&str>,
        outcome: &str,
    ) -> &AuditEntry {
        self.entries.push(AuditEntry {
            ts_ms,
            kind: kind.to_owned(),
            subject: subject.to_owned(),
            action: action.to_owned(),
            old_value: old_value.map(str::to_owned),
            new_value: new_value.map(str::to_owned),
            outcome: outcome.to_owned(),
        });
        if self.max_entries > 0 && self.entries.len() > self.max_entries {
            let drop_count = self.entries.len() - self.max_entries;
            self.entries.drain(0..drop_count);
        }
        // 刚 push 的元素一定在末尾，索引安全。
        self.entries.last().expect("刚写入的审计条目")
    }

    /// 全部条目（时间顺序）。
    ///
    /// # Returns
    /// 只读切片。
    ///
    /// # Errors
    /// 不会失败。
    pub fn entries(&self) -> &[AuditEntry] {
        &self.entries
    }

    /// 条目数。
    ///
    /// # Returns
    /// 当前保留条数。
    ///
    /// # Errors
    /// 不会失败。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否为空。
    ///
    /// # Returns
    /// 无条目时返回 `true`。
    ///
    /// # Errors
    /// 不会失败。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 导出 JSONL（每行一个条目）。
    ///
    /// # Returns
    /// 以 `\n` 分隔的 JSON 文本；条目为空时返回空串。
    ///
    /// # Errors
    /// 序列化不会失败（结构内只有字符串与整数）。
    pub fn to_jsonl(&self) -> String {
        self.entries
            .iter()
            .map(|entry| serde_json::to_string(entry).expect("审计条目可序列化"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

// =====** AuditEvent =====**

/// 审计事件（仅元数据）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditEvent {
    /// Unix 毫秒时间戳。
    pub ts_ms: u64,
    /// 事件类别，如 `connect` / `takeover` / `kick` / `clipboard`。
    pub kind: String,
    /// 会话 ID（可为空）。
    pub session_id: Option<String>,
    /// 结果，如 `allowed` / `denied`。
    pub outcome: String,
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    /// 审计条目字段齐全，JSONL 可逐行解析。
    #[test]
    fn entries_are_serializable_and_complete() {
        let mut log = AuditLog::new(0);
        log.record(
            1_000,
            "grant_created",
            "grant-1",
            "issue",
            None,
            Some("perms=0x3"),
            "ok",
        );
        log.record(
            2_000,
            "grant_updated",
            "grant-1",
            "reduce_perms",
            Some("perms=0x3"),
            Some("perms=0x1"),
            "ok",
        );
        assert_eq!(log.len(), 2);
        let jsonl = log.to_jsonl();
        let lines: Vec<_> = jsonl.lines().collect();
        assert_eq!(lines.len(), 2);
        for line in lines {
            let value: serde_json::Value = serde_json::from_str(line).expect("JSONL 行合法");
            let object = value.as_object().expect("条目是对象");
            for key in [
                "ts_ms",
                "kind",
                "subject",
                "action",
                "old_value",
                "new_value",
                "outcome",
            ] {
                assert!(object.contains_key(key), "缺少字段 {key}");
            }
        }
        // 红线：审计结构里不得出现媒体内容字段。
        let text = log.to_jsonl();
        for forbidden in ["screenshot", "frame", "clipboard_text", "file_bytes"] {
            assert!(!text.contains(forbidden), "审计不得包含 {forbidden}");
        }
    }

    /// 超过上限时按写入顺序丢弃最旧条目。
    #[test]
    fn ring_keeps_latest() {
        let mut log = AuditLog::new(2);
        for ts in 0..3u64 {
            log.record(ts, "kick", "s", "a", None, None, "ok");
        }
        assert_eq!(log.len(), 2);
        assert_eq!(log.entries()[0].ts_ms, 1);
        assert_eq!(log.entries()[1].ts_ms, 2);
    }
}
