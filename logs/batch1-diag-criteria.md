# 批次 1 判据（先定义后实施）

> 定义时间：2026-11-08（实施前落盘）
> 范围：`crates/rdc-diag` 补齐 frame_trace / error / platform_windows / redact 并接线 `rdc-codec`、`rdc-bridge`。
> 纪律：判据先写死，实施后逐条填实测结果；未实测写「未验证」，通过写命令与关键输出；失败写原因，不放宽判据。

## 一、硬性完成条件（全部满足才算批次完成）

| # | 判据 | 判定命令 | 通过标准 |
|---|---|---|---|
| C1 | `cargo build` 成功 | `cargo build -p rdc-diag -p rdc-codec -p rdc-bridge` | exit 0，无 warning 视为通过；有 warning 必须记录 |
| C2 | `clippy -D warnings` 通过 | `cargo clippy -p rdc-diag -p rdc-codec -p rdc-bridge --all-targets -- -D warnings` | exit 0 |
| C3 | `RDC_LOG` 过滤生效 | `cargo test -p rdc-diag` 内 `filter::parse` 用例 + 样例程序 `RDC_LOG=debug` / 未设置 / `off` 各跑一次 | `debug` 可见 DEBUG 行；未设置无 DEBUG 行；`off` 全静默且退出码 0 |
| C4 | 重复 `init` 不 panic | `cargo test -p rdc-diag init_is_idempotent` | PASS；同一进程两次 `init` 服务名/过滤器一致 |
| C5 | 退出前 flush 不丢尾行 | `cargo test -p rdc-diag` 中 flush 用例；样例程序最后一行写入后立即退出 | 最后一行能在 stderr 完整出现，不出现截断 |
| C6 | 格式与旧 `eprintln!` 兼容 | `record.rs` 单测 + 实跑样例肉眼比对 | 行内含 `[模块]` 前缀；`时间戳 级别 [模块] 消息 key=value` 顺序固定 |
| C7 | 新增四模块具备测试 | `cargo test -p rdc-diag` | 四模块各自至少 1 个用例，且码表唯一性、脱敏、CSV 表头、HRESULT 格式化均有断言 |
| C8 | 接线生效 | `cargo build -p rdc-codec`、`cargo build -p rdc-bridge` 后检查依赖与调用点 | 两 crate 的 `Cargo.toml` 均出现 `rdc-diag`；codec 的 `CodecError::diag_code()` 可返回码；bridge 首次 FFI 调用触发 `rdc_diag::init("bridge")` |

## 二、本批禁止项

1. **禁止**批量把 `eprintln!` 换成 `tracing`（留到批次 2，避免无 subscriber 时日志静默丢失的行为回退）。
2. **禁止**修改协议、编码参数、性能路径；接线只允许新增依赖、新增函数与幂等初始化调用。
3. **禁止**把 `rdc-diag` 反向依赖任何 `rdc-*` 业务 crate。
4. **禁止**为通过门禁改判据、改基线、批量改业务代码。
5. 磁盘不足时停下报告，不自行清理其他目录。

## 三、实施结果（实施后填写）

| # | 结果 | 证据 |
|---|---|---|
| C1 | 通过 | `cargo build -p rdc-diag`（28.59s）→ exit 0；`-p rdc-codec`（49.09s）→ exit 0；`-p rdc-bridge`（1m41s）→ exit 0；三处均无 warning |
| C2 | 通过 | `cargo clippy -p rdc-diag -p rdc-codec -p rdc-bridge --all-targets -- -D warnings` → exit 0，`Finished` 无告警 |
| C3 | 通过 | `cargo run -q -p rdc-diag --example diag_probe`：`RDC_LOG=debug`→5 行含 1 条 DEBUG；未设置→4 行、DEBUG=0；`RDC_LOG=off`→0 行、exit 0 |
| C4 | 通过 | `tests/init.rs::repeated_init_is_idempotent`、`lib.rs::tests::init_is_idempotent` 均 PASS，重复 init 无 panic、服务名与过滤器一致 |
| C5 | 通过 | `RDC_LOG=trace` 共 6 行，末行为 `2026-11-07T23:48:29.609Z TRACE [capture] trace line (only with RDC_LOG=trace) event=probe.trace`，flush 后完整落地 |
| C6 | 通过 | 实测格式 `2026-11-07T23:48:28.751Z DEBUG [capture] debug line ... event=probe.debug`，即「时间戳 级别 [模块] 消息 key=value」，与旧 `eprintln!` 行首风格一致 |
| C7 | 通过 | `cargo test -p rdc-diag -p rdc-codec`：diag lib 34 passed、integration 2 passed、codec 25 passed；四模块均有断言（码表唯一性/脱敏/CSV 表头/HRESULT 格式化） |
| C8 | 通过 | 两 crate `Cargo.toml` 均含 `rdc-diag`；`CodecError::diag_code()` 覆盖 5 个变体且单测校验码在表内；bridge `bridge_status()`、`doctor_report()` 调 `ensure_diagnostics()`，构建通过 |
| C9（门禁） | 通过（红线保留） | 全量 467 条（含 GATE-B=1，实体 466），与本批开始时一致 → 本批零新增违规；基线仍 441 未上调 |
