# S4-2 剪贴板（文本优先）判据（先落盘，后实现）

日期：2026-11-07
阶段：S4 第二块 S4-2
范围：新增 `crates/rdc-clipboard`（文本双向同步；图片/文件只留接口）
依据：docs/04 §2.1（stream 5、P1、文本 ≤1 MB、内容哈希去重、分块规则）、
      docs/10 §5（双向自动同步、CRLF/LF 转换正确、Unicode 往返一致）

## 判据

| 编号 | 判据 | 阈值 / 期望 | 数据来源 |
|------|------|-------------|----------|
| S4-2-a | 线格式往返 | 文本载荷 hash+长度+UTF-8 编码往返一致；Unicode（含代理对/emoji）逐字节一致 | `cargo test -p rdc-clipboard wire::` |
| S4-2-b | 校验 | 载荷内 hash 与实际内容不符 → `HashMismatch`；截断/超长/未知 kind → `MalformedPayload` | 同上 |
| S4-2-c | 大小上限 | >1 MiB（含归一化后）拒绝，不截断、不静默改写 | `cargo test -p rdc-clipboard sync::` |
| S4-2-d | 换行 | 线上统一 LF；Windows 写入时 LF→CRLF；读入时 CRLF→LF；Lone CR 不误改 | `cargo test -p rdc-clipboard text::` |
| S4-2-e | 去重 | 相同内容再发只跳过（dedup 计数）；对端重复内容不重复应用 | `cargo test -p rdc-clipboard sync::` |
| S4-2-f | 环路防护 | 远端应用后本机剪贴板回写同内容 → 判定为自写回环并跳过；自身已发内容对端回传同样跳过 | 同上 |
| S4-2-g | 自写识别 | Windows 端以 `GetClipboardSequenceNumber` 识别自写，序列号未变则不读 | `cargo test -p rdc-clipboard platform::` + 编译验证 |
| S4-2-h | 图片/文件接口 | kind=1/2 解码返回 `Unsupported`，不 panic；枚举已预留 | `cargo test -p rdc-clipboard wire::` |

## 不做（记待办）

- 图片/文件剪贴板：只留 kind 与错误路径，不实现采集与落盘。
- 剪贴板级 64 KB 分块（docs/04 §2.1）本轮不做：文本 ≤1 MB 由传输层分片承载；
  接入 `rdc-core` 时若走 TCP 路径需按 4 KB 帧分片另算。
- 2 MB/s 独立令牌桶不在此 crate：属 `rdc-transport` pacer 的 P1 结算，接线时补。
- 真实系统剪贴板读写在单测中**不做**（会覆盖使用者剪贴板），只验证纯函数与编译。
