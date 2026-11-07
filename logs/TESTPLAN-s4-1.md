# S4-1 输入注入通道判据（先落盘，后实现）

日期：2026-11-07
阶段：S4（功能层）第一块 S4-1
范围：`crates/rdc-input`（事件线格式、坐标映射、移动合并限频、Windows SendInput 注入）
约束：不改 `rdc-capture` / `rdc-codec` / `rdc-decode` 既有行为；不改 `docs/`（缺口记 `docs/决策待办.md`）。

## 本块要交付的四件事

1. **线格式**：`InputEvent` 四变体（指针 / 键盘 / 滚轮 / 触摸）单事件二进制编码，
   载荷 ≤14 字节，远小于单包明文上限 1352 字节（docs/04 §1.2）。
2. **坐标映射**：控制端视图坐标 → 被控端桌面物理坐标；含 letterbox 内容矩形、
   DPI 缩放（125% 用例）与多屏原点偏移（负原点用例），越界返回 `None`。
3. **合并与限频**：仅合并同类移动（纯移动 / 同一按键的拖拽），上限 200 次/秒
   （docs/04 §2 pacer 规则第 2 条“输入包上限 200 包/秒”）；按键/滚轮/按下抬起不合并、不丢、不乱序。
4. **Windows 注入**：`SendInput` + `MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK`，
   键盘用 `KEYEVENTF_SCANCODE`（0xE0xx 视为扩展键），被系统拒绝（返回 0）映射为 `InjectionRejected`。

## 判据

| 编号 | 判据 | 阈值 / 期望 | 数据来源 |
|------|------|-------------|----------|
| S4-1-a | 线格式往返 | 4 变体逐字节往返一致；长度不符/未知 tag 一律 `MalformedPayload` | `cargo test -p rdc-input wire::` |
| S4-1-b | 坐标映射 | 1:1 精确；letterbox 中心对中心；125% DPI 用例；负原点多屏偏移；越界 `None`；边缘钳制到最后一像素 | `cargo test -p rdc-input mapper::` |
| S4-1-c | 绝对坐标归一化 | 左上角 → (0,0)，右下角 → (65535,65535)，负原点同样成立；越界输入先钳制 | `cargo test -p rdc-input injector::` |
| S4-1-d | 移动合并 | 间隔内连续移动只保留最新一点；合并计数 > 0；限频 200/s（5ms） | `cargo test -p rdc-input coalesce::` |
| S4-1-e | 按键不丢不乱序 | 挂起移动先于后续按键输出；N 个按下/抬起事件全部按序输出，1 个不丢 | `cargo test -p rdc-input coalesce::` |
| S4-1-f | 注入标志 | 左/右/中按下抬起与滚轮方向标志正确；扩展扫描码带 `KEYEVENTF_EXTENDEDKEY`；真实 `SendInput` 编译通过 | `cargo test -p rdc-input platform::` + `cargo build -p rdc-input` |
| S4-1-g | 不越界改动 | `git diff --stat` 仅含 `crates/rdc-input/**`、`logs/**`、`docs/决策待办.md` | `git diff --stat` |

## 不做（记待办）

- 真实 `SendInput` 的自动化断言会移动本机鼠标/键盘，**单测只验证构造出的 INPUT 标志**，不做真实注入。
- 多屏**逐显示器枚举**接口（当前用虚拟桌面原点/尺寸，负原点已覆盖左右上下屏拼接）；
  每显示器 DPI 混合场景需枚举，留待接入 `rdc-core` 时按接口补齐。
- `rdc-core` 尚无 `rdc-in` 线程与 tx 环接线：本块只交付 crate 内能力，
  加密（`rdc-security` 的 AEAD）与 P0 插队（`StreamQueues`）已存在，接线另行记录。
