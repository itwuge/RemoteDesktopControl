# 配对（Pairing）协议提案 · S8-6

状态：**已决策**（采用方案 a：新增独立包类型；默认路径为被控端本机「允许/拒绝」，不强制输入配对码）
范围：**只做设计，不改代码**（本轮白屏修复优先）。本文不改 `crates/rdc-linux-e2e/src/protocol.rs` 的常量表，也不改 `docs/`。
适用端：Linux 被控端（`rdc-serve`，X11）↔ Windows 控制端 ↔ Android 控制端。

---

## 1. 决策摘要（口径固化）

| 项 | 决策 |
| --- | --- |
| 方案 | **a：新增独立包类型承载配对**，走 `kind` 命名空间，不塞进 `HELLO` 载荷 |
| 默认路径 | 控制端发起连接 → 被控端本机弹「允许 / 拒绝」→ 允许后进入媒体流；**配对码不是必经步骤**，首次连接阻力是一次点击 |
| 配对码用途 | 只在「无人值守 / 长期信任」场景作为固定凭证；此时**协议必须真校验**（SPAKE2），不能只展示 |
| 协商方式 | `capabilities` 位协商；旧端不支持时回退到「点允许」路径 |
| 降级路径 | 控制端**永远保留**「改为让对方点允许」的显式入口 |
| 防暴力 | 沿用 `rdc-security` 的退避与封禁（1/2/4/8/16/32/60 s，15 次/小时封 15 分钟；目标维度 10 次/10 分钟封 30 分钟），连续失败锁定并留日志 |
| 本轮 | **不实现代码**，只交设计与改动清单 |

---

## 2. 现状（基线事实）

1. **线格式**：`kind: u8` + `len: u32 LE` + `payload[len]`；单包上限 8 MiB（`MAX_PAYLOAD`）。见 `crates/rdc-linux-e2e/src/protocol.rs`。
2. **已占用 kind**：`1=VIDEO`、`2=INPUT`、`3=CONFIG`、`4=BYE`。
3. **`0x61–0x64`**：按决策口径为既有视频消息保留区，本提案回避（本仓库 grep 未见该常量定义，按上游约定处理）。
4. **当前配对码**：`rdc-serve` 启动时读 `~/.config/rdc/pair-code`，不存在则随机生成 **6 位十进制**并写入（权限 0600），见 `resolve_pair_code`。**只打印到 `SERVE_LISTEN` 日志与状态文件，不参与任何校验。**
5. **当前连接顺序**：服务端 accept 后建采集 → 发 `CONFIG` → 推 `VIDEO`；客户端只认 `CONFIG/VIDEO/BYE`，未知 kind 走 `_ => {}` 忽略（`s8_2_client` 实测行为），等待 `CONFIG` 超时为 5 s。
6. **既有安全件**：`rdc-security` 已有 10 位 Crockford Base32 配对码（50 bit 熵，`PAIRING_CODE_LEN=10`）、SPAKE2 `PakeSession`、4 报文握手、`FailureThrottle`（`ThrottlePolicy::default()/target()`）。本轮设计直接复用，不新造密码学。

---

## 3. 三个方案对比与取舍

| 维度 | 方案 a：新增独立包类型（**已决策**） | 方案 b：`HELLO` 加可选字段 | 方案 c：暂不强制、仅对账（现状） |
| --- | --- | --- | --- |
| 改动面（协议层） | 新增 4 个 kind + 能力位常量；媒体包字节不变 | 需先引入 `HELLO` 包/结构；字段塞进握手载荷，签名范围要重定义 | 0 |
| 改动面（Linux 被控端） | 认证状态机 + 本机弹窗 + 退避接线 | 同 a，另需解析可选字段与缺省语义 | 0 |
| 改动面（Windows 控制端） | 发送 `HELLO`、处理 `PAIR_RESULT`、等待允许 UI、保留降级入口 | 需改既有握手序列化与签名覆盖范围，回归面更大 | 0 |
| 改动面（Android） | 同 Windows，Kotlin 侧实现 4 个包 | 同 Windows，且要跟生产握手对齐 | 0 |
| 旧端兼容 | **好**：旧端忽略未知 kind（已实测 `_ => {}`），服务端凭 500 ms 无 `HELLO` 判定为旧端并回退「点允许」 | **差**：旧端若校验字段/签名会直接握手失败 | 无变化 |
| 重放风险 | **低**：每次连接用新 `server_nonce` 绑定 SPAKE2/确认；失败即断链，不复用 transcript | 中：字段若未进签名范围可被剥离/重放 | 高：无校验，任意人可连 |
| 暴力猜码风险 | **可管**：PAKE + 每 IP/每目标退避 + 一次性码 3 次作废 | 同 a，但字段解析分支多，易漏校验 | **不可管**：码只是展示 |
| 失败退避 | 直接复用 `FailureThrottle` 两套策略 | 同 a | 无 |
| 失败表现 | `PAIR_RESULT` 明确 `status/retry_after_ms`；控制端可显示倒计时 | 需借 `BYE` 或错误码表达，语义弱 | 无 |
| 结论 | **采用** | 否 | 否（仅作对照基线） |

**为什么否掉 b**：`HELLO` 目前在本仓库不存在（S8-2 最小协议没有握手包），加可选字段等于先造 `HELLO` 再造配对，且签名范围（`docs/04`/`docs/07` 的 AAD 与 `RDC-AUTH‖session‖eph‖ts`）需要重定义，旧端回退语义模糊。方案 a 的独立包可以被旧端安全忽略。

---

## 4. 已决策设计（方案 a）

### 4.1 包类型编号建议

在同一 `kind` 命名空间内新增 **控制/配对子命名空间 `0x10–0x13`**（低 4 位为组内序号，避开 `1..4` 与 `0x61–0x64`）：

| kind | 名称 | 方向 | 用途 |
| --- | --- | --- | --- |
| `0x10` | `KIND_CLIENT_HELLO` | client → host | 协议版本、能力位、客户端 nonce |
| `0x11` | `KIND_SERVER_HELLO` | host → client | 协议版本、能力位、服务端 nonce |
| `0x12` | `KIND_PAIR` | 双向 | 配对状态机一步（SPAKE2 消息或 HMAC 确认） |
| `0x13` | `KIND_PAIR_RESULT` | host → client | 结果、原因、退避剩余、剩余尝试次数 |

约定：
- 所有配对包 `len ≤ 64` 字节（远小于 `MAX_PAYLOAD`），解析必须严格校验长度，非法即断链并记 `E_PAIR_MALFORMED`。
- `0x14–0x1F` 预留给后续（设备信任持久化、SAS 展示、会话恢复），本轮不分配。
- **不修改** `1..4` 的既有语义与 `VIDEO` 载荷（`pts_ns u64 + flags u8 + Annex-B`）。

### 4.2 能力位（capabilities u32，LE）

| bit | 名称 | 归属 | 语义 |
| --- | --- | --- | --- |
| 0 | `CAP_PAIR_V1` | 双方 | 认识 `0x10–0x13`，参与配对状态机 |
| 1 | `CAP_PAKE_SPAKE2` | 双方 | 支持配对码 SPAKE2 校验（无人值守必需） |
| 2 | `CAP_HOST_APPROVAL` | host | 被控端能在本机弹「允许/拒绝」 |
| 3 | `CAP_PAIR_STATUS_EXT` | 双方 | 认识 `PAIR_RESULT` 的 `reason/retry_after_ms/attempt_left` 扩展语义 |
| 4–31 | 保留 | — | 必须为 0；解析时忽略未知位（向前兼容） |

协商规则：
- 交集能力决定路径：双方都有 `CAP_PAIR_V1` 才走配对包；否则走「旧端回退」。
- `CAP_PAKE_SPAKE2` 只在被控端策略为 `fixed`（无人值守）时被要求；缺失则返回 `UNSUPPORTED`。
- `CAP_HOST_APPROVAL` 由被控端声明；控制端据此把 UI 显示为「点击对方屏幕上的允许」。

### 4.3 字段布局

全部小端。`[n]` 表示定长字节数组，`var` 表示由前序长度约束。

**`0x10 KIND_CLIENT_HELLO`（len = 22）**

```
offset  0  u16  proto_ver        // 建议 0x0001
offset  2  u32  caps             // 见 4.2
offset  6  [16] client_nonce     // CSPRNG，每次连接新值
```

**`0x11 KIND_SERVER_HELLO`（len = 22）**

```
offset  0  u16  proto_ver
offset  2  u32  caps
offset  6  [16] server_nonce
```

**`0x12 KIND_PAIR`（len = 4 + body_len）**

```
offset  0  u8   step             // 1=CLIENT_PAKE 2=HOST_PAKE 3=CLIENT_CONFIRM 4=HOST_CONFIRM
offset  1  u8   status           // 0=OK；非 0 时 body 可空，语义与 PAIR_RESULT 对齐
offset  2  u16  body_len         // 0 或 32
offset  4  var  body             // step 1/2：SPAKE2 消息（32 字节）
                                 // step 3/4：HMAC-SHA256 确认（32 字节）
```

确认 MAC 的输入建议（设计口径，不落代码）：
`HMAC(confirm_key, "RDC-PAIR" ‖ proto_ver ‖ client_nonce ‖ server_nonce ‖ direction_tag ‖ step)`；
`direction_tag`：客户端 `0x43('C')`、服务端 `0x48('H')`，避免双向 transcript 互换。

**`0x13 KIND_PAIR_RESULT`（len = 8）**

```
offset  0  u8   status           // 0=ALLOWED 1=PENDING_APPROVAL 2=DENIED
                                 // 3=BAD_CODE 4=LOCKED 5=UNSUPPORTED 6=CANCELLED 7=TIMEOUT
offset  1  u8   reason           // 0=none 1=user_denied 2=wrong_code 3=throttled
                                 // 4=no_operator 5=legacy_unsupported 6=client_timeout
offset  2  u32  retry_after_ms   // status=LOCKED/BAD_CODE 时有效，其他为 0
offset  6  u8   attempt_left     // 一次性码剩余尝试次数；不适用时 0xFF
offset  7  u8   reserved         // 0
```

### 4.4 三条流程

#### 流程 A：默认路径（点允许，不输码）

```
C → H  0x10 CLIENT_HELLO (CAP_PAIR_V1 | CAP_PAIR_STATUS_EXT)
H → C  0x11 SERVER_HELLO (CAP_PAIR_V1 | CAP_HOST_APPROVAL | CAP_PAIR_STATUS_EXT)
H      本机弹「允许 / 拒绝」（非阻塞，默认 60 s 超时）
H → C  0x13 PAIR_RESULT(status=PENDING_APPROVAL)
C      UI 显示「等待对方允许…」，可取消（取消则断链）
H → C  0x13 PAIR_RESULT(status=ALLOWED, reason=none)      // 用户点允许
H → C  3 CONFIG → 1 VIDEO …                               // 进入媒体流
C → H  2 INPUT …
```

拒绝：`0x13(status=DENIED, reason=user_denied)` 后发 `BYE` 并断开。
超时：`0x13(status=TIMEOUT, reason=no_operator)` 后断开。

#### 流程 B：无人值守 / 长期信任（配对码强校验）

被控端策略 `pair_policy=fixed`。不弹窗，必须完成 PAKE：

```
C → H  0x10 CLIENT_HELLO (CAP_PAIR_V1 | CAP_PAKE_SPAKE2)
H → C  0x11 SERVER_HELLO (CAP_PAIR_V1 | CAP_PAKE_SPAKE2)
C → H  0x12 PAIR(step=1, body=SPAKE2_A)
H → C  0x12 PAIR(step=2, body=SPAKE2_B)
C → H  0x12 PAIR(step=3, body=HMAC_C)
H → C  0x12 PAIR(step=4, body=HMAC_H)
H → C  0x13 PAIR_RESULT(status=ALLOWED)
H → C  3 CONFIG → 1 VIDEO …
```

失败：任一步校验失败 → `0x13(status=BAD_CODE|LOCKED, retry_after_ms=…)` → `BYE` 断开；必须重连才能重试（不复用半成品 transcript）。

#### 流程 C：旧端回退（点允许）

旧控制端不发 `0x10`：

```
H      accept 后等待 CLIENT_HELLO，宽限期 500 ms
       超时 → 判定旧端，走「点允许」路径（不依赖客户端任何新包）
H      本机弹「允许 / 拒绝」
H → C  允许：3 CONFIG → 1 VIDEO；拒绝/超时：4 BYE 断开
```

**诚实边界**：仓库内既有 demo 客户端等 `CONFIG` 只有 **5 s**，人工点击可能来不及；旧端可能表现为“连接后超时/需要重试”。这不改客户端字节格式，只提示安卓线把等待窗口提高到 60 s（见 `android-connect-linux.md`）。若被控端策略为 `fixed`，旧端一律 `UNSUPPORTED` 断开（因为无法完成 PAKE）。

### 4.5 防暴力、重放与退避

| 威胁 | 设计应对 |
| --- | --- |
| 在线暴力猜码 | PAKE 会话本身不暴露码；每次猜测需一次完整握手。按 `FailureThrottle::default()`（每 IP 1/2/4/8/16/32/60 s 递增；15 次/小时封 15 分钟）与 `ThrottlePolicy::target()`（每目标 10 次/10 分钟封 30 分钟）双维度限速 |
| 一次性码被反复试 | 复用 `PAIRING_CODE_MAX_FAILURES=3`：同一码 3 次失败立即作废，`attempt_left` 返回剩余次数 |
| 重放旧 transcript | 每次连接由 `client_nonce`/`server_nonce` 绑定，确认 MAC 覆盖双方 nonce 与方向；失败即断链，不保留可续跑状态 |
| 降级攻击（强迫走点允许） | 被控端策略 `fixed` 时拒绝一切非 PAKE 路径；控制端不能把 `fixed` 端降级为点允许 |
| 侧信道 | 对外统一 `BAD_CODE`（不区分码错/过期/不存在）；日志不记录用户输入的码 |
| 日志泄露 | **修改点**：`SERVE_LISTEN` 当前打印完整配对码，后续必须脱敏（只留后缀 2 位或指纹前 8 位） |

退避失败后：
- `PAIR_RESULT(status=LOCKED, retry_after_ms)` 给控制端倒计时；
- 被控端记 `E_PAIR_LOCKED peer=… failures=… until=…`（不含码）；
- 锁定期间新连接直接 `0x13(status=LOCKED)` + `BYE`，不进入 PAKE。

### 4.6 与 `~/.config/rdc/pair-code` 的关系

| 场景 | 文件角色 |
| --- | --- |
| `pair_policy=approval`（默认） | **不参与校验**；仅用于人眼对账与模式切换时的凭据来源。日志需脱敏 |
| `pair_policy=fixed`（无人值守） | **就是长期信任凭证**；每次连接作为 SPAKE2 口令读取（服务启动时读入内存） |
| 一次性配对模式（未来） | 码带 TTL（`PAIRING_CODE_TTL_MS=120 s`）与 3 次失败作废；与固定文件互斥 |

格式与迁移：
- 现有文件是 **6 位十进制（约 20 bit）**，仅够“对账”，不足以在无人值守下抗暴力。
- 无人值守建议改为 `rdc-security` 的 **10 位 Crockford Base32（50 bit）**；`PairingCode::generate()` 已具备该能力。
- 兼容策略：读取时接受两种格式；`fixed` 模式若检测到 6 位旧码，启动日志记 `WARN pair_code_weak` 并要求 `rdc-serve --rotate-pair` 轮换（轮换命令属后续实现，本轮只登记）。
- 权限保持 `0600`；不做多用户共享；不写入日志、不写入 `serve-status.json`（当前状态文件含 `pair` 字段，后续同样要脱敏）。

---

## 5. 改动清单（本轮不实施）

### 5.1 协议层（`crates/rdc-linux-e2e/src/protocol.rs`，以及将来 `docs/04` 对齐）

1. 新增常量：`KIND_CLIENT_HELLO=0x10`、`KIND_SERVER_HELLO=0x11`、`KIND_PAIR=0x12`、`KIND_PAIR_RESULT=0x13`，以及 `CAP_*` 位。
2. 新增载荷构造/解析（严格定长校验）与 roundtrip 单测。
3. **不动** `1..4`、`VIDEO` 载荷、`MAX_PAYLOAD`。
4. 文档侧：若并入生产协议，需同步 `docs/04` 的 kind 注册表与 `docs/07` 的握手章节（本轮不改 `docs/`）。

### 5.2 Windows 控制端

1. 连接后先发 `CLIENT_HELLO`，读 `SERVER_HELLO`（超时 500 ms 视为对端旧端）。
2. 实现状态机：`PENDING_APPROVAL` → 等待 `ALLOWED`（默认 60 s，可取消）；`DENIED/TIMEOUT` 给明确错误。
3. 无人值守入口：输入 10 位配对码 → 走 `KIND_PAIR`；显示 `attempt_left` 与 `retry_after_ms`。
4. 保留「改为让对方点允许」按钮（降级路径，任何错误后都可用）。
5. 媒体/输入路径零改动；不做字节格式“适配性”临时改动。

### 5.3 Android 控制端

1. 同 5.2 的 1–4（Kotlin 侧实现）。
2. `CONFIG` 等待窗口从 5 s 提高到 60 s（兼容旧被控端“点允许”延迟）。
3. 触摸仍走 `INPUT` 的 Pointer 载荷（见 `android-connect-linux.md` 触摸映射节）；配对包与触摸无关。
4. 不为了连通性临时改 `VIDEO/INPUT` 字节格式。

### 5.4 Linux 被控端（`rdc-serve`）

1. 新增配置 `pair_policy=approval|fixed`（默认 `approval`）。
2. accept 后进入认证阶段：等服务端/客户端 `HELLO` → 走流程 A/B/C；认证通过才发 `CONFIG`。
3. 本机弹窗：图形会话内用 GTK/zenity 类对话框；无图形/无操作者时按 `no_operator` 拒绝（`fixed` 模式则不弹窗）。
4. 接线 `rdc-security`：`PakeSession` + `FailureThrottle`（默认 + 目标维度），错误码 `E_PAIR_*` 落日志。
5. 日志与状态文件脱敏配对码（当前 `SERVE_LISTEN`/`serve-status.json` 是明文）。
6. 旧端宽限 500 ms 判定；`fixed` 模式下旧端直接 `UNSUPPORTED`。
7. 崩溃留痕/常驻化逻辑不变。

### 5.5 明确不做

- 不实现任何上述代码（白屏修复优先）。
- 不改 `protocol.rs` 常量表。
- 不碰 Windows 主仓库代码、不改 `docs/`。
- 不做加密（AEAD）改造：配对只解决“准入”，传输仍是明文实验协议。

---

## 6. 验收设计（后续实现时按此测）

| 用例 | 期望 |
| --- | --- |
| 新端 × approval | 弹窗允许 → `ALLOWED` → `CONFIG` 在 1 s 内到达；拒绝 → `DENIED` + `BYE` |
| 新端 × fixed + 正确码 | 4 步 PAKE 完成 → `ALLOWED` → 媒体流 |
| 新端 × fixed + 错码 | `BAD_CODE`，`attempt_left` 递减，3 次后码作废；第 2 次起有退避 |
| 旧端 × approval | 500 ms 后回退点允许；允许后收到 `CONFIG`；无新包依赖 |
| 旧端 × fixed | `UNSUPPORTED` + `BYE`，日志记原因 |
| 重放 | 抓包重放 step 3/4 必须失败 |
| 限速 | 连续失败按 1/2/4/8/16/32/60 s 递增，达阈值封禁并留 `E_PAIR_LOCKED` |

---

## 7. 未决项（登记，不在本轮拍板）

1. 本机弹窗的具体实现（GTK 依赖是否随包分发；无桌面会话时的降级策略）。
2. 设备信任持久化（`CAP_PERSIST_TRUST` 预留位）：指纹存哪里、如何撤销。
3. SAS 4 位可视确认是否纳入默认路径（`docs/07` 已有 SAS 设计，可作为中间强度选项）。
4. 生产协议（`docs/04`）与本 demo 协议的并轨方式：本提案只覆盖 `rdc-linux-e2e` 的 `kind` 空间。
