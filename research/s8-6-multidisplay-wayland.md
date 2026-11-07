# 多显示器与 Wayland 探测报告 · S8-6

结论先行：
- **多显示器：部分支持**。能按显示器矩形裁剪采集，但**坐标映射缺偏移**（点击落在主屏坐标），且协议无偏移字段；RandR 虚拟显示器（`--setmonitor`）**不枚举**。
- **Wayland 原生采集：不支持**（代码只有 X11 后端，未实现 PipeWire 门户）。
- **Wayland 下的输入注入：不支持全局注入**；XTest 只能影响 Xwayland 里的 X11 客户端。
- **Xwayland 抓屏实测：未取得**（headless 探测中外部连接挂起），按代码推断只能看到 X11 客户端内容，看不到原生 Wayland 窗口。

环境：VM `192.168.37.130`（Ubuntu + GNOME 42，X11 会话），单一虚拟输出 `Virtual1 1280x800`。

---

## 1. 多显示器

### M1 枚举机制（代码事实）

- 采集端按 **RandR CRTC** 枚举，不是按 `xrandr --listmonitors` 的“monitor 对象”：
  - `crates/rdc-capture/src/platform/linux.rs#L1107` `resolve_displays()`：`randr_get_screen_resources` → 对每个 CRTC `randr_get_crtc_info`，把 `(x, y, width, height)` 收进内部 `CaptureRect`。
  - `#L242` `enumerate()` 只返回 `DisplayInfo`；`DisplayInfo`（`crates/rdc-capture/src/source.rs#L15-L32`）字段为 `id/name/width/height/refresh_hz/scale/is_primary/hdr`，**没有 x/y 偏移**。
  - `#L1176` / `#L1205`：CRTC 有有效 mode 时 push `(id, rect)`；RandR 不可用时降级为 `full_screen`（整屏一个矩形）。
- 采集裁剪本身是正确的：`start()` 按 `display_id` 命中 `rects`（`#L634`、`#L720`、`#L770`），从帧里裁出该显示器区域。

### M2 事实证据（VM）

当前布局（只读，长跑进行中未改动）：

```
$ xrandr --listmonitors
Monitors: 1
 0: +*Virtual1 1280/339x800/212+0+0  Virtual1

$ xrandr | head -2
Screen 0: minimum 1 x 1, current 1280 x 800, maximum 16384 x 16384
Virtual1 connected primary 1280x800+0+0
```

双虚拟显示器实验（S8-6 探测阶段，实验后已 `--delmonitor` 恢复为 1）：

```
$ xrandr --setmonitor S86L 640/0x800/0+0+0 none
$ xrandr --setmonitor S86R 640/0x800/0+640+0 none
$ xrandr --listmonitors
Monitors: 3
 0: +*Virtual1 1280/339x800/212+0+0  Virtual1
 1: S86L 640/0x800/0+0+0
 2: S86R 640/0x800/0+640+0
```

`rdc-serve` 的枚举结果仍只有 **1 个**：

```
id=0 name=Virtual1 1280x800
```

原因：虚拟 monitor 只是 RandR 1.5 的“命名区域”，没有对应 CRTC/mode，`resolve_displays()` 不认。

### M3 采集范围

- 单显示器时 `CONFIG` 输出 `1280x800@60`（实测）。
- 指定 `--display-id 1`（不存在）时，**仍然输出 `1280x800@60`**——这是缺陷，不是“正确回退”。代码在 `crates/rdc-linux-e2e/src/bin/rdc-serve.rs#L632-L638`：

```text
displays.iter().find(|item| item.id == cfg.display_id).or_else(|| displays.first())
```

`display_id` 不存在时静默取第一个显示器，日志没有告警。正确行为应是报错（底层 `start()` 本来会返回 `CaptureError::DisplayDisappeared`，但被这层 `or_else` 掩盖了）。

### M4 坐标映射（关键缺口）

- 注入侧 viewport 被写死为 `(0, 0, width, height)`：`crates/rdc-linux-e2e/src/bin/rdc-serve.rs#L738-L744`。
- 假设真实双屏：左屏 `1280x800+0+0`、右屏 `1280x800+1280+0`。服务选右屏采集（裁剪正确），但客户端发 `(100, 100)` 时注入器按 root 坐标算成左屏 `(100,100)`，**偏差 1280 px（整屏宽）**。
- 协议层也没有救：`CONFIG` 只有 `width/height/fps`（12 字节），客户端拿不到显示器偏移，无法自行补偿。
- 结论：**多显示器点击错位，协议不支持偏移**。这是“部分支持”的根因。

### M5 结论表

| 能力 | 结论 | 依据 |
| --- | --- | --- |
| 枚举多个真实 CRTC 输出 | **支持** | `resolve_displays()` 按 CRTC；真实双屏应能返回 2 个 `id` |
| 按 `display_id` 裁剪采集 | **支持** | `rects` 裁剪；`start()` 精确匹配 |
| 非主显示器坐标映射 | **不支持（会错位）** | viewport 原点写死 `(0,0)`；`DisplayInfo` 无偏移；`CONFIG` 无偏移 |
| RandR 虚拟 monitor（`--setmonitor`） | **不支持** | 无双屏硬件时无法用虚拟 monitor 模拟；实测不枚举 |
| `display_id` 不存在时 | **缺陷：静默回退首屏** | `rdc-serve.rs#L632-L638` |
| 混合 DPI / HDR | **不支持** | `scale` 字段恒 `1.0` 未被注入器使用；`hdr` 未参与编码 |

> 诚实边界：VM 只有一块虚拟输出，双屏结论来自**代码路径 + 虚拟 monitor 负证据**，未接真实双屏硬件实测。“非主屏偏移 1280 px”是按代码推算，不是实测值。

---

## 2. Wayland

### W1 环境事实

```
$ loginctl show-session 1 -p Type
Type=x11

$ grep WaylandEnable /etc/gdm3/custom.conf
WaylandEnable=false

$ ls -l /dev/dri
card0  renderD128
```

- 当前桌面是 **X11**（`Virtual1`，`DISPLAY=:0`），不是 Wayland。
- 采集后端只有 X11：`crates/rdc-capture/src/platform/linux.rs`（`x11rb` + XShm/XGetImage）；`platform/mod.rs` 没有 Wayland/PipeWire 分支。
- 仓库内没有任何 PipeWire/portal 实现：`grep -rn "pipewire|ScreenCast|xdg-desktop-portal" crates/` 只命中 `rdc-capture/src/error.rs#L17` 的注释/错误变体（“缺少屏幕录制系统权限”），无调用逻辑。
- 环境**具备**门户能力（可作未来实现路径，但不是现有能力）：

```
$ gdbus introspect ... /org/freedesktop/portal/desktop | grep ScreenCast
interface org.freedesktop.portal.ScreenCast
interface org.freedesktop.portal.RemoteDesktop
$ ls -l /run/user/1000/pipewire-0
srw-rw-rw- ... /run/user/1000/pipewire-0
```

### W2 实测（Xwayland 替身）

做法：`dbus-run-session` 内起 `gnome-shell --wayland --headless --wayland-display=wayland-s86`，它自带一个 rootless `Xwayland`（实测分到 `:1/:3/:5`），用其 `-auth` 文件尝试 `xdpyinfo` / `ffmpeg -f x11grab`。

结果：

- `gnome-shell --wayland --headless` 能起来，Wayland socket 存在，Xwayland 进程存在、`/tmp/.X11-unix/X1` 在监听。
- 外部进程连接 `DISPLAY=:1`（含正确 `XAUTHORITY`）**挂起**：`xdpyinfo` 6 s 无任何输出（被 `timeout` 杀掉），`xlsclients`/抓屏无法进行。
- 单独起 `Xwayland :2 -screen 1280x800` 失败：`Fatal server error: Unrecognized option: -screen`（`-screen` 是 Xvfb 参数，Xwayland 不吃）。
- 多次清理后重试（清 `/tmp/.X11-unix/X*`、`/tmp/.X*-lock`、硬杀残留 gnome-shell/Xwayland）仍无法连接。

因此：**没有取得“在 Xwayland 上跑 rdc-serve 并抓到画面”的实测数据**。这一项按“未验证”处理，不给通过结论。

### W3 能做什么 / 不能做什么（结论）

| 能力 | Wayland 下结论 | 原因 |
| --- | --- | --- |
| 用现有 X11 后端采集原生 Wayland 桌面 | **不支持** | Wayland 客户端内容不进入 Xwayland root；X11 截图 API 看不到别人的 surface |
| 采集 Xwayland 里的 X11 客户端 | **未验证（本环境连接失败）**；理论上“仅 X11 客户端可见” | rootless Xwayland 外部连接在本环境挂起；即使连通，也没有原生窗口 |
| 通过 PipeWire 门户（ScreenCast）采集 | **不支持（未实现）** | 代码无 portal/PipeWire 后端；环境有 `ScreenCast` 接口，可实现但本轮不做 |
| 全局鼠标/键盘注入 | **不支持** | XTest 只对 Xwayland 的 X11 客户端生效；Wayland 客户端不受全局 XTest 控制 |
| 仅对 Xwayland 客户端注入 | **部分支持（未实测）** | 与 X11 路径相同，但前提是能连上 Xwayland |
| 通过 `/dev/uinput` 注入 | **未验证** | 代码有 uinput 备用通道，但需要设备权限；本部署未验证，且不在本轮范围 |
| 读取全局光标/窗口位置 | **不支持** | Wayland 安全模型不提供全局坐标；portal RemoteDesktop 才有受控输入 |
| 多显示器偏移 | **同样不支持** | 与 §1 的缺口相同，且 Wayland 下连“枚举其他输出”都没有 X11 RandR 等价物 |

**一句话**：现有 `rdc-serve` 是 **X11-only**；切到 Wayland 会话后，采集与全局注入都**不支持**，需要新增 PipeWire portal 后端 + RemoteDesktop 输入路径，属于新功能，不是配置问题。

---

## 3. 与判据的对应

| 判据 | 结果 |
| --- | --- |
| M1 枚举机制 | ✅ 已写（CRTC + `DisplayInfo` 字段） |
| M2 事实证据 | ✅ 1 显示器现状 + 虚拟 monitor 实验（3 monitors 但服务只看到 1） |
| M3 采集范围 | ✅ `CONFIG` 为显示器矩形；`--display-id 1` 静默回退（缺陷） |
| M4 坐标映射 | ✅ 缺口定位到 viewport 原点写死 + 协议无偏移 |
| M5 结论 | ✅ 支持/部分支持/不支持 三档明确 |
| W1 环境事实 | ✅ X11 会话、`WaylandEnable=false`、仅 X11 后端 |
| W2 实测 | ⚠️ Xwayland 外部连接挂起，未取得抓屏数据（如实记录） |
| W3 结论 | ✅ 不支持项逐条给出原因 |

## 4. 复现命令

```bash
# 多显示器现状
DISPLAY=:0 XAUTHORITY=/run/user/1000/gdm/Xauthority xrandr --listmonitors
# 虚拟 monitor 实验（会改布局，实验后务必恢复）
xrandr --setmonitor S86L 640/0x800/0+0+0 none
xrandr --setmonitor S86R 640/0x800/0+640+0 none
xrandr --delmonitor S86L; xrandr --delmonitor S86R
# Wayland 会话事实
loginctl show-session 1 -p Type
grep WaylandEnable /etc/gdm3/custom.conf
# 门户能力（存在 ≠ 已实现）
gdbus introspect --session --dest org.freedesktop.portal.Desktop \
  --object-path /org/freedesktop/portal/desktop | grep -E "ScreenCast|RemoteDesktop"
```

## 5. 修正与未验证项

- 判据文件里“无 DRM 设备”不准确：VM 有 `/dev/dri/{card0,renderD128}`；但 gdm 配了 `WaylandEnable=false`，本轮**没有切换真实 Wayland 会话**（切换需重启，且会打断长跑）。
- Xwayland 抓屏未取得实测；报告中相关结论已标“未验证/按代码推断”。
- 多显示器偏移为代码推算，未接双真实输出。
