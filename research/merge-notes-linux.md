# Linux 分支合并说明（S8-4 A 部分）

- 分支：`linux-x11`（工作树 `RDC-linux`），基线主干：`master` @ `357ce49`
- 合并提交：`4786f7a merge(master): 吸纳主干 S7-6 及 AMF/解码改动，保留 Linux cfg 分支`
- 修复提交：紧随其后的 `fix(capture): 合并主干后补齐 Frame.gpu 字段（Linux X11 路径恒为 None）`
- **未执行合并到主干、未推送**；合并时机由你决定。

## 1. 吸纳了主干的哪些改动（`git diff HEAD~1^1 HEAD~1`）

主干新增/修改 29 个文件：

```
M THIRD_PARTY_NOTICES.md
M crates/rdc-bridge/Cargo.toml
M crates/rdc-bridge/src/control_service.rs        # Windows 路径
M crates/rdc-bridge/src/wall_service.rs
M crates/rdc-capture/src/frame.rs                 # 新增 GPU 帧租约字段
M crates/rdc-capture/src/lib.rs
M crates/rdc-capture/src/platform/windows.rs
M crates/rdc-capture/src/source.rs
A crates/rdc-codec/shim/amf_enc.cpp
A crates/rdc-codec/shim/amf_shim.h
A crates/rdc-codec/shim/amf_spike.cpp
A crates/rdc-codec/shim/build_amf.ps1
A crates/rdc-codec/src/backend.rs
M crates/rdc-codec/src/encoder.rs
M crates/rdc-codec/src/lib.rs
M crates/rdc-codec/src/platform/mod.rs
M crates/rdc-codec/src/platform/windows.rs
A crates/rdc-codec/src/platform/windows/amf.rs
M crates/rdc-decode/build.rs
M crates/rdc-decode/shim/ffdec.c
M crates/rdc-decode/src/platform/windows/mf_decoder.rs
M crates/rdc-decode/src/video_decoder.rs
A packaging/licenses/COPYING.LGPLv2.1
A packaging/licenses/FFmpeg-LICENSE.md
A packaging/licenses/libwinpthread-COPYING.txt
M packaging/windows/build_installer.ps1
M docs/决策待办.md                                 # 主干自带改动，本分支未再编辑
```

## 2. 冲突点与解法

`git merge master` 本身**无冲突**（三方合并成功），但有三处需要人工核对语义：

| 文件 | 情况 | 处理 |
| --- | --- | --- |
| `crates/rdc-bridge/Cargo.toml` | 分支加了 Linux 目标依赖（x11/xcb/xtst/shm），主干改了 Windows 目标依赖 | 自动合并，逐段确认两侧 `[target.'cfg(...)'.dependencies]` 都在 |
| `crates/rdc-bridge/src/wall_service.rs` | 分支把 `start` 拆成「非 Windows 返回明确不支持」+ `start_windows` | 自动合并后确认 `#[cfg(not(windows))]` 桩与 Windows 实现并存 |
| `crates/rdc-codec/src/platform/mod.rs` | 主干新增 `backend.rs` 与 AMF 模块挂接 | 自动合并后确认 Linux 分支仍走 `linux-ffmpeg-x264` 后端 |
| `crates/rdc-capture/src/platform/linux.rs` | **合并期真缺陷**：主干 `Frame` 新增 `gpu` 字段，Linux 侧构造未同步 → `E0063 missing field` | 补 `gpu: None`（X11 采集像素始终在 `buffer`，无 GPU 租约） |

## 3. 合并后校验（Linux，VM）

```
cargo build  --workspace                   → RC=0
cargo test   --workspace                   → RC=0（全部 test result: ok，含 doc-tests）
cargo clippy --workspace --all-targets      → RC=0（仅 warning，无 error）
```

失败项分类：**无失败项**。clippy 仅剩 warning，其中 `crates/rdc-cli/src/s3b*.rs` 的未使用常量是 S8-2 探针遗留，
属「可清理的告警」，非缺陷；其余 warning 与主干一致。

**平台预期缺口（未验证，需在 Windows 上补跑）**：

- 本环境没有 Windows Rust 工具链，`cargo build/test/clippy` 只覆盖 Linux cfg 分支。
  主干新增的 `control_service.rs`、`platform/windows.rs`、`windows/amf.rs`、`mf_decoder.rs`、`shim/amf_enc.cpp`
  在 Linux 上被 cfg 掉，**未被编译过**。
- AMF 硬件编码、Media Foundation 解码需要 Windows + 对应 GPU，未验证。

## 4. 合并顺序建议

分支已把 `master` 的 HEAD 变成祖先，因此「合入主干」时可以直接
`git checkout master && git merge --no-ff linux-x11`（或对调分支做一次 fast-forward），**不需要 rebase 重写历史**。

建议顺序：

1. 先在 Windows 上跑 `cargo build --workspace` / `cargo test --workspace` / `cargo clippy --workspace --all-targets`，
   确认本次合并没有把 Windows 侧编译打断（尤其 `Frame.gpu` 的众构造函数）。
2. 在 Windows 客户端做一次人工回归：连接 → 画面 → 点击命中 → 双指缩放/拖动平移。
3. 再合并入主干，合并后重跑 S7/S8 的判据。

## 5. 合并后必须重跑的判据（重点：共享代码的两处修复）

S8-3 修的两处在 Windows/Linux **共用的 Dart 代码**里，合并后 Windows 必然受影响，必须重测：

| 判据 | 为什么必须重跑 |
| --- | --- |
| Windows 客户端点击命中 ≤2px | `ViewportController` 的 `Matrix4` 由「平移写错位」改为列主序正确写法。Windows 若用拉伸铺满（无 letterbox），旧代码恰好等效正确；但窗口一旦出现上下/左右留边（竖屏、非等比窗口），旧代码就会偏。改成正确写法后，Windows 侧表现应更准，**必须重新量一次**，防止回归 |
| Windows 客户端缩放/拖动平移 | `_refreshMatrix` 同源修改，`zoom(focal:)` / `pan` 的换算一起变了 |
| Windows 客户端按下-抬起同点 | `SessionPage` 新增「用户意图点抬手」。Windows 用 `SendInput` 绝对定位，不会产生指针回声，此逻辑在 Windows 上等价于原来的 `clamp` 行为，但要确认没有把拖拽末点丢掉 |
| Windows 单机回环（若有人用） | 120ms 回声窗口只对「控制端=被控端同一套指针」有效；Windows 真双机不受影响 |
| Linux 侧全部 S8-3 判据 | 合并引入 `Frame.gpu`，采集路径已改，需重跑一次回环（帧率/延迟/点击） |

## 6. 已知边界

- 主干工作区还有未提交改动（`control_service.rs`、`amf_enc.cpp`、`product/`、`research/`），本次合并只取了**已提交**的主干状态，
  未触碰主干工作区。
- `docs/` 未由本分支编辑；合并带入的 `docs/决策待办.md` 改动来自主干。
