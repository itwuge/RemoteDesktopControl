# S4-3 文件传输最小版判据（先落盘，后实现）

日期：2026-11-07
阶段：S4 第三块 S4-3
范围：新增 `crates/rdc-filetransfer`（分块、进度回调、断点续传、SHA-256 校验）
依据：docs/10 §5（4 MB 分块、逐块校验、断点续传、失败块重传 ≤3 次）
差异：docs/10 写 BLAKE3，本任务书要求 SHA-256 —— 按任务书用 SHA-256，差异记 D38 待裁决。

## 判据

| 编号 | 判据 | 阈值 / 期望 | 数据来源 |
|------|------|-------------|----------|
| S4-3-a | 分块计划 | 0 字节 → 0 块；不足一块 → 1 块；正好整数倍不产生空尾块；末块长度正确 | `cargo test -p rdc-filetransfer plan::` |
| S4-3-b | 往返完整性 | 随机内容按 7 字节块往返，逐字节一致；整文件 SHA-256 与发送端一致 | `cargo test -p rdc-filetransfer transfer::` |
| S4-3-c | 逐块校验 | 篡改块内容 → `ChunkHashMismatch`，该块不计入已收 | 同上 |
| S4-3-d | 断点续传 | 已确认块重连后不重发；接收端按状态位续写，最终哈希一致 | 同上 |
| S4-3-e | 进度回调 | 每次确认触发一次回调，`chunks_done/bytes_done` 单调不减，终值等于总数 | 同上 |
| S4-3-f | 重复/缺块 | 重复块 → `Duplicate` 不重复计数；缺块 finish → `MissingChunks(n)` | 同上 |
| S4-3-g | 状态编解码 | 位图编解码往返一致；长度不符 → `MalformedState` | `cargo test -p rdc-filetransfer state::` |
| S4-3-h | 独立目录 | 新 crate 不依赖 transport/capture/codec/decode，不进入音视频热路径 | `cargo check -p rdc-filetransfer` + 依赖清单 |

## 不做（记待办）

- 真正的网络收发与重连触发：本块只交付可独立测试的传输状态机，接线在 rdc-core。
- 失败块重传策略（≤3 次）只在发送端提供 `request_chunk(index)` 接口，退避与计数由调用方实现。
- 磁盘落盘的原子性（`.part` → 重命名）与权限/路径安全检查（docs/10 §5）不在本块。
- 多文件打包、目录递归、压缩均不做。
