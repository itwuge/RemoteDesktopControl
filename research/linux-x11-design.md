# Linux X11 采集与注入实现规格（被控端）

- 适用平台：**Ubuntu 22.04 / X11（Xorg）**，本文件只描述实现规格，**不包含代码，不修改 `crates/` 与 `app/`**
- 依据实测：VMware 虚拟机 `192.168.37.130`（8 vCPU Xeon E5-2666 v3 / 15 GiB / VMware SVGA II + vmwgfx / `Accelerated: no`）
- 数据来源：`logs/linux-env-report.md` 第十二章（X11 扩展、注入、分辨率）与第十四章（uinput、1080p 定参、脏区）
- 约定：**【实测】**=本环境有命令输出佐证；**【未验证】**=未测，禁止当作已知条件

## 0. 实测基准数据（实现与验收的唯一数字来源）

| 项目 | 数值 | 来源 |
|---|---|---|
| X11 扩展 | DAMAGE 1.1 / Composite 0.4 / MIT-SHM 1.2 / XTEST 2.2 / XFIXES 6.0 | 报告 12.1【实测】 |
| 刷新率 | `Virtual1` 60.00 Hz（另有 59.81 / 59.91 档） | 报告 12.4【实测】 |
| 1280×800 采集 | rawvideo 60 fps，约 **5% 单核** | 报告 12.2 / 12.5【实测】 |
| 1080p 单帧 | **8,294,400 B**（32 bpp） | 报告 14.2.2【实测】 |
| 1080p60 采集（raw） | **60.09 fps，0 丢帧，13% 单核** | 报告 14.2.2【实测】 |
| 1080p60 + x264 ultrafast | **60.17 fps，2 丢帧，129–137% 单核** | 报告 14.2.3【实测】 |
| 1080p60 + x264 veryfast | **55.13 fps，198% 单核（追不上）** | 报告 14.2.3【实测】 |
| 1080p30 + x264 ultrafast | **30.24 fps，0 丢帧，77% 单核** | 报告 14.2.3【实测】 |
| 1080p30 + x264 veryfast | 30.05 fps，**17 丢帧，135% 单核** | 报告 14.2.3【实测】 |
| 运行中改分辨率 | x11grab **不自动适配**，需重建采集 | 报告 14.2.4【实测】 |
| 静态时 Damage | **0 事件 / 0 px**；全屏动画 44–50 事件、100%；全屏滚动 62–283 事件、91–97.5%；100×100 小窗 0.5% | 报告 14.3.2【实测】 |
| GNOME 下 root window Damage | **仅 1 次初始事件，之后为 0** | 报告 14.3.2【实测】 |
| XTest 注入 | 免 sudo 可用 | 报告 12.3【实测】 |
| uinput | 加 udev 规则后 `wuge` 免 sudo 可写并注入成功 | 报告 14.1【实测】 |

---

## 1. 采集设计：XShm 双缓冲全帧采集

### 1.1 线程模型

| 线程 | 职责 | 约束 |
|---|---|---|
| `rdc-ctl` | 会话管理：`enumerate` → `set_sink` → `start` →（`reconfigure`）→ `stop` | 唯一可调用 `CaptureSource` 生命周期方法的线程 |
| `rdc-cap-{display}` | X11 采集：持有一个 `Display*`、两个 SHM 段、Damage/Xrandr 事件状态；按 `target_fps` 产帧 | **独占** X 连接（Xlib 非线程安全，禁止跨线程共享 `Display*`） |
| `rdc-enc` | 从 SPSC 环 A 取帧 → 色彩转换/编码（x264）→ 输出到网络 | 不进采集线程，不在 `on_frame` 内做任何重活 |

数据流：

```
rdc-cap-N ── FrameSink::on_frame(帧) ──> 环 A（SPSC，容量 3）──> rdc-enc ──> 环 B ──> 网络
                │
                └── 帧缓冲取自 FramePool（8 块），drop 自动归还
```

**呈现边界（清晰化）**：
- 采集侧**不做 jitter buffer、不做呈现节拍**，只按目标帧率网格产帧；
- 控制端的渲染器采用 **latest-wins**（拿到新帧就替换旧帧），呈现时刻由客户端垂直同步决定；
- 端到端时延定义：`FrameMeta.pts_ns`（采集完成）→ 编码出包 → 网络 → 客户端上屏，各段可独立打点。
- 环 A 容量固定为 **3 帧**（`RING_CAP_TO_ENC_CAP`），环满即丢帧，这一动作由 `on_frame` 实现方静默完成并计数，**禁止阻塞采集线程**（契约要求 `on_frame` ≤1ms）。

### 1.2 缓冲与内存

| 项 | 规格 | 依据 |
|---|---|---|
| SHM 段 | **2 段**，每段 `w*h*4`（1080p 各 8.29 MB，共 16.6 MB），A/B 交替使用 | 双缓冲：一段交给下游的瞬间，下一帧写入另一段 |
| 帧池 | `POOL_BLOCKS = 8`，单块按当前模式 BGRA8 整帧 | 1080p 约 66.3 MB；1280×800 约 32.7 MB |
| 每帧拷贝次数 | SHM 段 → 帧池 **1 次** memcpy | XShmGetImage 已由服务端完成一次拷贝，无法省掉这次落池拷贝（除非帧池本身由 shm 段支撑，见 1.6 可选优化） |
| 总量 | 1080p ≈ **83 MB**（66.3 + 16.6） | 与 Windows 侧同量级，可直接沿用 `FramePool` 参数 |

**SHM 段尺寸固定**，分辨率变化必须销毁重建（见第 2 节）。

### 1.3 采集循环（伪代码级规格）

```
安装 XRandr 事件掩码 → 进入循环：
  deadline += 1/target_fps（绝对网格，非相对 sleep，避免累积漂移）
  1) 收事件：XPending + 处理 Damage / RRScreenChangeNotify / ConfigureNotify（非阻塞）
  2) XShmGetImage(dpy, root, 段[n], 0, 0, ZPixmap)   ← 整屏一次取，禁止分块拼接
  3) ts = CLOCK_MONOTONIC 读数；memcpy 段[n] → FramePool 块
  4) 填 Frame：format=Bgra8（X11 的 ZPixmap 是 BGRA 小端）、width/height、
     pts_ns/qpc_us（同源单调时钟）、frame_id++、capture_cost_us、interval_us
  5) sink.on_frame(frame)
  6) n = 1 - n（切换 SHM 段）
  7) clock_nanosleep(ABSTIME, deadline)；若落后 >1 个帧间隔，跳到下一网格点并计一次丢帧
```

**必须遵守的三条硬约束**：
1. **整屏一次 `XShmGetImage`**。任何“分矩形抓取后拼帧”的做法都会在矩形边界产生**撕裂**，因为各次请求之间有刷新。
2. **一段 SHM 在编码侧未释放前不得复用**。环 A 深度 3 + 双 SHM 段已能保证；实现上以“段空闲标记”而非时间推断。
3. **禁止在采集线程内做色彩转换**。x264 可直接吃 `bgr0`，转换放在编码线程（实测 CPU 数字即按此路径测得）。

### 1.4 帧时间戳：取“采集完成时刻”，不取刷新节拍

**结论：`pts_ns = XShmGetImage 返回后的 CLOCK_MONOTONIC`**，`qpc_us = pts_ns / 1000`（字段名沿用 Windows，语义为“同源单调时钟微秒”）。

理由：
- 本环境**没有可用的 vblank 时间戳**：DRI3/Present 扩展未在本机验证；VMware SVGA 不提供可靠的 vblank 事件。**【未验证】**
- 若用“理论刷新网格”代替真实完成时刻，会掩盖采集侧卡顿，导致时延统计失真。
- 帧间节拍用**绝对网格**（`deadline = t0 + n*T`）保证长期平均帧率不漂移；时间戳仍记录真实完成时刻。
- `interval_us` 填真实相邻帧差值，`capture_cost_us` 填“GetImage 到 memcpy 完成”的耗时，两者用于 p50/p99 统计（沿用 `CaptureStats`）。

**撕裂的判定与防线**：
- X 服务端对像素拷贝是串行的，单次 `XShmGetImage` 得到的是**某一时刻的一致快照**，本身不撕裂；
- 撕裂实际来自三处，逐一封堵：
  1. 分块抓取拼接 → **禁止**（上条）；
  2. SHM 段被下一帧覆写时下游仍在读 → 双段交替 + 环深度限制（上条）；
  3. 光标单独合成的时间差 → 光标**不随帧抓取**（XShmGetImage 不包含硬件光标），需单独合成，见 1.5；若发现光标与画面错位 >1 帧，改成独立光标通道（客户端本地绘制）。

### 1.5 光标（`include_cursor`）

- `XShmGetImage(root)` **不包含光标**（光标是叠加层）。
- 实现方式：每帧 `XFixesGetCursorImage` + `XQueryPointer` 取位置，按 `root_x - hotspot_x/y` 叠到帧缓冲；开销为一次小图合成，**增量成本【未验证】**（1080p 预计 <1% 单核）。
- 采样顺序：**先取光标位置与图像，再 `XShmGetImage`**，保证光标不会“跳”到画面未来位置。
- 若客户端已有本地光标渲染（推荐），则 `include_cursor=false`，避免双光标与额外合成成本。

### 1.6 CPU 预算（两条预设，基于实测）

| 预设 | 采集（实测） | 编码（实测） | 合计 | 占 8 vCPU | 判定 |
|---|---|---|---|---|---|
| **1080p30 + x264 `ultrafast`** | 0.13 核 | 0.77 核 | **≈0.90 核** | 11% | **默认预设** |
| **1080p60 + x264 `ultrafast`** | 0.13 核 | 1.29–1.37 核 | **≈1.50 核** | 19% | **性能上限预设** |
| 1080p60 + `veryfast` | 0.13 核 | 1.98 核（仅 55 fps） | 超预算且不达标 | — | **禁止作为默认** |
| 1080p30 + `veryfast` | 0.13 核 | 1.35 核（17 丢帧） | ≈1.48 核 | — | 不推荐（比 60fps ultrafast 还贵） |
| 1280×800@60 + `ultrafast` | ≈0.05 核 | ≈0.5 核【推断】 | ≈0.6 核 | — | 低配回退 |

预算余量说明：
- 上述采集数字已含 ffmpeg 自身的“SHM → 帧缓冲”一次拷贝，与本规格的 1 次 memcpy 同构；
- 本实现额外增加 **光标合成（未测）** 与 **Damage 并集计算（未测，静态时几乎为 0）**，默认预设预留 **+0.10 核** 安全量，即 **1080p30 预算 1.0 核 / 1080p60 预算 1.6 核**；
- 编码器线程为多线程（x264 默认按 slice/frame 并行），上述“核”是跨核合计，不是单线程占用。

**可选优化（仅在测出瓶颈后做）**：把 `FramePool` 的块直接由 SHM 段支撑（`shmat` 内存交给池），可省掉 1 次整帧 memcpy，理论上省 2–3% 单核（1080p）。**【未验证】**，不作为第一阶段要求。

---

## 2. 尺寸变化：监听、重建与防抖

### 2.1 事件监听

```
XRRSelectInput(dpy, root, RRScreenChangeNotifyMask
                        | RRCrtcChangeNotifyMask
                        | RROutputChangeNotifyMask)
```

- 事件到达：先 `XRRUpdateConfiguration(event)`，再进入**防抖判定**（不要立刻重建）。
- 同时处理 `ConfigureNotify`（root 尺寸变化）作为兜底信号。

### 2.2 尺寸读取：三重校验

VMware 存在**视图自适应虚报**：切换后立即读 `xrandr` 得到 `1736x985`，**2 秒后**才是 `1920x1080`【实测】。因此：

| 校验源 | 用途 | 说明 |
|---|---|---|
| `XGetGeometry(root)` | **主判据**：服务端实际使用的 drawable 尺寸 | 用于建立 SHM 段与帧缓冲 |
| `XRRGetScreenResourcesCurrent` 的 crtc mode | 交叉校验目标模式 | 可能瞬时滞后，仅作参考 |
| `xrandr` 文本输出 | 日志留痕 | 不作为判据 |

**判定规则**：以 `XGetGeometry(root)` 为准；若它与目标模式不一致，视为“尚未稳定”，继续等待，不得按虚报值建缓冲。

### 2.3 防抖窗口与重建触发

```
状态机：Stable --(事件)--> Debouncing --(静默 Tquiet + 稳定 Tsettle)--> Rebuilding --> Stable
```

| 参数 | 取值 | 理由 |
|---|---|---|
| `T_quiet` | **300 ms** | 用户拖动分辨率滑块会连续产生事件，静默期内每来一个事件就重置计时 |
| `T_settle` | **1700 ms** | VMware 实测需 ≥2 s 才稳定；300+1700 覆盖最坏情况 |
| 判定次数 | 稳定后连续 **2 次**读取（间隔 200 ms）尺寸一致才重建 | 防单次抖动误判 |
| 重建冷却 | 同一会话 **10 s 内最多 3 次**重建 | 防事件风暴导致反复 stop/start |
| 超限处理 | 第 4 次起**只记录日志、维持旧尺寸**，并把 `reconfigure` 失败上报 `CaptureError::Internal` | 宁可画质错位，不可无限重启 |

**重建动作（一次完成，不可拆开对外暴露中间态）**：
1. 置退出令牌 → `join` 采集线程（有界，最长 1 帧间隔）；
2. 销毁旧 SHM 段（`shmdt` + `shmctl IPC_RMID`）、旧帧池；
3. 按新 `w*h` 建 2 个 SHM 段 + 8 块帧池；
4. 重建编码器（x264 的分辨率在 `open` 时固定，必须重开）；
5. 重启采集线程，`frame_id` 继续递增（不重置，便于跨重建的因果校验）；
6. 通过协议向控制端发送“流几何变更”，客户端重建解码器与 `Viewport`。

**对外契约**：`CaptureSource::reconfigure` 已明确“短阻塞、期间采集线程暂停、旧帧丢弃”，与本流程一致；`stop()` + `start()` 的组合即 Windows 侧同一策略（`windows.rs::reconfigure` 也是 stop→start），行为对齐。

### 2.4 启动时双读校验

启动顺序（必须）：
1. `enumerate()` 读取当前尺寸；
2. 立即再读一次 `XGetGeometry`；
3. **等待 2 s，第三次读取**；
4. 三次取“最后两次一致的值”建缓冲；若第三次与第二次不同，再等 1 s 读第四次（最多重试 3 次），仍不稳定则按当前值启动并在日志标注 `unstable-size`。

采集期间同样按 2.3 处理运行中变化；**不得假设分辨率不变**（实测 x11grab 不会自适应，必须显式重建）。

---

## 3. 脏区路线（可选，第二阶段优化）

### 3.1 为什么必须按窗口建 Damage

【实测】在 GNOME（合成型桌面）下，对 **root window** 建 Damage 只收到 **1 次初始事件**，之后恒为 0；实际重绘发生在子窗口/合成器。
→ **必须逐个顶层窗口建 Damage**，或走 XComposite 重定向后按窗口采集。此结论直接决定实现方式，不可省略。

### 3.2 实现步骤

1. `XQueryTree(root)` 取全部顶层子窗口；对每个窗口：
   - 过滤 `XGetWindowAttributes.class == InputOutput` 且 `map_state == IsViewable`；
   - **安装 `XSetErrorHandler`** 忽略 `BadMatch`（窗口在枚举与建 Damage 之间被销毁时会报错，直接中止进程是实测踩过的坑）；
   - `XDamageCreate(dpy, win, XDamageReportRawRectangles)`；
   - 记录窗口的屏幕矩形：`XTranslateCoordinates(win → root, 0,0)` 得到原点 `(wx, wy)`，尺寸取 `width/height`。
2. 监听 `DestroyNotify` / `UnmapNotify` / `ReparentNotify`：销毁对应 Damage 并从表中移除。
3. 每个采集节拍：
   ```
   XDamageSubtract(dpy, dmg, None, frame_region)   // 自上次 subtract 以来的累积
   XFixesFetchRegion(dpy, frame_region, &nrects)   // 取矩形列表
   每个矩形 → 平移 (wx, wy) → 裁到 root 边界 → XFixesUnionRegion 并入 total_region
   ```
   **必须用“并集”而不是“面积求和”**：实测按矩形求和会重复计数，出现 >100% 的荒谬值。
4. 得到本帧脏区并集面积 `dirty_px`；`dirty_ratio = dirty_px / (W*H)`。

### 3.3 与全帧采样的切换阈值

| 条件 | 动作 |
|---|---|
| `dirty_ratio == 0` 连续 ≥1 帧 | 整帧跳过：不抓屏、不编码、不发包（仅发心跳），采集线程直接进入下一节拍 |
| `dirty_ratio < 0.40` 且矩形数 ≤ 8 | 走脏区路径：按并集外接矩形（或矩形列表）抓取/编码 |
| `dirty_ratio ≥ 0.40` 或矩形数 > 8 | **升格为全帧**（编码器对小矩形碎片开销更大，实测 91–97.5% 的滚动场景接近全屏） |
| 连续 10 帧 `dirty_ratio > 0.60` | 关闭 Damage 监听 5 s（纯全帧模式），期满重估 |
| 连续 60 帧 `dirty_ratio == 0` | 每 500 ms 才做一次 `XShmGetImage` 校验（防漏事件，如光标移动） |

与既有代码的衔接：`Frame::is_full_frame()` 的阈值是 **50%**；建议脏区实现内部用 **40%** 升格、并在 `dirty_rects` 为空时按整帧处理，两者不冲突（`is_full_frame` 只是下游判断，不改变采集行为）。

### 3.4 明确边界

- **有效**：静态桌面、打字、看文档、单窗口小范围动画——可省 **≈99–100%**。
- **无效**：全屏视频、全屏滚动——实测脏区仍占 **91–100%**，几乎没有收益。
- **额外复杂度**：窗口生命周期、层叠顺序、遮挡、光标（光标移动不一定产生 Damage，需独立通道处理）。
- **定位**：第二阶段可选项。第一阶段按全帧实现（1080p30/60 已实测可跑），不要把脏区纳入首发验收门槛。

---

## 4. 注入设计

### 4.1 主路径：XTest（绝对定位 + 按键）

| 事件 | 调用 | 说明 |
|---|---|---|
| 指针绝对定位 | `XTestFakeMotionEvent(dpy, screen_no, X, Y, 0)` | 坐标为 **root 像素**，不是 0..65535 归一化值 |
| 按键 | `XTestFakeButtonEvent(dpy, button, is_press, 0)` | 左/中/右 = 1/2/3 |
| 滚轮 | 同上，垂直 = 4（上）/5（下），水平 = 6（左）/7（右） | 每次“刻度”= 一次 press+release；`delta_y` 的每个单位发一对 |
| 键盘 | `XTestFakeKeyEvent(dpy, keycode, is_press, 0)` | 需把 `KeyEvent::scan_code`（PC/AT Set 1）映射为 X keycode |
| 提交 | `XFlush(dpy)` | 合并同一批事件后 flush 一次；纯移动事件按 4 ms 合并 |

- `screen_no` 固定为 0（Xinerama/RandR 下 screen 0 的 root 覆盖整个虚拟桌面）。
- 已知前提【实测】：XTest 免 sudo 可用（`xdotool` 移动并读回坐标成功），无需任何组/udev 改造。
- 精度：XTest 是**线性、无指针加速**的，映射后误差应 ≤1 px。

### 4.2 坐标系与映射公式（客户端画面 → X11 root）

输入：`ViewTransform`（已有实现，`crates/rdc-input/src/mapper.rs`）——视图矩形、视频内容矩形（含 letterbox）、桌面矩形。

```
已知：
  view:   (0,0) 到 (view_w, view_h)              // 控制端视图逻辑像素
  video:  (video_x, video_y, video_w, video_h)   // fit() 算出的内容矩形，含黑边偏移
  desk:   (desk_x, desk_y, desk_w, desk_h)       // 被控端桌面物理像素

1) 黑边判定（落在黑边 → 丢弃事件，绝不注入错坐标）：
   if vx ∉ [video_x, video_x+video_w] or vy ∉ [video_y, video_y+video_h] → drop

2) 归一化：
   u = (vx - video_x) / video_w ∈ [0,1]
   v = (vy - video_y) / video_h ∈ [0,1]

3) 缩放到屏幕物理像素（闭区间，右/下边界取最后一个像素）：
   X = desk_x + round(u * (desk_w - 1))
   Y = desk_y + round(v * (desk_h - 1))

4) 钳制：
   X ∈ [desk_x, desk_x + desk_w - 1]，Y ∈ [desk_y, desk_y + desk_h - 1]
```

`ViewTransform::fit()` 已经实现 letterbox 居中、DPI 比值与多屏原点（Windows 语义下原点可为负）。**Linux 侧必须做两点适配**：

1. **负原点不存在**：X11 root 的坐标系原点恒为 `(0,0)`，多显示器是“放在 root 内的区域”，`desk_x/desk_y ≥ 0`。若 `Viewport` 由 X11（XRandR）构造，天然满足；**禁止把 Windows 的负原点坐标直接喂给 XTest**。
2. **`scale` 必须为 1.0**：XTest 使用物理像素，Xorg 不做 DPI 虚拟化；若把 `Viewport::scale` 再乘一次会双重缩放。已有注释也明确 `ViewTransform` 不乘 `scale`，Linux 侧 `DefaultInputMapper` 同样不要用。

**多显示器**：`desk` 为目标输出在 root 中的矩形（例如副屏 `desk_x=1920`）。若采集只覆盖该屏，映射结果仍落在该屏内，逻辑成立。跨屏拼接场景需按“root 虚拟桌面”给 `desk`，由客户端分屏渲染保持一致。

**仅有单显示器（本 VM）**：`desk = (0, 0, W, H)`，公式退化为 `X = round(u*(W-1))`。

### 4.3 备用路径：uinput（相对位移 + 加速补偿）

**为什么是相对**：uinput 若做相对指针，libinput 会施加**指针加速**——【实测】发送 25 次 `REL_X +10`（累计 250 px），实际位移 **417 px**，倍率约 1.67。直接按绝对目标发 `REL` 落点必然偏移。

**补偿策略（闭环修正）**：

```
目标 (X, Y)：
  repeat 最多 3 次：
    cur = XQueryPointer(root)                      // 真实位置
    ex  = X - cur.x; ey = Y - cur.y
    if |ex| ≤ 2 and |ey| ≤ 2 → done（误差 ≤2 px 可接受）
    step = clamp(ex, -50, 50)                      // 大步长受加速影响更大，限制单步
    emit(REL_X, step); emit(REL_Y, clamp(ey,-50,50)); SYNC
    sleep 8 ms                                     // 等 libinput → X 指针生效
  若 3 次后仍 >20 px：改发 1 px/8 ms 的微步序列（低速区加速倍率≈1），再闭环 5 次
```

要点：
- **绝不为了 uinput 去改系统指针加速设置**（会污染用户桌面）。
- 可缓存“粗定位倍率”用于首次大步，但最终必须闭环收敛到 ±2 px。
- 设备创建（【实测】必需）：`UI_SET_EVBIT` 声明 `EV_REL`、`EV_KEY`；`UI_SET_RELBIT` 声明 `REL_X/REL_Y`（滚轮再加 `REL_WHEEL/REL_HWREL`）；**必须声明 `BTN_LEFT/BTN_RIGHT/BTN_MIDDLE`**，否则 libinput 不识别为指针（实测踩坑）。
- 设备存活：`UI_DEV_CREATE` 后**常驻整个会话**，不要按事件创建/销毁（X 服务器枚举需要时间）。
- 键盘：uinput 用 `EV_KEY` + `KEY_*`（evdev 码）；X keycode 与 evdev 码的固定偏移为 `+8`，映射表以 USB HID Usage 为中间层维护，避免布局差异。

**后端选择**：`PlatformInjector::new()` 内探测顺序 = XTest（`XTestQueryExtension`）→ uinput（`open("/dev/uinput")` 成功）→ 都不可用时 `inject` 返回 `InputError::InjectionRejected`（**不得 panic**；现有 stub 的 `todo!()` 必须替换）。
可选环境变量强制后端（如 `RDC_LINUX_INPUT_BACKEND=uinput`），用于验收 §7 的备用通道测试。

### 4.4 权限与错误映射

| 场景 | 映射 |
|---|---|
| 无 X11 连接 / `DISPLAY` 缺失 / `XAUTHORITY` 不可读 | `InputError::Unsupported`（会话不可交互） |
| XTest 扩展缺失 | 尝试 uinput；仍失败 → `InjectionRejected` |
| `/dev/uinput` 打不开（EACCES/ENOENT） | `InjectionRejected`（日志附 `ls -l /dev/uinput` 状态与修复命令） |
| 事件载荷非法（未知枚举） | `MalformedPayload` |
| 视口非法 | `InvalidViewport`（应由 `mapper` 提前拦截） |

---

## 5. 平台接口落点（trait 方法清单与 Windows 差异）

### 5.1 `rdc-capture/src/platform/linux.rs` 需实现的方法

文件现为 S1 空壳（`todo!()`），实现时**只替换方法体，不改 trait 与模块结构**：

| 类型 | 方法 | 实现要点 |
|---|---|---|
| `PlatformCapture` | `new()` | 不建立连接（保持“不阻塞、不建硬件”的既有约定）；记录后端候选 |
| `CaptureSource` | `backend_name()` | 返回 `"x11-xshm"`；无 MIT-SHM 时回退 `"x11-xgetimage"`；启用脏区时 `"x11-xshm+damage"` |
| | `stats()` | 复用 `CaptureStats::snapshot(elapsed)`，语义与 Windows 一致 |
| | `enumerate()` | XRandR 输出枚举：`id`=输出序号、`name`=输出名、`width/height`=当前 mode、`refresh_hz`=由 mode dotclock 推算（60.00 → 60）、`scale`=1.0、`is_primary`=XRandR primary、`hdr`=false |
| | `start(config)` | 校验 sink、建线程、建 SHM 段与帧池（尺寸取自 `XGetGeometry` 双读校验）、安装 XRandr/Damage；失败映射到 `CaptureError` |
| | `set_sink(sink)` | 仅保存 `Arc`，与 Windows 同 |
| | `reconfigure(config)` | **stop() + start()**（与 Windows 策略一致；分辨率变化必须重建） |
| | `stop()` | 置退出令牌 → join → 销毁 SHM/Damage/Xrandr 状态；未启动为空操作 |

新增内部（非 trait）构件：`X11Session`（持 `Display*`，只在采集线程创建/销毁）、`ShmRing`、`RandrWatcher`、`DamageSet`、`settle_size()`、`union_damage()`。

**关键约束**：`Display*` 不进 `PlatformCapture`（否则结构不再 `Send`），只存在于采集线程栈内；`PlatformCapture` 只保存 `JoinHandle`、配置、统计与令牌（与 `windows.rs` 同构）。

### 5.2 `rdc-input/src/platform/linux.rs` 需实现的方法

| 类型 | 方法 | 实现要点 |
|---|---|---|
| `PlatformInjector` | `new()` | 探测 XTest/uinput，选定主后端；不做阻塞系统调用之外的重活 |
| `InputInjector` | `inject(event)` | `Pointer`：绝对定位 + 按钮状态机（用 `ButtonState` 区分“首次按下”和“拖动”）；`Key`：scan_code→keycode；`Wheel`：Button4/5/6/7 或 `REL_WHEEL`；`Touch` → 暂不支持，返回 `Unsupported` |
| | `set_viewport(viewport)` | 缓存；Linux 侧 `scale` 强制按 1.0 处理（见 4.2） |

### 5.3 Windows 实现差异对照

| 维度 | Windows（现有实现） | Linux（本规格） |
|---|---|---|
| 采集主路径 | DXGI Desktop Duplication；GDI BitBlt 回退 | XShm（`XShmGetImage`）；`XGetImage` 回退 |
| 采集后端标识 | `dxgi-desktop-duplication` / `gdi-bitblt` | `x11-xshm` / `x11-xgetimage`（+`+damage`） |
| 脏区来源 | DXGI 自带 `dirty_rects` / `move_rects` | 无；需按窗口建 XDamage（可选） |
| 光标 | DXGI 自带 pointer 形状与位置 | `XFixesGetCursorImage` + `XQueryPointer` 手工合成 |
| 时间戳 | `QPC`（`qpc_us`） | `CLOCK_MONOTONIC`（同字段，语义=单调时钟） |
| 指针注入 | `SendInput`，0..65535 归一化（`normalize_absolute`） | XTest，root 物理像素绝对坐标 |
| 多屏原点 | 可为负（虚拟桌面坐标系） | 恒 `(0,0)` 起，输出矩形都在 root 内 |
| DPI | 进程声明 per-monitor v2（`ensure_dpi_awareness`） | 无 DPI 虚拟化，`scale=1.0` |
| 键盘 | `scan_code` 直接送入 `SendInput` | `scan_code` → X keycode / evdev `KEY_*` |
| 滚轮 | `MOUSEEVENTF_WHEEL`，120 单位/刻度 | Button4/5 或 `REL_WHEEL ±1` |
| 权限 | 通常无额外权限（部分场景受 UIPI 限制） | XTest 无需权限；uinput 需 0660 + `input` 组 |
| 线程 | COM MTA，采集线程内持有 COM 对象 | 每线程独立 `Display*`，Xlib 不可跨线程 |
| 失败语义 | `DisplayBusy`（DXGI 独占） | X11 无独占概念；`DisplayBusy` 基本不会出现（保留映射） |

**建议的 crate 选型**（需接口评审后再进 `Cargo.toml`）：
- X11：`x11rb`（纯 Rust，覆盖 shm / damage / xfixes / randr / xtest），避免 `libX11` 的 unsafe 绑定；平台依赖仅加在 `[target.'cfg(target_os = "linux")'.dependencies]`，符合“平台 API 只出现在 `src/platform/*.rs`”的纪律。
- uinput：`libc`（ioctl/`shm`）或 `evdev` 的虚拟设备；`evdev` 需确认能声明 `BTN_*`（实测必需）。
- 编码：`rdc-codec` 侧需要 Linux 后端（x264/FFmpeg），**不在本文件范围内**，但采集预算以 `x264 ultrafast` 为准。

---

## 6. 已知风险

| # | 风险 | 影响 | 处置 |
|---|---|---|---|
| 1 | **无硬件编码**（`glxinfo` 报 `Accelerated: no`，VMware SVGA3D + Mesa 软件路径） | 1080p60 需 ≈1.5 核；veryfast 无法维持 60 | 默认 1080p30+ultrafast；60 fps 需显式开启并提示 CPU 占用 |
| 2 | **Wayland 后续**（当前 `GDM WaylandEnable=false`） | XShm/XTest 在 Wayland 下不可用 | 预留 PipeWire + portal + libei/RemoteDesktop 路线；本期不实现，界面需能报“不支持” |
| 3 | **权限**：uinput 依赖 udev 规则 + `input` 组；XTest 依赖 `DISPLAY`/`XAUTHORITY` | 服务化运行（systemd）时拿不到用户会话 → 注入全失败 | 以用户会话进程运行；启动自检 `XTestQueryExtension` 与 `/dev/uinput` 可读性，失败显式报错 |
| 4 | **多显示器**：X11 root 是单一 drawable，跨屏是一整块 | 按输出采集需裁剪矩形；输入坐标必须落在对应输出 | 本期只支持单输出采集；多屏按 root 整块采集并把 `Viewport` 设为整块 |
| 5 | **HiDPI / X11 缩放** | 客户端缩放的逻辑像素与物理像素混用 → 双重缩放 | 统一在 `ViewTransform` 用“物理/内容”比值；`Viewport::scale=1.0`；增加一条 125% 截图回归用例 |
| 6 | **VMware 分辨率虚报 2 s** | 按虚报值建缓冲 → 黑边/错位 | 2.2/2.4 的三重校验与双读 |
| 7 | **GNOME 合成器**：root Damage 恒 0 | 脏区方案直接失效 | 必须按顶层窗口建 Damage（3.1） |
| 8 | **光标不在帧内** | 远端看不到指针或双光标 | 明确 `include_cursor` 策略；推荐客户端本地渲染 |
| 9 | **MIT-SHM 不可用**（`/dev/shm` 过小、X server 禁用 SHM） | 回退 `XGetImage`，CPU 与延迟显著上升（**未测**） | 启动探测 `XShmQueryExtension`，回退并日志告警 |
| 10 | **Xlib 线程安全** | 跨线程共享 `Display*` 导致崩溃/丢事件 | 每线程独立连接；采集线程内完成全部 X 调用 |
| 11 | **libinput 指针加速**（uinput 路径） | 落点偏移（实测倍率 ≈1.67） | 闭环修正至 ±2 px；见 4.3 |
| 12 | **键盘布局差异** | 符号键错位 | 以 scan_code/HID Usage 为语义层，X keycode 按当前 keymap 反查；不做字符级传输 |
| 13 | **编码器缺失**（代码仓库当前无 Linux 编码后端） | 采集能跑但发不出码流 | 依赖主开发线补齐 `rdc-codec` Linux 后端；本规格只约束输入帧格式（BGRA8 / bgr0） |
| 14 | **1920×1080 以上未测** | 2560×1600 / 4096×2160 的 CPU 与 SHM 内存是线性外推 | 不承诺 4K；如需，必须重测后再定参 |

---

## 7. 验收口径建议

原则：**每条能力都要有可执行命令与阈值**，且必须区分“真机/VM 限制”。

### 7.1 采集

| # | 能力 | 判过标准 | 测法 |
|---|---|---|---|
| C1 | 1280×800@60 基线 | 10 s 内 fps ≥59，丢帧 ≤0.5%，单核 ≤20% | 真实采集实现跑 10 s；CPU 用 `/proc/<pid>/stat` 的 `utime+stime` 差分（与实测同法） |
| C2 | **1080p30 + ultrafast** | fps ≥29.5，丢帧 ≤0.5%，**单核 ≤100%** | 同上；这是默认预设，必须过 |
| C3 | **1080p60 + ultrafast** | fps ≥58，丢帧 ≤1%，**单核 ≤160%** | 同上；这是性能上限预设 |
| C4 | 帧完整性与无撕裂 | 10 帧连续静态截图中，任意两帧逐像素一致；运动中无水平错位带 | 抓 10 帧存 PNG 比对；用水平移动的竖线（xterm 滚动）目视/脚本检查 |
| C5 | 时间戳正确 | `frame_id` 单调递增；`interval_us` p50 ≈ 33.3 ms（30 fps）/ 16.7 ms（60 fps），p99 ≤ 2× 目标间隔 | 读 `CaptureSnapshot` |
| C6 | 内存上界 | 1080p 常驻增量 ≤90 MB（帧池 66.3 + SHM 16.6 + 编码器） | `smem`/`/proc/<pid>/status` 前后对比 |

### 7.2 尺寸变化

| # | 能力 | 判过标准 | 测法 |
|---|---|---|---|
| S1 | 启动虚报应对 | 启动后报告的宽高 = 最终 `xrandr` 值；2 s 内自动纠正 | 先切 1080p，立即启动采集，比对日志与 `xrandr` |
| S2 | 运行中切换 1280×800 → 1920×1080 | 新尺寸在 **≤2.5 s** 生效；重建 **恰好 1 次**；采集不中断会话 | 切分辨率；统计重建日志行数；连续抓帧不出现 exit |
| S3 | 防抖 | 快速连续切 5 次，重建次数 ≤3（10 s 冷却内） | 脚本连续 `xrandr` 切换 |
| S4 | 重建后可用 | 新尺寸下 fps 达标、`Viewport` 更新、客户端画面无黑边 | 结合 S2 + 客户端截图 |

### 7.3 注入

| # | 能力 | 判过标准 | 测法 |
|---|---|---|---|
| I1 | XTest 绝对定位精度 | 给定 5 个目标点（四角+中心），`XQueryPointer` 读回误差 **≤2 px** | 客户端等价调用 + 探针读回（不依赖 xdotool） |
| I2 | 黑边不注入 | letterbox 区域内点击，远端坐标**不变化** | 用 4:3 视图装 16:9 内容，点击上下黑边 |
| I3 | 多屏原点 | 副屏矩形（如 `x=1920`）映射后落在副屏内 | 双屏（本 VM 无第二屏 → 用虚拟 `Viewport` 单测，标注为**单元测试级验证**） |
| I4 | 按键与滚轮 | 左/中/右、滚轮上下左右均生效；拖动不重复 DOWN | 客户端事件回放 + `xev` 记录 |
| I5 | 键盘布局 | 一组“字母+数字+Shift 组合”在远端产生正确 keycode 序列 | `xev` 比对 keycode/pressed |
| I6 | 注入延迟 | 单次 `inject` ≤1 ms；注入到指针可见变化 ≤5 ms | 微基准 + `XQueryPointer` 轮询 |
| I7 | uinput 备用通道 | 强制 `RDC_LINUX_INPUT_BACKEND=uinput` 后，I1 仍满足误差 ≤2 px（闭环生效） | 同上，强制后端 |
| I8 | 权限失败语义 | 移除 `input` 组/恢复 0600 后，uinput 路径返回 `InjectionRejected`，不 panic | 临时回滚 udev 规则后重试 |

### 7.4 脏区（第二阶段）

| # | 能力 | 判过标准 | 测法 |
|---|---|---|---|
| D1 | 静态不产帧 | 静止 60 s，编码帧数 ≤ 少量心跳（≤2/s），CPU 单核 ≤2% | 对照 14.3 的静态数据（0 事件） |
| D2 | 局部动画按面积节省 | 100×100 动画时脏区占比 ≤1%，码率显著低于全帧 | 复用 14.3.2 的 ffplay 小窗素材 |
| D3 | 全屏场景自动升格 | 全屏视频/滚动时脏区占比 ≥90%，自动切全帧，fps 不下降 | 复用 testsrc2 与 xterm 滚动素材 |
| D4 | 无重复计数 | 报告的脏区占比恒 ≤100% | 日志断言（曾出现 507745% 的重复计数 bug） |
| D5 | 窗口生命周期 | 开/关窗口 20 次不崩、不泄漏 Damage 对象 | `valgrind`/`xdotool` 压测 + 进程 RSS 观察 |

### 7.5 虚拟机环境的验收限制（必须在报告中声明）

- **无硬件编码**：所有编码数字都是 CPU 软件编码；不得用“支持硬件编码”作为验收前提。
- **分辨率上限**：本 VM 实测只到 **1920×1080@60**；1280×800 是默认。**4K、2560×1600 一律视为未验证**。
- **单显示器**：无第二块屏，多屏条目只能做单元测试级验证。
- **Wayland 不可测**：`WaylandEnable=false`，本期不验收 Wayland 路线。
- **网络带宽未测**：码率数据是在静态桌面取得的（107/59/109 kbps），**不能作为带宽验收依据**；真实动态码率需用真实桌面素材重测。
- **测量方法固定**：CPU 一律用 `/proc/<pid>/stat` 的 `utime+stime` 差分（10 s 窗口）；fps 与丢帧取自实现的 `CaptureSnapshot`（与 ffmpeg `-progress` 口径一致）。不同方法测出的数字不可混用。

---

## 附录 A：实现落地清单（按文件）

| 文件 | 动作 | 备注 |
|---|---|---|
| `crates/rdc-capture/src/platform/linux.rs` | 实现 7 个 `CaptureSource` 方法 + 内部 X11 构件 | 替换全部 `todo!()` |
| `crates/rdc-capture/Cargo.toml` | 追加 `[target.'cfg(target_os = "linux")'.dependencies]`：`x11rb`、`libc` | 不影响 Windows 构建 |
| `crates/rdc-input/src/platform/linux.rs` | 实现 2 个 `InputInjector` 方法 + XTest/uinput 后端 | 替换全部 `todo!()` |
| `crates/rdc-input/Cargo.toml` | 同上：`x11rb`、`libc` | |
| `crates/rdc-codec` | Linux 编码后端（x264） | **不在本规格范围**，由主开发线补齐 |

## 附录 B：与报告数据的对应索引

| 规格结论 | 报告章节 |
|---|---|
| 扩展存在、60 fps 基线 | 12.1 / 12.2 |
| XTest 免 sudo | 12.3 |
| 刷新率与分辨率可改 | 12.4 |
| uinput 改造与验证 | 14.1 |
| 1080p 采集与编码预算 | 14.2.2 / 14.2.3 / 14.2.5 |
| 运行中改分辨率需重建 | 14.2.4 |
| 每帧全屏拷贝 | 14.3.1 |
| Damage 事件与脏区占比 | 14.3.2 / 14.3.3 |
