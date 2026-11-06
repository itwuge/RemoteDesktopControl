# S1 基准基线（Rust）

- 采集时间：2026-11-07
- 环境：Windows / rustc 1.99.1 stable / release（`cargo bench --workspace`）
- 命令：`cargo bench --workspace`

| 用例 | 结果 | 说明 |
| --- | --- | --- |
| `envelope_encode`（rdc-protocol） | 293.2 ns/op，平均 146 bytes | 信令信封 JSON 序列化 |
| `envelope_decode`（rdc-protocol） | 1057.9 ns/op | 信令信封 JSON 反序列化 |
| `pacer_new+target`（rdc-transport） | 0.3 ns/op | 构造与读取被优化为常量，仅验证骨架可跑 |
| `fixed_controller`（rdc-transport） | 52.8 ns/op | `Box<dyn>` 构造 + 目标码率读取 |
| `fec_encode` | n/a | stage3 实现后补充 |
| `gcc_on_ack` | n/a | stage4 实现后补充 |

结论：S1 仅建立可运行基线与回滚对照点，不设性能门槛；FEC/拥塞控制实现后必须在同一文件更新。
