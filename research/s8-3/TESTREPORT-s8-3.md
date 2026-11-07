# S8-3 验收报告：Linux 能力接入 Flutter 客户端

- 分支：`linux-x11`（工作树 `RDC-linux`，未合主干、未推送）
- 判据先行文档：`logs/TESTPLAN-s8-3.md`（`logs/` 被 `.gitignore` 第 43 行忽略，故结论统一记在本文件）
- 运行环境：Ubuntu VM（**无 GPU**，Mesa llvmpipe 软件渲染 + Flutter Impeller `OpenGLESSDF` 后端），
  X11 单屏 1280×800（`Virtual1`），控制端与被控端都在同一台 VM 上。

## 结论总表

| 判据 | 结果 | 实测 |
| --- | --- | --- |
| E1 桥接接口齐全（Linux 分支真实现，不靠 cfg 删功能） | 通过 | 设备/设置/会话/采集编码/注入全部返回 `ok`；屏幕墙返回**明确的不支持**错误 |
| E2 `flutter build linux --release` 真的链上桥接库 | 通过 | `bundle/librdc_bridge.so` + `bundle/lib/libapp.so`（6,128,520 B），CMake `install` 规则安装 |
| E3 桌面应用能启动（设备页 / 设置页） | 部分通过 | 进程常驻、UI 有交互；**渲染只有部分图元**（见 E9 缺陷 1） |
| E4 回环链路能出帧（RGBA 降级通道） | 通过 | `textureId=null path=rgba-frame 1280x800`，首帧延迟 ~116–127 ms，持续 30 fps |
| E5 800p / 1080p 帧率与 CPU | 通过 | 见下表 |
| E6 点击命中 ≤2px | 通过 | 目标端 `PRESS x_root=1120 y_root=680`，绿块中心 `(1120,680)` ⇒ **偏差 0 px** |
| E7 连续 60 秒不崩 | 通过 | 15 次点击 / 62 s，进程存活、持续出帧、无 panic/异常 |
| E8 做不到的接口要「明确不支持」而非静默 | 通过 | `wall_start=err 屏幕墙缩略预览暂不支持当前平台…（接口保留）` |
| E9 诚实边界 | 已记录 | 无 GPU 渲染缺陷、Wayland/真实多显示器未验证 |

## E1 桥接层跨平台化

`rdc-bridge` 按 cfg 拆分：Windows 分支保留原实现，新增 `control_service_linux.rs`（X11 采集 → 外部 ffmpeg/libx264 编码 →
本机 TCP 回环 → 外部 ffmpeg 解码 → BGRA 转 RGBA），`wall_service.rs` 的 `start()` 在非 Windows 平台直接返回明确错误。

`crates/rdc-bridge/examples/linux_api_check.rs` 逐项调用 `api::*` 自检输出：

```
devices_load=ok
settings_load=ok settings_save=ok
session_status=ok
wall_start=err 屏幕墙缩略预览暂不支持当前平台：缩略流解码仅 Windows 已实现（接口保留）
control_start=ok 1280x800 path=rgba-frame
control_frame=ok samples=25 last_bytes=4096000
control_status=ok running=true fps=30.3 decoded=78 dropped=0 p50=118ms p99=132ms
input_start=ok
input_push=ok received=3 emitted=3 injected=3 rejected=0
input_stop=ok
```

## E2 构建

```
cargo build -p rdc-bridge --release
cd app && flutter build linux --release
→ ✓ Built build/linux/x64/release/bundle/rdc
```

`app/linux/runner/CMakeLists.txt` 增加安装规则，把 `target/release/librdc_bridge.so` 装进 bundle，
运行时 `dlopen` 走 bundle 内的 .so（不依赖 `LD_LIBRARY_PATH`）。

## E3 / E4 启动与出帧

启动入口：`app/build/linux/x64/release/bundle/rdc`，必须带 `GDK_SCALE=1 GDK_DPI_SCALE=1`
（否则 GNOME 分数缩放下 Flutter DPR 与 X11 坐标不一致，点击会整体偏移）。

出帧证据（应用自身日志，`RdcLogger`）：

- `画面通道 textureId=null path=rgba-frame 1280x800 backend=unprobed`
- `首帧 #32 1280x800 4096000B 延迟 116123us`
- 之后帧号持续递增（60s 观测内从 #4020 到 #4050，30 帧打印 61→118 条）

> Linux 无外部纹理（FRB 不提供跨平台纹理句柄），按判据允许的**降级方案**：轮询 RGBA 帧 →
> `decodeImageFromPixels` → `Image` 上屏。日志与 UI 中的 `path=rgba-frame` 即标记该降级。

## E5 帧率与 CPU（软件渲染，静态桌面）

| 分辨率 | 时长 | fps | p50 延迟 | p99 延迟 | 码率 | CPU（探针/编码/解码） | decoded / dropped |
| --- | --- | --- | --- | --- | --- | --- | --- |
| 1280×800 | 30 s | 30.00 | 117.5 ms | 128 ms | ~41 kbps | 36.9% / 44.3% / 20.6% | 887 / 0 |
| 1920×1080 | 30 s | 30（首秒 15.86） | ~137 ms | ~176 ms | ~67 kbps | 61.3% / 78.5% / 34.7% | — / 2 |

说明：都为静态桌面（码率低是正常现象）；CPU 是三个进程（采集探针 / 编码 / 解码）各自占单核百分比，
VM 无 GPU，帧率上限由「采集+软编+软解+软渲染」整条链路决定，未做插帧。

## E6 点击命中

窗口 1000×700、远端 1280×800 时，画面显示矩形 `(111.6, 148, 724.8×453)`（视口内坐标），
输入矩阵同步为 `724.8×453 + (111.6, 0)`（**结构一致**：画到哪就点在哪）。

- 点击预览镜像里绿块中心的位置（root 816,607 → 视口局部 746,385）
- 目标端实测：`PRESS t_ms=5487 x_root=1120 y_root=680 button=1`
- 目标绿块中心：`green_center=(1120,680)`
- **偏差 0 px**（判据 ≤2px）

本轮修复了两个导致点击错位的真实缺陷（都在共享的 model/controller，不是 Linux 专属分支）：

1. `ViewportController` 里 `Matrix4` 构造参数是**列主序**，原代码把 `rect.left/top` 写进第 4/8 位，
   平移实际落在 storage[3]/[7]，等价于「平移丢失」→ 有左右 letterbox 时横向必错。
2. 本机回环下 XTest 把真实指针移到远端坐标，系统把这次移动回传成 pointer move；
   客户端又把它当成用户移动再次下发，形成放大反馈（实测指针会一路跑飞）。
   现在 `SessionPage` 用 120 ms 回声窗口丢弃这类移动，并用「用户意图点」抬手，保证 press/release 落在同一点。

## E7 连续 60 秒稳定

15 次点击 / 62 s：进程存活（`./rdc` 常驻，CPU ~150–165%），帧号持续递增，日志无 panic、无 `Exception`、
无 `RDC_FRAME_ERR`（清理前日志口径），点击后指针停在注入点（未跑飞）。
说明：该 60 s 用的是「带诊断打印」的构建，随后同一功能代码已清理诊断并重建，重建后再做一次点击复验仍是 0 px。

## E9 诚实边界（未验证 / 已知缺陷）

1. **渲染缺陷（无 GPU）**：Impeller 在 llvmpipe 上表现不稳定——多数时候只有部分图元上屏
   （卡片/按钮/圆角背景缺失），视频层常为黑屏；偶尔（如 19:53 那次）能完整画出镜像桌面。
   这是 VM 软件渲染栈的问题，不是桥接或坐标逻辑；换带 GPU 的机器需复测。
   影响：E3「设备页/设置页截图」只能给出如实记录的部分渲染截图。
2. **Wayland 未验证**：全部在 X11 下完成（`xdotool`/XTest 依赖 X11）。
3. **真实多显示器未验证**：VM 只有 `Virtual1` 单屏，多屏坐标系与 XRandR 热插拔未测。
4. **单屏回环下拖拽不可用**：注入会移动真实指针，用户拖拽的位移会被注入改写；
   点击（press+release 同点）可用且命中准确。这是「控制端与被控端同一块屏幕」的固有限制，
   真实两端（不同机器）不受影响。
5. **帧率结论只对本 VM 成立**：无 GPU，未做 60fps 目标验证（目标 60fps 未达成，实测 30fps 是管线侧上限）。
