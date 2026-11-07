# S8-5 · Linux 被控端常驻服务（rdc-serve）

分支 `linux-x11`，工作树 `RDC-linux`。不合并、不推送、不改 `docs/`、不改协议、不碰 Windows 侧代码。

## 1. 结论

VM（`192.168.37.130`，Ubuntu + X11 `Virtual1` 1280×800）上的 Linux 被控端现在是**图形会话内的常驻服务**：
开机自启、可被局域网连上、日志固定路径可判读。5 分钟实测无崩溃、内存基本不增长、注入坐标与请求完全一致。

| 项 | 结果 | 判据 |
| --- | --- | --- |
| 监听 | `0.0.0.0:45900`（实际监听地址/端口/配对码写入启动日志） | A1 通过 |
| 配对码 | `802893`，持久化在 `~/.config/rdc/pair-code`（0600），重启不变 | A2/A3 通过 |
| 重复连接 | 断开后继续 accept；5 分钟结束后的重连成功（`session=3`） | A4/D5 通过 |
| 自启 | 重启后 **03:22:51** 自动拉起（pid 1771），无需人工登录操作 | B1/B2 通过 |
| 崩溃留痕 | `kill -9` → `SERVE_EXIT rc=137`，附控制台尾部，3 s 后自动重启 | B3 通过 |
| 干净停止 | `stop.sh` → `SERVE_EXIT rc=0 clean=1`，pidfile 删除、端口关闭 | B4 通过 |
| 日志 | `~/.local/state/rdc/serve.log`（结构化）、`serve-console.log`、`serve-crash.log`、`serve-status.json` | C1–C4 通过 |
| 采集 fps | **59.9**（299 个 1 s 样本，min 45 / max 61） | D2 通过（实测即结论） |
| 注入精度 | 请求点→目标回读 **0 px**（30/30 次 `(399,299)` 完全一致）；相对绿块几何中心 (400,300) 为 **1 px** | D3 通过（≤1 px） |
| 内存 | 服务 RSS 19,644 → 21,156 kB（**+1.5 MB / 300 s**）；子进程 ffmpeg 79.7 → 81.7 MB | D4 通过（≤20 MB） |
| 崩溃 | 无。299 条 STATS 中 `encode_errors=0 write_errors=0`，`serve.log` 无 `ERROR`/`E_PANIC` | D1 通过 |
| 跨主机 | Windows 主机直连 `192.168.37.130:45900`，收到 `kind=3 1280x800@60`，服务日志 `CONNECT peer=192.168.37.1` | D6 通过 |

## 2. 端点与配置

- 二进制：`target/release/rdc-serve`（`crates/rdc-linux-e2e/src/bin/rdc-serve.rs`）。
- 配置：`~/.config/rdc/serve.conf`（`key=value`，CLI 同名参数优先；无重型配置框架）。
- 安装：`packaging/linux/deploy_serve.sh`（二进制 + `run.sh`/`stop.sh` + 配置样例 + autostart）。
- 启动日志首行（重启后实际）：
  `SERVE_LISTEN addr=0.0.0.0 port=45900 pair=802893 pid=1771 fps=60 bitrate=8000000`

## 3. 常驻化方式（为什么不用 systemd）

被控端必须能拿到图形会话的 `DISPLAY`/`XAUTHORITY`（X11 采集与 XTest 注入都依赖）。因此：
- 自启点在 `~/.config/autostart/rdc-serve.desktop`（GNOME autostart，`Exec=/home/wuge/rdc-serve/run.sh`）；
- `run.sh` 未显式设置时补 `DISPLAY=:0` 与 `/run/user/1000/gdm/Xauthority`；
- `run.sh` 负责崩溃留痕与受控重启（同一会话内 5 分钟连续 3 次崩溃即 `SERVE_GIVEUP`，不静默循环）；
- 进程内 SIGTERM/SIGINT 处理器置停机标志，主循环退出 → exit 0（干净停止不记为崩溃）。

## 4. 日志字段（安卓线与 Windows 线判读用）

固定路径 `~/.local/state/rdc/serve.log`，行格式 `UTC时间 级别 错误码 描述`。

- `SERVE_START` / `SERVE_LISTEN`：配置与**实际**监听地址、端口、配对码、pid。
- `CONNECT` / `DISCONNECT`：`peer`、`session`、`code`（`E_PEER_EOF`/`E_STOP`/`E_WRITE`…）、会话时长与累计量。
- `STATS`（每秒）：`capture_fps`、`packet_fps`、`bitrate_bps`、`injected_total`/`inject_rate`、`inject_p50_us`/`inject_p99_us`/`inject_max_us`、`rss_kb`、`backpressure`、`encode_errors`、`write_errors`。
- `IDLE`（每 10 s）：无客户端时的心跳（证明进程活着 + RSS）。
- 错误码：`E_LISTEN` `E_CAPTURE_INIT` `E_ENCODER_INIT` `E_WRITE` `E_SOCKET` `E_INPUT_DECODE` `E_INJECT` `E_INJECTOR_INIT` `E_PANIC`。

样例（5 分钟末尾）：

```
STATS peer=127.0.0.1:54514 capture_fps=60 packet_fps=51 bitrate_bps=2870878 packets_total=14644 \
      injected_total=687 inject_rate=2 inject_p50_us=70 inject_p99_us=70 inject_max_us=70 \
      rss_kb=21156 backpressure=3270 encode_errors=0 write_errors=0
```

## 5. 5 分钟证据（300 s，30 次点击，500 ms 移动）

- 客户端（`s8_2_client`，本机回环）：`packets=14668 decoded=14670 displayed=14669 dropped=0 decode_errors=0 pts_regressions=0`；
  端到端（采集→解码显示）延迟 `p50=54.9 ms`、`p99=117.5 ms`、`max=197.1 ms`。
- 注入：688 次注入，`inject_p50_us` 均值 **73.9 µs**（全样本 15 µs–1.46 ms），最坏单次 **6.66 ms**。
  （每秒仅 2–3 个注入样本，`inject_p99_us` 实际退化为该秒最大值，不足以代表长尾。）
- 点击命中：30/30 次目标窗口回读 `x_root=399 y_root=299`，客户端请求同为 `(399,299)` → **注入零误差**；
  目标窗口自身几何中心为 `(400,300)`，客户端绿块质心取整少 1 px → 相对中心 **1 px**。
- 采集 59.9 fps；编码出包 49 fps（32–60）；码率 2.40 Mbps（峰值 6.25 Mbps）；背压丢帧累计 3270 帧（约 18%）。
- 内存：本体 19,644 → 21,156 kB（+1.5 MB）；软编子进程 79.7 → 81.7 MB（+2 MB）。无单调爬升。
- 崩溃/错误：无；`serve-crash.log` 仅有本轮**故意**制造的 `kill -9` 与受控停止记录。

原始日志：`logs/s8-5/serve.log`（299 条 STATS）、`serve-crash.log`、`serve-status.json`、`serve-console.log`、
`target.out`（30 条 PRESS 回读）、`client.log`、`rss.log`（`logs/` 被 .gitignore 忽略，仅本机留证）。

## 6. 不达标 / 偏差（如实写）

| 项 | 目标 | 实测 | 结论 |
| --- | --- | --- | --- |
| 编码出包帧率 | 60 fps | 49 fps（min 32） | **不达标**：VM 无 GPU，x264 软编吞吐不足 |
| 码率 | 8 Mbps | 平均 2.40 Mbps | **不达标**：受帧率与静态画面限制，未吃满码率 |
| 背压 | 0 | 3270 帧丢弃（≈18%） | 主动丢帧保延迟，代价是编码帧率低于采集帧率 |
| 注入延迟长尾 | 稳定 | 个别秒 6.66 ms | 可接受但非稳定低延迟，与 X 服务器抢占有关 |
| 端到端显示延迟 | — | p50 54.9 ms | llvmpipe 软件渲染 + 软解，非真实设备指标 |

**采集侧 60 fps 稳定**是本轮硬指标；下游（编码/显示）受软件环境限制，数字照实记录。

## 7. 协议缺口（按约束「不改协议，先记录」）

1. **配对码无法强制校验**：S8-2 线协议包类型固定为 `1..4`（VIDEO/INPUT/CONFIG/BYE），没有认证/配对字段。
   要强制校验必须新增包类型（如 `KIND_PAIR`）或复用生产协议的握手，属于协议改动 → 本轮**未做**，只做展示/日志对账。
2. **无加密**：明文 TCP，仅限实验网段（`192.168.37.0/24`），不得暴露到不可信网络。
3. **Linux 注入器不支持 Touch**（`InputEvent::Touch` 返回 `Unsupported`，见 `rdc-input/src/platform/linux.rs`）：
   安卓触摸需在客户端转成 `Pointer`（move + press/release）。若要求原生触摸语义，需要改注入器（非协议）。
4. 该服务说的是 **S8-2 最小推流协议**，不是 `docs/` 里的生产协议。安卓线若走 Flutter + `rdc-bridge`，
   需要单独实现该线协议（或后续另做「生产协议被控端」，工作量另计）。

## 8. 约束核对

- 协议：`protocol.rs` 字节级未改（本轮新增文件不含 `protocol` 修改）。
- Windows 侧代码：未触碰（变更仅 `crates/rdc-linux-e2e/src/bin/rdc-serve.rs` 与 `packaging/linux/*`）。
- 未合并、未推送；`docs/` 未改。

## 9. 未验证项

- 安卓模拟器/真机：VM 无 `/dev/kvm`，模拟器不可用；以「本机客户端 300 s + Windows 跨网段 TCP」两层替身验证端点。
- Wayland 会话、多显示器、真实 GPU 编码、长时间（>1 h）泄漏、多客户端并发压力：未验证。
- 配对码安全强度（6 位、无握手）：仅作展示，不构成访问控制。

## 10. 复现

```bash
# VM 内
cd ~/rdc-linux
cargo build --release -p rdc-linux-e2e --bin rdc-serve
packaging/linux/deploy_serve.sh
~/rdc-serve/run.sh &          # 或等图形会话 next login 自启
~/rdc-serve/stop.sh           # SIGTERM → exit 0
tail -f ~/.local/state/rdc/serve.log
```
