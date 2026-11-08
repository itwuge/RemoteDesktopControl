# Linux 构建 warning 清单（2026-11-08）

> 用途：记录 Ubuntu VM 上 `cargo build --workspace --all-targets`（23 条 rustc warning）与
> `cargo clippy --workspace --all-targets -- -D warnings`（6 条 clippy lint）的清单。
> 门禁使用 `clippy -D warnings`，两类都会让 CI 直接失败，但归因和修复路径不同。
>
> 状态：**6 条 clippy lint 与 19 条 dead_code 已在「Linux clippy 机械修复」批次处置**；
> 剩余 4 条未标注 crate 的 rustc warning 待 Linux 原始日志补齐后再排期。

## 1. 来源与结论

- 环境：Ubuntu VM（Linux 线）
- 命令与结果：
  - `cargo build --workspace --all-targets`：退出码 0、0 error、**23 条 rustc warning**
  - `cargo clippy --workspace --all-targets -- -D warnings`：退出码 **101**、**6 条 clippy lint**
- 按来源归属：

| 来源 | 条数 | 已定位 | 待补齐 |
|---|---|---|---|
| `cargo build`（rustc warning） | 23 | 19（dead_code） | 4 |
| `cargo clippy`（clippy lint） | 6 | 6 | 0 |

## 2. 明细

> **类型必须分开记录**：`rustc warning`（`cargo build` 输出，与 clippy 共同）与 `clippy lint`
> （`cargo clippy` 专属）分开。两类在 `-D warnings` 下都会失败，但修复路径不同。

### 2.1 clippy lint（6 条，均已在批次内机械修复，未用 `#[allow]`）

| # | 类型 | crate | 文件:行 | 内容 | 处置 |
|---|---|---|---|---|---|
| 1 | clippy lint | `rdc-codec` | `src/platform/linux.rs:480` | `type_complexity`：`spawn_pumps` 返回四元组类型过长 | 提取 `type PumpHandles = (Sender<Job>, JoinHandle<()>, Receiver<RawAu>, JoinHandle<()>);`，签名改 `Result<PumpHandles, CodecError>` |
| 2 | clippy lint | `rdc-capture` | `src/platform/linux.rs:970` | `unnecessary_cast`：`((pixel >> 16) & 0xFF) as u32`，`pixel` 已是 `u32` | 去掉 `as u32`，保持 `u32` |
| 3 | clippy lint | `rdc-capture` | `src/platform/linux.rs:971` | 同上（绿通道） | 去掉 `as u32` |
| 4 | clippy lint | `rdc-capture` | `src/platform/linux.rs:972` | 同上（红通道） | 去掉 `as u32` |
| 5 | clippy lint | `rdc-capture` | `src/platform/linux.rs:1164` | `manual_checked_ops`：`if total == 0 { 0 } else { dot_clock / total }` | 改 `u64::from(mode.dot_clock).checked_div(total).unwrap_or(0) as u32`，语义等价（除零取 0） |
| 6 | clippy lint | `rdc-capture` | `src/platform/linux.rs:1251` | `manual_div_ceil`：`((bits + 31) / 32) * 4` | 改 `bits.div_ceil(32) * 4`（MSRV 1.80，`div_ceil` 自 1.73 稳定） |

### 2.2 rustc warning / dead_code（19 条，本批已处置）

**归类结论：19 条全部属于①「真有调用点，只是被条件编译遮住」，其中 cli 的 8 条同时属于③「文档/规格性质的阈值常量」。** 无一条属于②真死代码，因此没有删除任何常量或实现。

| # | 类型 | crate | 文件:行（改前行号） | 内容 | 归类 | 处置 |
|---|---|---|---|---|---|---|
| 7 | rustc warning | `rdc-cli` | `src/s3b.rs:19` | `TARGET_FPS` 未使用 | ①+③ | 加 `#[cfg(target_os = "windows")]`（仅 `windows_impl` 引用） |
| 8 | rustc warning | `rdc-cli` | `src/s3b.rs:21` | `TARGET_BITRATE` 未使用 | ①+③ | 同上 |
| 9 | rustc warning | `rdc-cli` | `src/s3b.rs:23` | `PUMP_GAP_LIMIT_MS` 未使用 | ①+③ | 同上 |
| 10 | rustc warning | `rdc-cli` | `src/s3b3.rs:17` | `PUSH_P99_LIMIT_US` 未使用 | ①+③ | 同上 |
| 11 | rustc warning | `rdc-cli` | `src/s3b3.rs:19` | `BITRATE_TOLERANCE` 未使用 | ①+③ | 同上 |
| 12 | rustc warning | `rdc-cli` | `src/s3b4.rs:17` | `HOST_PORT` 未使用 | ①+③ | 同上 |
| 13 | rustc warning | `rdc-cli` | `src/s3b4.rs:19` | `CTRL_PORT` 未使用 | ①+③ | 同上 |
| 14 | rustc warning | `rdc-cli` | `src/s3b4.rs:21` | `TARGET_BITRATE` 未使用 | ①+③ | 同上 |
| 15 | rustc warning | `rdc-bridge` | `src/wall_service.rs:33` | `CAPTURE_FPS` 未使用 | ① | 模块级 `#![cfg_attr(not(windows), allow(dead_code))]`（带理由，见下） |
| 16 | rustc warning | `rdc-bridge` | `src/wall_service.rs:35` | `CAPTURE_CHANNEL_CAP` 未使用 | ① | 同上 |
| 17 | rustc warning | `rdc-bridge` | `src/wall_service.rs:41` | `STREAM_ID` 未使用 | ① | 同上 |
| 18 | rustc warning | `rdc-bridge` | `src/wall_service.rs:43` | `TICK` 未使用 | ① | 同上 |
| 19 | rustc warning | `rdc-bridge` | `src/wall_service.rs:288` | `WallSink` 结构体未使用 | ① | 同上 |
| 20 | rustc warning | `rdc-bridge` | `src/wall_service.rs:303` | `scale_bgra_to_rgba` 未使用 | ① | 同上 |
| 21 | rustc warning | `rdc-bridge` | `src/wall_service.rs:328` | `run_pipeline` 未使用 | ① | 同上 |
| 22 | rustc warning | `rdc-bridge` | `src/wall_service.rs:478` | `frame_messages` 未使用 | ① | 同上 |
| 23 | rustc warning | `rdc-bridge` | `src/wall_service.rs:547` | `DecoderHandle`（非 Windows 占位）未使用 | ① | 同上 |
| 24 | rustc warning | `rdc-bridge` | `src/wall_service.rs:551` | `open_decoder`（非 Windows 占位）未使用 | ① | 同上 |
| 25 | rustc warning | `rdc-bridge` | `src/wall_service.rs:591` | `decode_frame`（非 Windows 占位）未使用 | ① | 同上 |

`rdc-bridge` 的 11 项只在 Windows 可达：`start()` 在其它平台直接返回「不支持」，`start_windows`
也按 `#[cfg(windows)]` 排除。这些实现与数值属规格，不删除；放行范围限定
`cfg_attr(not(windows))`，Windows 构建仍按默认严格检查。

### 2.3 未定位（4 条 rustc warning）

早前 Linux 线只按 crate 给出 23 条总数（`rdc-cli` 8 + `rdc-bridge` 11 + 未标注 4）。上述 19 条
即为前两类的 dead_code；剩余 **4 条未标注 crate 的 rustc warning** 尚缺原始日志，**不编造
文件:行:内容**，待 Linux 线转交原始构建日志后逐字补入。

## 3. 数据缺口（仅剩 4 条）

本会话在 Windows 上复跑 `cargo build --workspace --all-targets`，结果 **0 warning**；
另尝试 `cargo check --target x86_64-unknown-linux-gnu --workspace --all-targets`，
因缺少 `x86_64-linux-gnu-gcc`（cc-rs 构建脚本失败）在早期退出，未产出任何 rustc warning。
因此仍未定位的 4 条只能在 Linux 原始日志中补齐。

## 4. 处理计划

- 本批已处置 25 条已定位项（6 clippy lint + 19 dead_code）。
- 验证边界：6 条 clippy 中 4 条位于 `rdc-capture/src/platform/linux.rs`、1 条位于
  `rdc-codec/src/platform/linux.rs`，这两处被 `#[cfg(target_os = "linux")]` 包住，
  **Windows 宿主不编译，改动的正确性必须由 Linux 线复跑 `cargo clippy --workspace
  --all-targets -- -D warnings` 实证**，本会话只能保证 Windows 侧未被改坏。
- 剩余 4 条待原始日志补齐后再归类排期。
- 出口条件：`cargo clippy --workspace --all-targets -- -D warnings` 三平台均为 0。
