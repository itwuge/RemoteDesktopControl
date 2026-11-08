# Linux 构建 warning 清单（2026-11-08）

> 用途：记录 Ubuntu VM 上 `cargo build --workspace --all-targets`（23 条 rustc warning）与
> `cargo clippy --workspace --all-targets -- -D warnings`（6 条 clippy lint）的清单。
> 门禁使用 `clippy -D warnings`，两类都会让 CI 直接失败，但归因和修复路径不同。
>
> 状态：**6 条 clippy lint 与 19 条 dead_code 已在「Linux clippy 机械修复」批次处置**；
> Linux 复跑后其中 5 条 clippy 已销、`dead_code` 归零（build 0 warning），复跑又暴露**第 7 条**
> clippy lint（见 §2.3，已修）、`rdc-linux-e2e` 5 条（§2.7，已修）；最终一跑再暴露
> `rdc-cli` / `rdc-bridge` **4 条**（见 §2.8，已修）；剩余 4 条未标注 crate 的 rustc warning
> 待 Linux 原始日志补齐。

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

### 2.3 Linux 复跑新暴露（第 7 条 clippy lint，已修）

| # | 类型 | crate | 文件:行 | 内容 | 处置 |
|---|---|---|---|---|---|
| 26 | clippy lint | `rdc-linux-e2e` | `src/display.rs:210` | `collapsible_match`：`Event::KeyPress(key)` 分支内的 `if key.detail == 9` 可折叠进外层 `match` | 按编译器建议改为 `Event::KeyPress(key) if key.detail == 9 => { return false; }`，保留「keycode 9 在常见布局上为 Esc」注释；其他 keycode 仍落到 `_ => {}`，逻辑等价 |

### 2.4 未定位（4 条 rustc warning）

早前 Linux 线只按 crate 给出 23 条总数（`rdc-cli` 8 + `rdc-bridge` 11 + 未标注 4）。上述 19 条
即为前两类的 dead_code；剩余 **4 条未标注 crate 的 rustc warning** 尚缺原始日志，**不编造
文件:行:内容**，待 Linux 线转交原始构建日志后逐字补入。

### 2.5 Linux 复跑新暴露（第二类：Windows 不可见、Linux 才暴露）

> **第二类问题的定义**：代码位于 `#[cfg(target_os = "linux")]` / `#[cfg(unix)]` 分支（或被
> 这类 `mod` 引入的 `platform/linux.rs`），使用了裸 `debug!` / `info!` / `warn!` / `error!`，
> 但所在文件没有对应的 `use tracing::...`。Windows 宿主完全不编译这些分支，
> `cargo build/clippy -- -D warnings` 也发现不了，**只有 Linux 实编才暴露**。
> 与 §2.1/§2.2 的区别：那两类是跨平台可见的 rustc/clippy 诊断，这一类是「平台条件编译遮蔽」。

| # | 类型 | crate | 文件:行 | 内容 | 处置 |
|---|---|---|---|---|---|
| 27 | 编译错误（Linux） | `rdc-codec` | `src/platform/linux.rs:723` | `debug!(error = %error, "ffmpeg 写线程已退出，帧未写入")` 裸调用；文件无 `use tracing::debug;`，同文件其余 8 处均为 `tracing::debug!` 全限定 | 改为 `tracing::debug!(...)` 全限定，与文件既有风格一致；**未新增 `use`**。人读文案逐字不变 |

全仓同类扫描（判据：「裸宏调用」且「文件内无对应 `use tracing::<宏>`」）：除上述 1 处外
**0 命中**。`rdc-capture/src/platform/linux.rs`、`rdc-capture/src/platform/windows.rs`、
`rdc-input/src/platform/linux.rs` 中的裸宏均有文件级 `use tracing::{debug, info, warn}` 覆盖，
不构成同类问题。

验证边界：该行在 Windows 宿主不参与编译，`cargo build/clippy/test --workspace` 只能证明
「未改坏其它平台」；**必须由 Linux 线复跑实编实证**。

### 2.6 跨目标覆盖实测（Windows 宿主 → `--target x86_64-unknown-linux-gnu`）

**先明确边界：Windows 目标的 `cargo clippy --workspace --all-targets` 不编译
`#[cfg(target_os = "linux")]` / `#[cfg(unix)]` 分支**，所以「0 violations」不等于「全平台通过」。
为缩小盲区，本地（rustc 1.99.1 / clippy 0.1.99，2026-10-13）以 Linux 目标加跑了一次
`cargo clippy --all-targets -- -D warnings`：

| crate | 结果 | 说明 |
|---|---|---|
| `rdc-codec`、`rdc-input`、`rdc-capture`、`rdc-clipboard`、`rdc-decode`、`rdc-protocol`、`rdc-runtime`、`rdc-diag`、`rdc-filetransfer`、`rdc-relay`、`rdc-security`、`rdc-transport` | 通过（0 warning） | codec / input / capture 的 `platform/linux.rs` 本次被**实际编译**（type-check + clippy） |
| `rdc-linux-e2e` | 曾有 5 条 lint（已修） | 见 §2.7；跨目标实跑复现为真实问题，修复后同命令 0 warning |
| `rdc-core`、`rdc-cli`、`rdc-bridge`、`rdc-connect` | 无法运行 | 依赖链中 `ring v0.17.14` 的构建脚本需要 `x86_64-linux-gnu-gcc`，宿主缺失；`rdc-bridge` 另有 dart-sys / flutter_rust_bridge 构建依赖 |

结论：§2.5 那一行以及 `rdc-input` / `rdc-capture` 的 Linux 分支，已在 Windows 宿主用 Linux
目标**实际编译过**；其余 4 个 crate 的 Linux 分支仍只能由 Linux 线覆盖，不能算已验证。

### 2.7 跨目标新暴露的 lint（`rdc-linux-e2e`，5 条，**真实问题，已修**）

> **更正**：本节最初判断为「本机 clippy 版本漂移噪声」，该判断**已被 Linux 线复跑推翻**。
> 依据：Linux 侧 `rustc 1.99.1 / clippy 0.1.99`，与本机**版本完全相同**；5 条 lint 稳定复现、
> 行号一致。结论：它们是真问题，`-D warnings` 下会让门禁变红，与「Windows 不可见」叠加后
> 只在 Linux 目标暴露。

| 文件:行 | lint | 改法 | 为什么行为不变 |
|---|---|---|---|
| `src/bin/s8_2_client.rs:208` | `while_let_loop` | `loop { match read_packet { Ok(Some(p)) => .., Ok(None) \| Err(_) => break } }` → `while let Ok(Some(packet)) = read_packet(&mut stream) { .. }` | 原循环只对 `Ok(Some)` 执行体；`Ok(None)`（对端正常关闭）与 `Err` 本来就 `break`，在 `while let` 里同样条件不成立而退出；体内 `send` 失败仍 `break`。退出条件集合完全一致 |
| `src/bin/s8_2_client.rs:285` | `while_let_loop` | `loop { let p = match try_recv() { Ok(p) => p, Err(_) => break }; .. }` → `while let Ok(packet) = rx.try_recv() { .. }` | `try_recv` 只有 `Ok` / `Err(Empty\|Disconnected)` 两种；原逻辑任何 `Err` 都 `break`，`while let` 对两种 `Err` 都不进循环体，等价 |
| `src/bin/s8_2_client.rs:382` | `ptr_arg` | `latencies: &mut Vec<u64>` → `&mut [u64]` | 仅收窄参数类型；调用点传 `&mut Vec<u64>` 由 `DerefMut` 自动强转为切片。函数体只用到 `len()` / `iter()`，无 `Vec` 专有操作，语义不变 |
| `src/bin/s8_2_loop.rs:150` | `ptr_arg` | `percentile(values: &mut Vec<u64>, ..)` → `&mut [u64]` | 同上；`is_empty/sort_unstable/len/索引` 切片均可用。调用点是 `&mut latencies.clone()`（临时 `Vec`），强转后排序仍作用在同一临时向量上，分位数结果不变 |
| `src/bin/s8_2_host.rs:386` | `while_let_loop` | `loop { match read_packet { Ok(Some(p)) => match kind {..}, Ok(None) \| Err(_) => break } }` → `while let Ok(Some(packet)) = read_packet(&mut stream) { match packet.kind {..} }` | 同上；`KIND_BYE => break` 原样保留，`_ => {}` 继续下一轮，退出条件一致 |

验证方式（Windows 宿主、Linux 目标实编）：

```
cargo clippy -p rdc-linux-e2e --all-targets --target x86_64-unknown-linux-gnu -- -D warnings
```

- 改前（`git stash` 还原到 `e5b7484` 版本）：退出码 **101**，复现 `while let` 与 `&mut Vec` 两类错误；
- 改后：退出码 **0**、**0 warning**（同一 `CARGO_TARGET_DIR`、同一工具链 `clippy 0.1.99`）。

`rdc-linux-e2e` 的 bin 均以 `#[cfg(target_os = "linux")]` 逐项包裹，Windows 编译时是空壳，
这也是它们此前从未被本地检查覆盖、且必须在 Linux 目标下实跑的原因。

### 2.8 Linux 最终一跑新暴露（`rdc-cli` / `rdc-bridge`，4 条，已修）

> **「逐层暴露」的性质**：`cargo clippy --workspace -- -D warnings` 会在**第一个失败的 crate
> 处给出错误并让整条命令以 101 退出**，同一批次里其它 crate 的 lint 被前面的失败遮住、不会一起
> 报出来。因此本节 4 条要等到 §2.1/§2.3/§2.7 全部归零后的**最终一跑**才浮现——它们并不是新引入
> 的回归，而是此前被前序失败掩盖的既存问题（`ci/github-actions` 分支早已修好，见
> `C:\RDC-ci\MERGE-NOTES.md` 第 2 节）。判断「Linux 是否真正干净」必须等前一个 crate 清零后重跑，
> 不能只看首轮输出。

| # | 类型 | crate | 文件:行 | 内容 | 处置 |
|---|---|---|---|---|---|
| 28 | clippy lint | `rdc-cli` | `src/s3b.rs:66` | `needless_return`：非 Windows 分支里 `return Err("S3b-2 上屏自检只实现于 Windows".into());` | 去掉 `return`，改为块尾表达式 `Err(...)`；`cfg` 剥离后它即函数体尾表达式 |
| 29 | clippy lint | `rdc-cli` | `src/s3b3.rs:86` | `needless_return`：`return Err("S3b-3 只实现于 Windows（MF 硬件编码）".into());` | 同上，改块尾表达式 |
| 30 | clippy lint | `rdc-cli` | `src/s3b4.rs:89` | `needless_return`：`return Err("S3b-4 只实现于 Windows".into());` | 同上，改块尾表达式（块内 `let _unused = (role, options);` 保留） |
| 31 | clippy lint | `rdc-bridge` | `src/wall_service.rs:346` | `let_unit_value`：`let mut decoder = open_decoder()?;`，非 Windows 下 `DecoderHandle = ()` | 把占位句柄改为独立 unit-like struct `UnsupportedDecoder`（`#[cfg(not(target_os = "windows"))]`），`type DecoderHandle = UnsupportedDecoder;` |

**错误文案逐字不变**：`S3b-2 上屏自检只实现于 Windows`、`S3b-3 只实现于 Windows（MF 硬件编码）`、
`S3b-4 只实现于 Windows`、`屏幕墙预览解码目前仅支持 Windows`（`open_decoder` / `decode_frame` 两处
非 Windows 占位未动文案）。三处 `needless_return` 的返回类型仍为各自 `S3bResult<()>`，
`wall_service` 的 `open_decoder` 仍为 `Result<DecoderHandle, String>`，调用点签名未变。

**为什么第 31 条不再是 unit 值**：lint 的判据是「`let` 绑定的右值类型为 `()`」。`()` 是唯一以
`()` 为值的类型，所以 `type DecoderHandle = ()` 时 `open_decoder()?` 解包得到的就是 `()` 值，
`let mut decoder` 必然命中 `let_unit_value`。换成具名 `struct UnsupportedDecoder;` 后，
`DecoderHandle` 是一个独立类型的 unit-like 值（ADR: unit-like struct 的构造值类型是
`UnsupportedDecoder` 而非 `()`），绑定值类型不是 `()`，判据不成立。运行时行为完全不变：
`open_decoder()` 在非 Windows 下仍恒返回 `Err("屏幕墙预览解码目前仅支持 Windows")`，
`?` 立即上抛，`decoder` 分支不可达。

**验证方式与边界（如实说明）**：

1. Windows 侧 `cargo clippy -p rdc-cli -p rdc-bridge --all-targets -- -D warnings`：退出码 **0**（确认未改坏 Windows 分支）。但 Windows 目标不编译上述非 Windows 分支，这不构成对 4 条修复的证明。
2. Linux 目标实跑被构建脚本卡住，**无法完成**：
   ```
   cargo clippy -p rdc-cli --all-targets --target x86_64-unknown-linux-gnu -- -D warnings
   → error occurred in cc-rs: failed to find tool "x86_64-linux-gnu-gcc"（ring v0.17.14）
   cargo clippy -p rdc-bridge --all-targets --target x86_64-unknown-linux-gnu -- -D warnings
   → 同上（ring）+ dart-sys v4.1.5
   ```
   宿主无 `x86_64-linux-gnu-gcc`（亦无 clang / zig / WSL 可替代），两条命令均以 101 退出，**没有硬凑结论**。
3. 改用**等价片段双向实证**（独立临时 crate，非仓库代码）：把 4 处「改动前 / 改动后」写法原样复制成两套
   `cfg(feature)` 变体，在宿主与 `--target x86_64-unknown-linux-gnu` 上各跑 `cargo clippy -- -D warnings`：
   - 「改动前」（`return Err(...);` 的 cfg 块 + `type DecoderHandle = ()`）：Linux 目标 **101**，
     在 `src/lib.rs:13` 报 `needless_return`（即 cfg 剥离后的块尾 `return`）、在 `src/lib.rs:42`
     报 `let_unit_value`（`let mut decoder = open_decoder()?;`）——这两条即真实 crate 中
     66/86/89 与 346 的同一形态；探针内另加的 2 条宿主可见样本也一并报出，共 4 条；
   - 「改动后」（块尾表达式 + `UnsupportedDecoder`）：宿主 **0**、Linux 目标 **0**（同一工具链
     `rustc 1.99.1 / clippy 0.1.99`）。
   该实证只能证明「改法本身消除这两类 lint」，**不能替代 Linux 线对真实 crate 的最终复跑**；
   真实 crate 的实编仍由 Linux 线完成。

## 3. 数据缺口（仅剩 4 条）

本会话在 Windows 上复跑 `cargo build --workspace --all-targets`，结果 **0 warning**；
另尝试 `cargo check --target x86_64-unknown-linux-gnu --workspace --all-targets`，
因缺少 `x86_64-linux-gnu-gcc`（cc-rs 构建脚本失败）在早期退出，未产出任何 rustc warning。
后续按 crate 拆开跑，12 个 crate 已跨目标通过、4 个被 `ring`/dart-sys 构建脚本挡住（见 §2.6）。
因此仍未定位的 4 条只能在 Linux 原始日志中补齐。

## 4. 处理计划

- 本批已处置 25 条已定位项（6 clippy lint + 19 dead_code）。
- `rdc-linux-e2e` 的 5 条（3×`while_let_loop` + 2×`ptr_arg`）已修并跨目标验证 0（§2.7）。
- `rdc-cli` / `rdc-bridge` 的 4 条（3×`needless_return` + 1×`let_unit_value`）已修（§2.8）；
  这两个 crate 无法在宿主跨目标实跑（`ring` / dart-sys 缺 `x86_64-linux-gnu-gcc`），
  仅以等价片段双向实证，**最终由 Linux 线实编确认**。
- 验证边界：6 条 clippy 中 4 条位于 `rdc-capture/src/platform/linux.rs`、1 条位于
  `rdc-codec/src/platform/linux.rs`，这两处被 `#[cfg(target_os = "linux")]` 包住，
  **Windows 宿主不编译，改动的正确性必须由 Linux 线复跑 `cargo clippy --workspace
  --all-targets -- -D warnings` 实证**，本会话只能保证 Windows 侧未被改坏。
- 剩余 4 条待原始日志补齐后再归类排期。
- 出口条件：`cargo clippy --workspace --all-targets -- -D warnings` 三平台均为 0。

## 5. 教训（方法论）

- **「两边结果不一致」不能默认归因为版本差异。** 本次把 `rdc-linux-e2e` 的 5 条 lint 判成
  「本机工具链更新导致的漂移」，事实是 Linux 侧 `rustc 1.99.1 / clippy 0.1.99` 与本机**完全
  相同**，5 条稳定复现、行号一致——是真问题。归因前必须先做两件事：①对齐双方版本号
  （`rustc -V`、`cargo clippy -V`）；②在同一份代码上用同等命令复现，并记录退出码与行号。
  只有确认版本不同、且 lint 随版本才出现，才能提「版本漂移」。
- **「Windows 侧 0 violations」不等于「全平台通过」。** `#[cfg(target_os = "linux")]` /
  `#[cfg(unix)]` 分支在 Windows 宿主根本不参与编译；必须用 `--target
  x86_64-unknown-linux-gnu` 对相关 crate 实跑一次（见 §2.6），并明确记录哪些 crate 因构建
  脚本（`ring` / dart-sys）无法在宿主验证、只能交 Linux 线覆盖。
- **跨目标复现要能双向证明**：改动前用 `git stash` 跑出失败（101 + 具体行号），改动后用同一
  命令跑出 0；否则「通过」可能只是缓存未重编译造成的假象。
- **lint 是「逐层暴露」的，不能拿首轮输出判定全仓干净。** `cargo clippy --workspace -- -D
  warnings` 在第一个失败的 crate 处即以 101 退出，后续 crate 的 lint 被掩盖。§2.8 的 4 条直到
  §2.1/§2.3/§2.7 全部清零后的最终一跑才浮现，就是这种掩盖。**每次修完都必须重跑整套直到 0**，
  不能只验「新报的那几条」。
