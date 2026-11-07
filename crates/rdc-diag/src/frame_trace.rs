//! ============================================================
//! rdc-diag · 逐帧因果链写出器
//!
//! 层级：L1 基础设施（rdc-diag 内部模块）。
//! 用途：以 `FrameRecord` 为唯一数据结构，同时产出 JSONL（主数据）与
//!       `frames-detail.csv`（兼容既有分析脚本，旧列保留、新列追加在末尾）。
//! 调用方：rdc-bridge 的本地管线、rdc-capture/rdc-decode 的帧发布路径（后续批次接线）。
//! 开关：`RDC_FRAME_TRACE=1` 启用；目录取 `RDC_CONTROL_LOG_DIR`，默认 `rdc-control-logs`。
//! 纪律：不改协议；远程链路关联是“本进程侧表 + pts_ns”最佳努力，缺失必须写 `frame_id=0`。
//! ============================================================

// =====** 模块导入 =====**

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::record;

// =====** 常量 =====**

/// 旧 `frames-detail.csv` 的 15 列表头，必须原样保留以兼容既有分析脚本。
pub const LEGACY_CSV_HEADER: &str = "seq,t_ms,pipeline_us,capture_us,rx_queue_us,push_us,enc_wait_us,decode_us,present_us,residual_us,drops_delta,rx_wait_us,busy_us,publish_us,submit_call_us";

/// 追加在旧列之后的新列（`FrameRecord` 的完整信息）。
pub const APPEND_CSV_HEADER: &str =
    "session_id,frame_id,stage,outcome,reason,elapsed_us,ts_qpc_us,detail";

/// 阶段名：采集。
pub const STAGE_CAPTURE: &str = "capture";
/// 阶段名：入队。
pub const STAGE_QUEUE: &str = "queue";
/// 阶段名：编码。
pub const STAGE_ENCODE: &str = "encode";
/// 阶段名：发送。
pub const STAGE_SEND: &str = "send";
/// 阶段名：接收。
pub const STAGE_RECV: &str = "recv";
/// 阶段名：解码。
pub const STAGE_DECODE: &str = "decode";
/// 阶段名：呈现。
pub const STAGE_PRESENT: &str = "present";

// =====** 数据结构 =====**

/// 一帧在某个阶段的记录；按 `frame_id` 聚合即完整因果链。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrameRecord {
    /// 会话内唯一标识。
    pub session_id: u64,
    /// 采集帧号；`0` 表示未能关联（必须同时给出 `reason`）。
    pub frame_id: u64,
    /// 阶段：`capture`/`queue`/`encode`/`send`/`recv`/`decode`/`present`。
    pub stage: String,
    /// 结果：`ok`/`drop`/`error`。
    pub outcome: String,
    /// 丢帧或失败原因；`ok` 时为空串。
    pub reason: String,
    /// 本段耗时（微秒）；入队类记录为 0。
    pub elapsed_us: u64,
    /// 本段开始时的单调时钟（微秒）。
    pub ts_qpc_us: u64,
    /// 可选旁证：包序号、码流字节、后端名等。
    pub detail: serde_json::Value,
}

impl FrameRecord {
    /// 构造一条 `ok` 记录。
    pub fn ok(session_id: u64, frame_id: u64, stage: &str, elapsed_us: u64, ts_qpc_us: u64) -> Self {
        Self {
            session_id,
            frame_id,
            stage: stage.to_string(),
            outcome: "ok".to_string(),
            reason: String::new(),
            elapsed_us,
            ts_qpc_us,
            detail: serde_json::Value::Null,
        }
    }

    /// 构造一条丢弃/失败记录；`reason` 必须非空，否则会丢掉归因能力。
    pub fn dropped(
        session_id: u64,
        frame_id: u64,
        stage: &str,
        reason: &str,
        elapsed_us: u64,
        ts_qpc_us: u64,
    ) -> Self {
        Self {
            session_id,
            frame_id,
            stage: stage.to_string(),
            outcome: "drop".to_string(),
            reason: reason.to_string(),
            elapsed_us,
            ts_qpc_us,
            detail: serde_json::Value::Null,
        }
    }
}

// =====** 写出器 =====**

/// 逐帧写出器：一个实例对应一次会话、两个文件。
pub struct FrameTrace {
    /// 主数据：`frames-<session_id>.jsonl`。
    jsonl: BufWriter<File>,
    /// 兼容视图：`frames-detail.csv`（旧列留空，新列追加）。
    csv: BufWriter<File>,
}

impl FrameTrace {
    /// `RDC_FRAME_TRACE=1` 时才启用；其余值一律关闭。
    pub fn enabled() -> bool {
        std::env::var("RDC_FRAME_TRACE").ok().as_deref() == Some("1")
    }

    /// 按环境变量打开写出器；未启用返回 `None`。
    ///
    /// 打开失败不静默：写一条 fallback 告警后返回 `None`，主流程继续。
    pub fn from_env(session_id: u64) -> Option<Self> {
        if !Self::enabled() {
            return None;
        }
        let dir = std::env::var_os("RDC_CONTROL_LOG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("rdc-control-logs"));
        match Self::open(&dir, session_id) {
            Ok(trace) => Some(trace),
            Err(error) => {
                record::write_fallback(
                    "WARN",
                    "diag",
                    &format!("frame trace disabled: {} ({})", error, dir.display()),
                );
                None
            }
        }
    }

    /// 在指定目录打开写出器，并按新会话重建文件。
    ///
    /// # Arguments
    /// * `dir` - 日志目录；不存在时创建。
    /// * `session_id` - 写入文件名，并填充每条记录的 `session_id` 字段。
    pub fn open(dir: &Path, session_id: u64) -> io::Result<Self> {
        std::fs::create_dir_all(dir)?;
        let jsonl_path = dir.join(format!("frames-{session_id}.jsonl"));
        let csv_path = dir.join("frames-detail.csv");
        let jsonl = BufWriter::new(File::create(&jsonl_path)?);
        let mut csv = BufWriter::new(File::create(&csv_path)?);
        // 表头 = 旧列 + 追加列，保证旧脚本按列名读仍可用。
        writeln!(csv, "{LEGACY_CSV_HEADER},{APPEND_CSV_HEADER}")?;
        Ok(Self { jsonl, csv })
    }

    /// 写一条记录到 JSONL 与 CSV。
    pub fn record(&mut self, record: &FrameRecord) -> io::Result<()> {
        // JSONL 是主数据，序列化失败直接返回错误，不写半行。
        serde_json::to_writer(&mut self.jsonl, record)?;
        self.jsonl.write_all(b"\n")?;
        // 旧列在本批尚未迁移：15 个空字段占位，保持列数与表头一致。
        let legacy_columns = vec![""; 15].join(",");
        writeln!(
            self.csv,
            "{legacy_columns},{},{},{},{},{},{},{},{}",
            record.session_id,
            record.frame_id,
            csv_field(&record.stage),
            csv_field(&record.outcome),
            csv_field(&record.reason),
            record.elapsed_us,
            record.ts_qpc_us,
            csv_field(&record.detail.to_string()),
        )?;
        self.csv.flush()
    }

    /// 冲刷两份文件；进程退出前必须调用。
    pub fn flush(&mut self) -> io::Result<()> {
        self.jsonl.flush()?;
        self.csv.flush()
    }
}

impl Drop for FrameTrace {
    fn drop(&mut self) {
        // guard 语义：退出时尽力冲刷；失败只能计数，不能 panic（媒体线程仍在跑）。
        if self.flush().is_err() {
            record::write_fallback("WARN", "diag", "frame trace flush failed on drop");
        }
    }
}

// =====** 聚合 =====**

/// 按 `frame_id` 聚合记录，得到每帧的因果链（升序）。
pub fn group_by_frame(records: &[FrameRecord]) -> BTreeMap<u64, Vec<&FrameRecord>> {
    let mut grouped: BTreeMap<u64, Vec<&FrameRecord>> = BTreeMap::new();
    for record in records {
        grouped.entry(record.frame_id).or_default().push(record);
    }
    grouped
}

/// CSV 字段转义：含逗号/引号/换行时加引号并把内部引号翻倍。
fn csv_field(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') || value.contains('\r') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

// =====** 单元测试 =====**

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rdc-diag-{}-{}", tag, std::process::id()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).expect("clean temp dir");
        }
        dir
    }

    #[test]
    fn csv_header_keeps_legacy_columns_then_appends() {
        let dir = temp_dir("header");
        let trace = FrameTrace::open(&dir, 7).expect("open trace");
        drop(trace);
        let text = std::fs::read_to_string(dir.join("frames-detail.csv")).expect("read csv");
        let header = text.lines().next().expect("header line");
        assert!(header.starts_with(LEGACY_CSV_HEADER), "legacy columns must stay first");
        assert!(header.ends_with(APPEND_CSV_HEADER), "new columns must be appended");
        std::fs::remove_dir_all(&dir).expect("clean temp dir");
    }

    #[test]
    fn record_writes_jsonl_and_csv_rows() {
        let dir = temp_dir("rows");
        let mut trace = FrameTrace::open(&dir, 9).expect("open trace");
        let record = FrameRecord::dropped(9, 42, STAGE_DECODE, "feed_failed,retry", 1200, 55);
        trace.record(&record).expect("write record");
        trace.flush().expect("flush");

        let jsonl = std::fs::read_to_string(dir.join("frames-9.jsonl")).expect("read jsonl");
        let parsed: FrameRecord = serde_json::from_str(jsonl.trim()).expect("jsonl roundtrip");
        assert_eq!(parsed, record);

        let csv = std::fs::read_to_string(dir.join("frames-detail.csv")).expect("read csv");
        let mut lines = csv.lines();
        let header = lines.next().expect("header line");
        let row = lines.next().expect("data row");
        // 逗号必须被转义，列数仍与表头一致（15 旧列 + 8 追加列 = 23）。
        assert!(row.contains("\"feed_failed,retry\""), "got {row}");
        assert_eq!(header.split(',').count(), 23, "got {header}");
        // 15 个旧列留空 → 行首是 15 个逗号；其余字段按追加列顺序排列。
        assert!(row.starts_with(",,,,,,,,,,,,,,"), "got {row}");
        assert!(
            row.ends_with(",9,42,decode,drop,\"feed_failed,retry\",1200,55,null"),
            "got {row}"
        );
        std::fs::remove_dir_all(&dir).expect("clean temp dir");
    }

    #[test]
    fn groups_records_by_frame_id() {
        let records = vec![
            FrameRecord::ok(1, 10, STAGE_CAPTURE, 100, 0),
            FrameRecord::ok(1, 10, STAGE_ENCODE, 200, 100),
            FrameRecord::dropped(1, 11, STAGE_QUEUE, "queue_full", 0, 300),
        ];
        let grouped = group_by_frame(&records);
        assert_eq!(grouped.len(), 2);
        assert_eq!(grouped[&10].len(), 2);
        assert_eq!(grouped[&11][0].reason, "queue_full");
    }
}
