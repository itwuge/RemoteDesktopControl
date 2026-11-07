# S9 文本/IME 输入协议兼容性说明（android-client）

- 变更范围：`crates/rdc-input`（事件类型与线格式）、`crates/rdc-protocol`（能力位）
- 影响面：输入通道（`stream=2` / `priority=0`）的所有收发端：Windows 注入端、Linux 注入端、Android 客户端、未来的移动端
- 不改动：S4 消息头、AEAD 封装、键盘/指针/滚轮/触摸事件的字节布局

## 1. 线格式变更

### 1.1 新增事件 tag

既有输入事件 tag 不变（保证旧客户端逐字节兼容）：

| tag | 事件 | 载荷 |
|---|---|---|
| 0 | 指针 | 定长 14 字节 |
| 1 | 键盘 | 定长 |
| 2 | 滚轮 | 定长 |
| 3 | 触摸 | 定长 14 字节 |
| **4（新增）** | **文本** | `tag(1) + delete_units(2, LE) + utf8_text(N)` |

文本事件语义：先在远端发送 `delete_units` 次退格（UTF-16 码元计），再按 UTF-16 码元逐个注入 `utf8_text`。

- `delete_units = 0, text = "你"`：逐字上屏；
- `delete_units = 5, text = "中"`：输入法组合串 `zhong` 被候选词替换；
- `delete_units = 1, text = ""`：退格；
- 两者都为 0 视为空操作，发送端不应产生该事件。

为什么按 UTF-16 码元而不是 Unicode 字符计数：Windows `SendInput(KEYEVENTF_UNICODE)` 以码元为单位，BMP 外字符（emoji）占两个码元；按字符计数会让退格与远端文本错位。

### 1.2 载荷上限

- 单事件上限从 14 字节放宽到 **1351 字节**：单包明文 1352 - S4 消息 tag 1。
- 文本正文上限 1348 字节 = 1351 - tag 1 - 删除码元数 2。
- 旧事件（tag 0~3）最长仍为 14 字节，放宽上限不改变旧事件编码。

## 2. 能力位

- `Capabilities` 新增 **bit13 = TEXT_INPUT**（`crates/rdc-protocol/src/capabilities.rs`）。
- `KNOWN_BITS` 从 `(1<<13)-1` 扩展为 `(1<<14)-1`。
- 协商规则不变：双方按位与。任一端没有 bit13 时，文本事件**不得发送**，发送端退回逐扫描码键盘路径（英文可用，中文不可用）。

### 2.1 `Capabilities::DEFAULT` 故意不含 bit13

`Capabilities::DEFAULT` 仍是 `HEVC | TEMPORAL | CLIPBOARD | FILE_TRANSFER`（`capabilities.rs:69`），**没有**加 `TEXT_INPUT`。原因是这个常量同时被 Windows / Linux 注册路径复用，而 Linux 注入端对 `InputEvent::Text` 返回 `Unsupported`（`platform/linux.rs`）；若并入默认值，Linux 主机会声明一个无法兑现的能力。

因此：**现有 Windows 主控端即使升级到本分支代码，也不会自动声明文本能力**；要启用必须在注册时显式 `Capabilities::DEFAULT.with(bits::TEXT_INPUT, true)`（Windows 注入端已实现）。当前仓库没有这条注册调用点，属于「缺协议/缺接线」，见 TESTPLAN 阶段 3 收尾清单。

## 3. 旧客户端收到新消息时的行为

| 场景 | 旧客户端行为 | 后果 |
|---|---|---|
| 旧注入端收到 tag4 文本事件 | `decode_event` 对未知 tag 返回 `InputError::MalformedPayload` | 该事件被丢弃并记账（`rejected`/错误字符串），**不会崩溃、不会误解为其它事件** |
| 旧客户端读注册报文里的 bit13 | 位图解析保留原值；`has_unknown_bits()` 返回 true；当前仓库没有调用该判定（只提供 API） | 会话继续，旧端不启用文本能力 |
| 旧客户端协商 | `negotiate` 会把 bit13 裁掉（旧二进制的 `KNOWN_BITS = (1<<13)-1`） | 新端看到协商结果无 bit13，不会发送文本 |
| 新客户端收到旧端（无 bit13） | 协商结果无 bit13 | 新端不发送文本事件，输入法退化为扫描码 |
| **Windows 主控端（现有二进制）连新 Android 被控端** | 旧 `KNOWN_BITS` 不认识 bit13：`has_unknown_bits()` 只是告警，`negotiate` 按位与后丢掉 bit13；解码 tag 不受影响 | **不会误判为其它能力**，不会崩溃；只是协商不出文本，主控端继续用扫描码键盘 |
| **新版 Windows 主控端连旧 Android 客户端** | 新版 `DEFAULT` 仍无 bit13，故上报位图与旧版一致（低 13 位不变） | 旧客户端看到的能力集完全不变，行为零变化 |
| **旧版 Windows 主控端 + 新 Android 客户端** | 主控端不声明 bit13；Android 只按协商结果门控（`SessionController.setNegotiatedCapabilities`） | Android 不发 tag4；旧主控端不需要认识新事件 |
| **新版 Windows 主控端（显式 opt-in）+ 新 Android 客户端** | 双方都置 bit13，协商保留 | 文本事件启用；这是唯一会真正发送 tag4 的组合 |

**结论**：新增消息类型 + 能力位是向后兼容的“软升级”；旧端不会因为新端发文本而崩溃，也不会把 bit13 误判成其它能力，但旧端**不能**处理中文输入。默认配置下（含现有 Windows 主控端）文本能力是关闭的，必须两端都显式 opt-in。

## 4. 发送端实现要点（Android）

- 捕获：`TextInputConnection`（`RemoteTextInput`）拿到 IME 的 `TextEditingValue` 流。
- 差分：取前后文本的最长公共前缀，转成 `deleteUnits + insert`；代理对边界回退一位，避免把 emoji 拆成两半。
- 分段：按 1348 UTF-8 字节切段（`splitTextEdit`），删除量超过 `u16` 时按 65535 切分。
- 顺序：先发完删除，再发插入；单帧内不合并，保持输入时序。

## 5. 注入端实现（Windows）

- `SendInput` 的 `KEYEVENTF_UNICODE`：`wVk = 0`、`wScan = UTF-16 码元`，按下/抬起成对发送。
- 退格用 `wVk = VK_BACK` 的普通键盘事件（不是 UNICODE）。
- 每批最多 64 条输入（32 码元），避免安全软件截断与单次调用超时。
- 限制：目标窗口以管理员权限运行而本进程不是时，UIPI 会让 `SendInput` 返回 0；此时注入层返回 `InjectionRejected`，调用方应提示用户以相同权限运行。

## 6. 未验证项

- 没有真机：Android 端 IME 捕获、分段发送与被控端上屏的端到端链路**未实测**。
- Linux 注入端对文本事件返回 `Unsupported`（XTest/uinput 无任意 Unicode 注入能力），本次未实现，不在兼容承诺内。
- 延迟、丢事件率、长文本（>1348 字节）分段表现均需真机 + 被控端联合测量。
