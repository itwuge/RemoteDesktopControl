# MERGE-NOTES：ci/github-actions 合并对照

> 用途：合并 `ci/github-actions` 时按本表决定冲突文件的取舍，避免把已验证的修复改回去。
> 本文只记录决策与事实，不改任何代码、工作流或版本号。

## 0. 分支事实（合并前必读）

| 项 | 值 |
|---|---|
| 本分支 | `ci/github-actions` @ `9ee7436`（工作树 `C:\RDC-ci`） |
| 本分支基线 | `origin/observability` @ `d54d9dc`（远端旧点） |
| 本地 `observability` | @ `74ade21`，比 `origin/observability` **领先 12 个提交**（未推送） |
| 本地 `observability` 含 | `e976676`（E0308）、`8bbfed4`（6 条 clippy + 19 条 dead_code）、`dcdc871`（collapsible_match）等 |
| 三平台验证点 | `9ee7436`：Windows / Linux / Android 三条 workflow 全部 success |
| 版本号 | 本分支 / `master` / `observability` 根 `Cargo.toml` 均为 `0.1.1`（未改动） |

合并目标应是本地 `observability`（`74ade21` 或其后续），不要拿 `origin/observability`（`d54d9dc`）当基线，否则会丢掉那 12 个提交。

## 1. 冲突文件对照表

规则：**代码类文件取 `observability` 侧；工作流与 `packaging/linux/build_appimage.sh` 取本分支侧。**

| 文件 | 取哪侧 | 为什么取这侧 |
|---|---|---|
| `crates/rdc-capture/src/platform/linux.rs` | observability | 同一修复两边都有（`Ok(_)`、去 `unnecessary_cast`、`checked_div`、`div_ceil`），observability 侧是 `e976676`/`8bbfed4` 的修复源且走完 R-C-3；差异仅注释措辞 |
| `crates/rdc-codec/src/platform/linux.rs` | observability | `PumpHandles` 类型别名两边都有，仅注释长短不同，行为一致 |
| `crates/rdc-linux-e2e/src/display.rs` | observability | `KeyPress` 的 match guard 折叠就是 observability 的 `dcdc871`，取那边即取修复源 |
| `crates/rdc-cli/src/s3b.rs`、`s3b3.rs`、`s3b4.rs` | observability | 判据常量的 `#[cfg(target_os = "windows")]` 收敛已在 `8bbfed4`；差异只剩注释，**但需补回第 2 节的 3 处 `needless_return`** |
| `crates/rdc-linux-e2e/src/bin/s8_2_client.rs`、`s8_2_host.rs`、`s8_2_loop.rs` | **本分支** | 本分支的 `while let` + `&mut [u64]` 写法通过了真实 Linux `clippy -D warnings`；observability 侧对应版本会报 5 条 lint（`while_let_loop` ×3、`ptr_arg` ×2），逐条见下表后的注。取 observability 侧会让 Linux 测试 job 复红 |
| `crates/rdc-bridge/src/wall_service.rs` | observability | `#![cfg_attr(not(windows), allow(dead_code))]` 两侧等价（只是 `not(windows)` vs `not(target_os = "windows")`）；**但需补回第 2 节的 `let_unit_value` 修复** |
| `.github/workflows/build-windows.yml`、`build-linux.yml`、`build-android.yml` | 本分支 | 三条流水线只在本分支存在；observability 侧没有，取那边等于删掉三平台 CI |
| `.github/workflows/ci.yml`、`observability-gate.yml` | 任一侧（两边一致） | `ci.yml` 两个分支内容完全相同（未改）；`observability-gate.yml` 由 observability 引入，本分支也继承同一版本 |
| `packaging/linux/build_appimage.sh` | 本分支 | 本分支补了图标回退（`appimagetool` 找不到 `Icon=rdc` 会 `exit 1`，不是警告）与硬断言；observability 侧仍是「图标缺失不阻断」的旧逻辑，取那边 Linux 打包会复红（`master` 存量 `ci.yml` 就死在这一步） |

> **s8_2 三行的行号与 lint 名，一律以 observability 提交 `e5b7484` 版本的文件内容为准，不要拿本分支工作树的同名文件对账**——两个版本文件内容不同，行号必然不同。
> 已按 `git show e5b7484:<path>` 逐条核对：
>
> | 位置（`e5b7484` 版） | lint | 对应代码 |
> |---|---|---|
> | `crates/rdc-linux-e2e/src/bin/s8_2_client.rs:208` | `while_let_loop` | `spawn_reader` 内 `.spawn(move \|\| loop { match protocol::read_packet(...) {...} })` |
> | `crates/rdc-linux-e2e/src/bin/s8_2_client.rs:285` | `while_let_loop` | 收包 `loop { let packet = match rx.try_recv() { Ok(p) => p, Err(_) => break }; ... }` |
> | `crates/rdc-linux-e2e/src/bin/s8_2_client.rs:382` | `ptr_arg` | `fn finish(... latencies: &mut Vec<u64>, ...)` |
> | `crates/rdc-linux-e2e/src/bin/s8_2_loop.rs:150` | `ptr_arg` | `#[cfg(target_os = "linux")] fn percentile(values: &mut Vec<u64>, pct: f64)` |
> | `crates/rdc-linux-e2e/src/bin/s8_2_host.rs:386` | `while_let_loop` | 注入线程 `loop { match protocol::read_packet(...) {...} }` |
>
> 注：同版本 `logs/linux-build-warnings-20261108.md` §2.7 把 `s8_2_loop.rs:150` 也写成 `while_let_loop`，但该行实际是 `percentile` 的 `&mut Vec<u64>` 形参，按代码应为 `ptr_arg`；此处以文件内容为准。

## 2. 合并后必须补回的 4 处（否则 Linux clippy `-D warnings` 会复红）

按你的规则代码取 observability 侧，但下面 4 处修复**只在 `ci/github-actions` 上存在**，observability 侧还没有。合并完成、冲突解完后，立即把这 4 个 hunk 补回（或这 4 行直接取本分支侧）：

1. `crates/rdc-bridge/src/wall_service.rs:346` 附近 —— `crates/rdc-bridge/src/wall_service.rs` 里 `let mut decoder = open_decoder()?;`，observability 的 `type DecoderHandle = ()`（同文件第 555 行）会让绑定变成 unit 值 → `clippy::let_unit_value`。
   修复 = 本分支 `c7eae7d`：`type DecoderHandle = UnsupportedDecoder;` + 新增 `struct UnsupportedDecoder;`（均带 `#[cfg(not(target_os = "windows"))]`）。
   证据：CI run `42456994303`、`42457650372` 的 `Linux 测试` 报的 lint 就是 `let_unit_value`。
2. `crates/rdc-cli/src/s3b.rs:61` —— `return Err("S3b-2 上屏自检只实现于 Windows".into());` → `Err(...)`（块尾表达式，去 `needless_return`）。
3. `crates/rdc-cli/src/s3b3.rs:83` —— 同上，`S3b-3` 分支。
4. `crates/rdc-cli/src/s3b4.rs:86` —— 同上，`S3b-4` 分支。

修复来源是本分支 `9ee7436`。证据：CI run `42458192207` 的 `Linux 测试` 报的 lint 就是 `needless_return`。

> 说明：4 处都是 clippy 的 warn-by-default lint，在 `-D warnings` 下直接算 error；不是风格偏好，会真的把 Linux 测试 job 打红。

## 3. 版本号：不动

- 三处根 `Cargo.toml` 当前都是 `0.1.1`，本次三平台产物因此是 `RDC-0.1.1-*`（Windows `RDC-0.1.1-setup.exe` / Linux `RDC-0.1.1-*` / Android `RDC-0.1.1-android-*-debug.apk`）。
- **CI 产物版本只取决于被构建分支的根 `Cargo.toml`**，与 workflow 文件无关：打包脚本从根 `Cargo.toml` 读版本号。
- 因此不需要在合并时改版本；合并带 `0.3.0` 的分支（如 `feat/auto-update`，根 `Cargo.toml` 已是 `0.3.0`）后重跑，产物自然变成 `0.3.0`。

## 4. master 存量 `.github/workflows/ci.yml`

### 4.1 它是什么

`master` 上的仓库原生 CI（`672a3c7` 已含），六个 job：`rust`（fmt / clippy / test / bench）、`windows`、`macos`+iOS、`linux`、`android`、`compliance`。
触发：`push` 到 `main`/`master`、`v*` tag、`pull_request`、`workflow_dispatch`。
runner 用不固定的 `ubuntu-latest` / `windows-latest` / `macos-latest`。

### 4.2 为什么要构建

`docs/10-里程碑与验收.md` §3 把「rust / windows / macos / linux / android / compliance」六个 job 作为 CI 层级定义；`ci.yml` 就是这套定义的实现，所以它一 push 到 master 就会跑。

### 4.3 失败在哪一步（run `42454831501`，master `672a3c7`）

| job | 结果 | 失败 step | 原始报错行 |
|---|---|---|---|
| `Windows package` | success | — | — |
| `Compliance` | success | — | — |
| `Rust check & test` | failure | #5 `格式检查` | `Process completed with exit code 1.`（`cargo fmt --all -- --check` 输出 352 处 diff、涉及 83 个文件，全是存量未格式化） |
| `Linux package` | failure | #7 `AppImage` | `./packaging/linux/build_appimage.sh: Permission denied` → `##[error]Process completed with exit code 126.` |
| `macOS & iOS package` | failure | #6 `macOS dmg（ad-hoc 签名）` | `./packaging/macos/build_dmg.sh: Permission denied` → `##[error]Process completed with exit code 1.` |
| `Android package` | failure | #4 `Run android-actions/setup-android@v3` | `Wrong version in preinstalled sdkmanager` → `Error: The process '/usr/local/lib/android/sdk/cmdline-tools/16.0/bin/sdkmanager' failed with exit code 1` |

性质：4 个红灯全部是**工作流/环境问题**（脚本无执行位、镜像自带的 `sdkmanager` 版本不匹配、仓库存量未 `cargo fmt`），没有一条是代码编译错误。

### 4.4 三个选项（不自行删除，等定）

**A. 删除 `ci.yml`**
- 利：master push 不再每次 4 红；不会与新三条流水线重复构建 Windows/Linux/Android。
- 弊：同时丢掉 `rust`（fmt/clippy/test/bench）、`macos`+iOS 打包、`compliance` 三块能力，这三块新流水线没有替代；`docs/10` §3 的六 job 定义失去实现。

**B. 改成 `workflow_dispatch`（只留手动触发）**
- 利：完整保留六个 job 的定义与手动可用性，master push / PR 不再必红；改动面最小（只动 `on:`）。
- 弊：默认不再有 CI 保护，回归要靠人记得手动点；若保留 `windows`/`linux`/`android` 三个 job，手动跑时仍会与新流水线重复。
- 若选 B，建议同时把 `windows`/`linux`/`android` 三个 job 从 `ci.yml` 裁掉（只留 `rust`/`macos`/`compliance`），否则同一平台有两套构建。

**C. 原样保留**
- 利：零改动，历史行为不变。
- 弊：master 每次 push 都 4 红，红灯噪声会掩盖真实回归；`ubuntu-latest`/`macos-latest` 不固定，镜像再变会引入新的不确定失败；与 `build-*.yml` 重复构建同一平台，排队与缓存互相挤占。

### 4.5 合并后的重复构建提醒

本分支三条 workflow 触发条件含 `push: branches: [master, ci/github-actions]`。合并进 master 后，一次 push 会同时触发 `ci.yml` 与 `build-windows/linux/android.yml`，Windows/Linux/Android 各构建两遍。

## 5. 操作约束（本次遵守）

- 不向 `master` 推送任何提交，不做 `force push`。
- 不改版本号，不改任何文件的可执行位（脚本一律 `bash <path>` 调用）。
- 本文档只落盘在工作树，未提交、未推送。

## 6. 教训：前一轮 CI 没报到某条 lint ≠ 那条 lint 不存在

本轮亲手踩到的坑：`faed7ba` 那轮 Linux `clippy` 日志里只出现 `collapsible_match`，我据此判定 `s8_2_client.rs` / `s8_2_host.rs` / `s8_2_loop.rs` 的 `while_let_loop`、`ptr_arg` 不存在，把这三行写成了「取 observability 侧」。实际情况是那轮编译在更早的错误处就中断了，clippy 根本没跑到这三条；修到那一步后 5 条 lint 才依次暴露（真实 Linux、`rustc 1.99.1 / clippy 0.1.99` 稳定复现）。

规则：**「某轮 CI 没报」推不出「不存在」**。要判定某条 lint/错误不存在，只有两种依据：

1. 该轮日志里目标 crate 确实跑完（各 crate 都到 `Finished`，没有被更早的错误中断）；
2. 本地在目标平台上、用同版本工具链完整跑过一次 `clippy -D warnings`，退出码 0。

两者都不满足时，只能记为「本轮未观测到」，不能写成「无问题」。同理：`cargo build` 通过不等于 `clippy -D warnings` 通过；`cargo test` 通过也不等于 clippy 通过。
