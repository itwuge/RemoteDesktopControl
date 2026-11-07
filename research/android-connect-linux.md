# 安卓线直连说明 · Linux 被控端（S8-5）

面向：安卓客户端（模拟器或真机）连 VM 上的 Linux 被控端。
状态：端点已常驻、已验证可达；安卓侧客户端尚未在本仓库实现（见「限制」）。

## 1. 连接参数

| 项 | 值 |
| --- | --- |
| IP | `192.168.37.130`（VM 内自连可用 `127.0.0.1`） |
| 端口 | `45900`（TCP，非 WebSocket/HTTP） |
| 配对码 | `802893`（持久化于 `~/.config/rdc/pair-code`，重启不变） |
| 画面 | 1280×800 @ 60 fps，H.264 Annex-B，目标码率 8 Mbps（实测约 2.4 Mbps） |
| 坐标空间 | 物理像素，左上原点；viewport `(0,0,1280,800)`，scale `1.0` |

> 配对码当前**只做展示与日志对账，不做强制校验**：S8-2 线协议没有配对字段。见「限制」。

## 2. 先做三件事（排障顺序）

```bash
# 1) 网络可达
ping 192.168.37.130
# 2) 端口开放（Windows 上也验过：CONFIG 能拿到）
nc -vz 192.168.37.130 45900
# 3) 看服务是否在跑（VM 内）
ss -lntp | grep 45900
tail -n 20 ~/.local/state/rdc/serve.log
```

安卓模拟器与 VM 要在同一网段（VM NAT 模式下需把 45900 映射到宿主机；桥接模式直接可用）。

## 3. 线协议（S8-2 最小推流协议，未改动）

字节序全部小端（LE）。每包定长头 + 载荷：

```
+--------+-----------------+---------------------+
| kind   | len             | payload[len]        |
| u8     | u32 (LE)        | ...                 |
+--------+-----------------+---------------------+
```

`kind`：

| 值 | 方向 | 载荷 |
| --- | --- | --- |
| 3 `CONFIG` | host → client | `width u32, height u32, fps u32`（12 字节） |
| 2 `INPUT` | client → host | rdc-input 单事件载荷（5–14 字节，见下） |
| 1 `VIDEO` | host → client | `pts_ns u64, flags u8, Annex-B H.264`（flags bit0=关键帧） |
| 4 `BYE` | 双向 | 空（主动断开） |

单包载荷上限 8 MiB（`MAX_PAYLOAD`）。

### 3.1 输入事件（`rdc_input::encode_event` 兼容格式）

| tag | 事件 | 载荷 |
| --- | --- | --- |
| `0` | Pointer（**安卓触摸请用这个**） | `tag(1) + x i32 + y i32 + button u8 + pressed u8`（11 字节） |
| `1` | Key | `tag(1) + scan_code u16 + pressed u8 + modifiers u8`（5 字节） |
| `2` | Wheel | `tag(1) + delta_x i32 + delta_y i32`（9 字节） |
| `3` | Touch | `tag(1) + id u32 + x f32 + y f32 + phase u8`（14 字节，**Linux 被控端不支持**） |

- `button`：`0=None`（纯移动）、`1=Left`、`2=Right`、`3=Middle`。
- `modifiers`：`1=Shift`、`2=Ctrl`、`4=Alt`、`8=Meta`（位掩码）。
- `scan_code`：PC/AT Set 1 语义；扩展键带 `0xE000` 前缀（方向键、Home/End、右 Alt 等）。
- 坐标是**远端桌面物理像素**，客户端负责把触摸/手势映射到 1280×800 空间后再编码。

### 3.2 连接流程

1. `TcpStream.connect(192.168.37.130:45900)`。
2. 读 `CONFIG`（一般是首个包；服务端在建会话、启动采集后才发）。
3. 用 `CONFIG` 初始化解码器；之后循环读 `VIDEO` 送 MediaCodec，同时把触摸/键盘编码成 `INPUT` 写出。
4. 断开发 `BYE`（kind=4，len=0）后关闭 socket。

MediaCodec 提示：VIDEO 是 Annex-B，首个关键帧里可提取 SPS/PPS 组装 `csd-0/csd-1`；若直接喂码流，需保证输入缓冲带 start code。

## 4. 启动 / 停止 / 自启（VM 内）

```bash
# 启动（手动，当前图形会话内）
~/rdc-serve/run.sh &

# 停止（SIGTERM，干净退出，不记为崩溃）
~/rdc-serve/stop.sh

# 查看
cat ~/.local/state/rdc/serve-status.json     # 每秒刷新的状态快照
tail -f ~/.local/state/rdc/serve.log         # 结构化日志
cat ~/.local/state/rdc/serve-status.json
cat ~/.config/rdc/pair-code                  # 配对码
```

- 自启：`~/.config/autostart/rdc-serve.desktop`（GNOME 图形会话内拉起，**不是 systemd 系统服务**）。
- 重装/更新：`cd ~/rdc-linux && cargo build --release -p rdc-linux-e2e --bin rdc-serve && packaging/linux/deploy_serve.sh`。
- 重启 VM 后无需人工操作：已实测在开机 30 s 内自动监听。

## 5. 日志判读

- `SERVE_LISTEN`：`addr/port/pair/pid` —— 对账配对码与端口。
- `CONNECT peer=... session=N` / `DISCONNECT peer=... code=...`：连接建立/断开与原因码。
- `STATS`（每秒）：`capture_fps`、`packet_fps`、`bitrate_bps`、`inject_p50_us/p99/max`、`rss_kb`、`backpressure`、`encode_errors`、`write_errors`。
  安卓端卡顿先看 `packet_fps`（<50 说明服务端编码或网络跟不上）与 `backpressure`（主动丢帧数）。
- `IDLE`（每 10 s）：无客户端心跳。
- 错误码：`E_LISTEN` `E_CAPTURE_INIT` `E_ENCODER_INIT` `E_WRITE` `E_SOCKET` `E_INPUT_DECODE` `E_INJECT` `E_INJECTOR_INIT` `E_PANIC`。
- `~/.local/state/rdc/serve-crash.log`：非 0 退出记录（含 rc、运行时长、控制台尾部 12 行）。

## 6. 已测证据（供安卓线对齐）

- Windows 主机 → `192.168.37.130:45900`：`kind=3 len=12 1280x800@60`，服务日志 `CONNECT peer=192.168.37.1`。
- VM 本机 300 s 会话：采集 59.9 fps，30/30 次点击回读 `(399,299)` 与请求一致（0 px），内存 +1.5 MB，无崩溃。
- 详见 `research/s8-5-linux-serve.md` 与 `logs/s8-5/`。

## 7. 触摸 / 手势映射（安卓必读）

**结论先行**：Linux 被控端注入器**不支持 `InputEvent::Touch`（tag=3）**，收到即返回 `Unsupported`（`crates/rdc-input/src/platform/linux.rs` 的 `inject_xtest` 与 `inject_uinput` 分支），服务只累计 `E_INJECT`，画面无任何反应。安卓必须把触摸手势**翻译成 Pointer（tag=0）**，且只能表达**单指针**。

### 7.1 单指针 Pointer 的可用语义

`Pointer` 载荷只有：`x i32`、`y i32`、`button u8`、`pressed u8`。注入器行为是「先移动，再按键/抬键」，因此一次事件可以同时表达移动与按键变化。

推荐状态机（固定 `id=0`，丢弃其余触点）：

| 触摸阶段 | 编码 |
| --- | --- |
| Down | `Pointer(x, y, button=1 Left, pressed=1)` |
| Move（按住期间） | `Pointer(x, y, button=0 None, pressed=0)`（只移动，保持按下状态） |
| Up | `Pointer(x, y, button=1 Left, pressed=0)` |
| Cancel | 同 Up（确保释放，避免“粘住”左键） |

坐标：安卓屏幕坐标 → 远端物理像素 `1280×800`（`viewport=(0,0,1280,800)`、`scale=1.0`），越界会被服务端钳制到视口。

### 7.2 手势映射与失真判定

| 手势 | 可否映射 | 映射方式 | 保真度 | 说明 |
| --- | --- | --- | --- | --- |
| 单击 / 点按 | ✅ | Down→Up（同点） | 高 | 与鼠标左键单击等价 |
| 双击 | ✅ | 两次单击，间隔 ≤ 系统双击阈值 | 中高 | X11 应用按自身双击时间判定；跨网络抖动可能漏判 |
| 长按 | ✅ | Down 后保持，超过阈值再 Up | 高 | 按住期间不移动即可 |
| 拖动 / 拖拽 | ✅ | Down → 多次 Move → Up | 高 | Move 必须带 `button=0`，否则会重复按下 |
| 单指上下滑（滚动列表） | ⚠️ | 二选一：① 鼠标拖动；② Wheel | **中低** | ① 在文本区会变成选择文本，不是滚动；② 需客户端自行做位移→轮次量化，无惯性 |
| 双指滚动 | ⚠️ | Wheel（`delta_y` 正=向下） | 中 | 只能近似；两指间距/速度信息丢失，无平滑惯性 |
| 双指缩放（pinch） | ❌ **不支持** | 无法表达两个同时按下的指针 | 失真 | 只能退化为单指拖动或 Wheel，缩放语义丢失 |
| 三指及以上 | ❌ **不支持** | — | 失真 | 多指并发无编码位；只会有一个“主触点”生效 |
| 压力 / 笔倾斜 | ❌ **不支持** | — | 失真 | Pointer 无压力/倾斜字段 |
| 边缘手势、系统返回手势 | ❌ **不支持** | — | 失真 | X11 桌面无对应触摸语义；最多表现为鼠标拖到边缘 |
| 双指轻点（右键近似） | ⚠️ | `button=2 Right` 单击 | 中 | 需客户端自定义；不是所有应用都有右键菜单 |

**失真根因**：线协议与 XTest 注入都只有**单指针 + 3 键 + 4 向滚轮**，没有触点 id 集合、没有并发触点、没有压力/面积/时间戳。任何需要“两个手指同时按在不同位置”的手势都无法忠实表达。

### 7.3 安卓实现要点

1. **只发 tag=0**：不要发 `Touch`（tag=3），也不要在本轮为了多指新增 tag/字段（协议冻结）。
2. **多点触控取主触点**：`ACTION_POINTER_DOWN/UP` 时维护触点集合，仅把 `index=0`（或最后按下）当作主触点；其余忽略并记日志，避免坐标跳动。
3. **滚动优先用 Wheel**：单指滑动建议按位移阈值累积成 `Wheel`（`delta_y` 单位=1 格），比“鼠标拖动”更接近用户预期；只有明确进入拖拽状态（长按后移动）才发按住 Move。
4. **务必发 Up/Cancel**：Activity 失焦、手势取消、连接断开时都要补一个 `pressed=0`，否则被控端左键会一直按下。
5. **节流**：Move 建议 ≤ 60–120 Hz 合并发送，避免小包刷屏；输入包不占令牌桶，但仍会增加对端注入耗时。
6. **按键映射**：`button` 线上编码是 `1=Left、2=Right、3=Middle`（注意与 X11 按钮号 1/3/2 的差异由注入器内部转换，客户端只按线上编码填）。
7. **滚轮方向**：`delta_y > 0` → 注入 X11 button 5（向下滚），`< 0` → button 4；`delta_x > 0` → button 7（向右），`< 0` → button 6。数值按“格”计，注入器会按 `abs(delta)` 重复。

### 7.4 给安卓线的验收用例

| 用例 | 判定 |
| --- | --- |
| 单击桌面图标 | 被控端出现左键单击，图标被选中/打开 |
| 长按后拖动窗口 | 窗口跟随移动，松手后停止，无“粘键” |
| 列表单指上滑 | 列表滚动（用 Wheel 实现时）；若用鼠标拖动则允许出现选择文本（已知失真） |
| 双指缩放地图 | **预期不生效**；客户端应在 UI 上禁用/提示“被控端不支持多点手势” |
| 断连/切后台 | 被控端左键必须已释放（补 Up） |

---

## 8. 限制与待办（务必先读）

1. **配对码不校验（本轮现状）**：当前 S8-2 线协议没有配对字段，码只做展示。方案已定：新增 `kind=0x10–0x13` 配对包，默认走被控端「允许/拒绝」，配对码仅用于无人值守强校验；设计与改动清单见 `research/protocol-pair-proposal.md`，**本轮不实现**。
2. **明文无加密**：只允许在实验网段使用，禁止暴露到不可信网络。
3. **Touch 事件不支持**：Linux 注入器对 `InputEvent::Touch` 返回 `Unsupported`；手势映射与失真判定见 §7。
4. **单会话**：同一时刻只服务一个客户端；第二连接会排在被 listen backlog 中，前一会话结束后才被服务（TCP connect 会成功，读 `CONFIG` 会等待）。
5. **安卓模拟器不可用**：VM 无 `/dev/kvm`；本轮没有真机验证。端点可达性来自「本机客户端 + Windows 跨网段 TCP」。
6. **非生产协议**：本服务说 S8-2 最小推流协议，不是 `docs/` 的生产协议；`rdc-bridge` Flutter 插件不认这个端点，安卓需按第 3 节自行实现客户端（或后续另做生产协议被控端）。
