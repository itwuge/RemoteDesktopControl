# master 侧欠账明细（rebase 继承，不由 observability 线治理）

> 生成日期：2026-11-08
> 触发原因：`observability` rebase 到 master `672a3c7` 后，门禁全量从 441 升到 466。
> 口径裁定（用户 2026-11-08）：基线**不上调**，本线口径保持 441；差额 25 条记为「master 侧欠账」，来源是 master 谱系（merge-base `45657a8` 之后 master 的改动）。
> 后续裁定（用户 2026-11-08）：这 20 条**由本线代偿**（R-L-1 的 6 条已在批次 2 随 eprintln 替换清偿，故剩 20 条），处理落在批次 D；来源记账保留，不回推 master 治理线。
> 取证方式：`scripts/gate.ps1` 全量只读扫描（无 `-UpdateBaseline`），按 `规则+文件` 与 `logs/gate-baseline.json` 的 `entries` 做计数差；行号取当前 HEAD `72163a8`，并用 `git diff 45657a8 HEAD` 的新增行区间确认归属。

## 计数口径（务必先读）

- 门禁**实际计数含环境项**：批次 A 后为 404，批次 C 后为 **382**。
- 其中 `R-C-15=1` 属**本机只有 C 盘**的客观限制项（`build.minFreeGB` 与 `checkSystemDrive` 的组合检查），**不参与治理计数**。
- 因此**治理分母 = 违规总数 − 1**：404 时口径为 403，当前 382 时口径为 **381**。
- 不为凑数下调 `baseline.allowedCount`，也不为凑数修改判据或批量改业务代码。

## 欠账清偿状态

| 规则 | 原欠账 | 现状 | 依据 |
|---|---|---|---|
| R-C-1 | 5 | **已清** | 批次 A `be9545a` |
| R-C-2 | 11 | 10 待清 / 1 已清 | `control_service.rs:1121` 由批次 C 以 gate-allow + 理由清偿；其余 10 条（api.rs 4、s5/settings.rs 6）留待批次 D |
| R-L-1 | 6 | **已清** | 批次 2 `29859ba` 替换 28 处 eprintln |
| R-N-1 | 2 | **已清** | 批次 A `be9545a` |
| R-N-4 | 2 | **已清** | 批次 A `be9545a` |

## 汇总

| 规则 | 净增 | 涉及文件数 |
|---|---|---|
| R-C-1 | 5 | 5 |
| R-C-2 | 11 | 3 |
| R-L-1 | 6 | 4 |
| R-N-1 | 2 | 2 |
| R-N-4 | 2 | 2 |
| **合计** | **25** | **16** |

## 明细

### R-C-1（非 ASCII 源码缺 UTF-8 BOM，5 条）

| # | 文件:行 | 说明 |
|---|---|---|
| 1 | `app/windows/runner/flutter_window.h:17` | 中文注释无 BOM，MSVC 会按 GBK 解析（C4819） |
| 2 | `app/windows/runner/render_fallback.cpp:22` | 同上；文件由 master 的 RDR-001 保底窗口引入 |
| 3 | `app/windows/runner/render_fallback.h:9` | 同上 |
| 4 | `app/windows/runner/utils.cpp:105` | 同上 |
| 5 | `app/windows/runner/utils.h:27` | 同上 |

### R-C-2（丢弃错误，11 条）

| # | 文件:行 | 内容 |
|---|---|---|
| 1 | `crates/rdc-bridge/src/api.rs:130` | `let _ = std::fs::create_dir_all(parent);` |
| 2 | `crates/rdc-bridge/src/api.rs:139` | `let _ = writeln!(file, "[{}] {line}", local_time_stamp());` |
| 3 | `crates/rdc-bridge/src/api.rs:1932` | `let _ = std::fs::remove_dir_all(&dir);` |
| 4 | `crates/rdc-bridge/src/api.rs:1938` | `let _ = std::fs::remove_dir_all(&dir);` |
| 5 | `crates/rdc-bridge/src/control_service.rs:1121` | `in_flight.pop_front().unwrap_or_default()` 掩盖失败 |
| 6 | `crates/rdc-core/src/s5/settings.rs:258` | `let _ = std::fs::remove_file(&temp);` |
| 7 | `crates/rdc-core/src/s5/settings.rs:263` | `let _ = std::fs::remove_file(&temp);` |
| 8 | `crates/rdc-core/src/s5/settings.rs:366` | `let _ = ...`（settings 保存/清理路径） |
| 9 | `crates/rdc-core/src/s5/settings.rs:382` | 同上 |
| 10 | `crates/rdc-core/src/s5/settings.rs:401` | 同上 |
| 11 | `crates/rdc-core/src/s5/settings.rs:466` | 同上 |

### R-L-1（绕过统一日志层，净增 6 条）

| # | 文件:行 | 说明 |
|---|---|---|
| 1 | `crates/rdc-bridge/src/control_service.rs:797` | 新增 `eprintln!` |
| 2 | `crates/rdc-bridge/src/control_service.rs:1235` | 新增 `eprintln!` |
| 3 | `crates/rdc-bridge/src/control_service.rs:1241` | 新增 `eprintln!` |
| 4 | `crates/rdc-bridge/src/control_service.rs:1251` | 新增 `eprintln!` |
| 5 | `crates/rdc-capture/src/platform/windows.rs:521` | `eprintln!("[capture-diag] init_failed ...")` |
| 6 | `crates/rdc-codec/src/platform/mod.rs:174` | `eprintln!` 直写 |
| 7 | `crates/rdc-codec/src/platform/windows.rs:211` | `eprintln!("[enc] poll_ready 失败: ...")` |

> 口径说明：该文件当前实际有 8 处 `eprintln!`，基线记 5 处；master 侧新增 4 处、同时删除 1 处，故净增为 3。上表列出全部 4 处新增行，净增只在第 1–4 项中计 3 条。

### R-N-1（缺文件头注释，2 条）

| # | 文件:行 |
|---|---|
| 1 | `app/windows/runner/render_fallback.cpp:1` |
| 2 | `app/windows/runner/render_fallback.h:1` |

### R-N-4（长文件缺组件分区标记，2 条）

| # | 文件:行 | 说明 |
|---|---|---|
| 1 | `app/lib/view/components/fields.dart:1` | 320 行无 `# =====** 组件名 =====**` 标记 |
| 2 | `app/lib/view/components/rdc_button.dart:1` | 382 行无组件分区标记 |

## 处置约定

1. 这 25 条只在 master 谱系产生，`observability` 线**不修改**对应业务代码，也不上调 `baseline.allowedCount`。
2. 全量统计的 `GATE-B` 红灯保留，作为「存在继承欠账」的显式信号，不得为了让门禁变绿而改判据、关规则或批量改业务代码。
3. 本线只保证 `-Changed` 模式（只卡本次改动引入的新违规）为绿；master 侧欠账由 master 治理线按用户排序处理。
