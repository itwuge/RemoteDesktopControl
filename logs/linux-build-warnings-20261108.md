# Linux 构建 warning 清单（2026-11-08）

> 用途：记录 Ubuntu VM 上 `cargo build --workspace --all-targets` 暴露的 23 条 warning。
> **本轮只记账、不修复**，待 R-C-3 清零后单独排一轮。
> 门禁使用 `clippy -D warnings`，这些 warning 会让 CI 直接失败。

## 1. 来源与结论

- 环境：Ubuntu VM（Linux 线）
- 命令：`cargo build --workspace --all-targets`
- 结果：退出码 0、0 error、**23 warning**
- 按 crate 归属（Linux 线给出的已知部分）：

| crate | 条数 |
|---|---|
| `rdc-cli` | 8 |
| `rdc-bridge` | 11 |
| 未标注 crate | 4（23 − 8 − 11） |
| **合计** | **23** |

## 2. 明细（文件:行:内容）

> **补全时必须标注「类型」**：`rustc warning`（`cargo build` 输出）与 `clippy lint`（`cargo clippy` 专属）
> 必须分开记录。本仓库门禁命令是 `cargo clippy --all-targets -- -D warnings`，两者都会导致失败，
> 但归因和修复路径不同，混在一起会误导排期。

| # | 类型（rustc warning / clippy lint） | crate | 文件:行 | 内容 |
|---|---|---|---|---|
| 1–8 | 待标注 | `rdc-cli` | 待 Linux 原始日志补齐 | 待补齐 |
| 9–19 | 待标注 | `rdc-bridge` | 待 Linux 原始日志补齐 | 待补齐 |
| 20–23 | 待标注 | 未标注 | 待 Linux 原始日志补齐 | 待补齐 |

## 3. 数据缺口（必须补齐）

本会话在 Windows 上复跑 `cargo build --workspace --all-targets`，结果 **0 warning**；
另尝试 `cargo check --target x86_64-unknown-linux-gnu --workspace --all-targets`，
因缺少 `x86_64-linux-gnu-gcc`（cc-rs 构建脚本失败）在早期退出，未产出任何 rustc warning。

因此**文件:行:内容**无法在宿主机复现，必须由 Linux 线提供原始构建日志后逐字补入上表；
不按 crate 计数编造行号。

## 4. 处理计划

- 本轮不修。
- R-C-3 清零后单开一轮：先按类别归类（unused import/variable、dead_code、needless_* 等），
  再逐 crate 修复，最终 `cargo clippy --workspace --all-targets -- -D warnings` 必须为 0。
