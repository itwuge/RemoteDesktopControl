# inline `gate-allow` 审计清单（截至批次 D 第一段）

> 生成日期：2026-11-08
> 口径：`scripts/gate.ps1` 要求 `gate-allow <RULE>: <理由>` 与违规代码**同一行**；本清单由 `grep gate-allow` 逐行核对生成，共 **31 条**（C1=3、C2=8、D1=20）。
> 判定人：本会话（AI 判定）；用户抽查。凡标「AI」均由本会话逐条读上下文后判定，未引用他人结论。
> 目的：确认没有用放行掩盖「真吞错」。真吞错一律改显式分支（见文末 §3），只有下述刻意 best-effort 才允许放行。

## 1. 汇总

| 来源 | 条数 | 类别分布 |
|---|---|---|
| C1（批次 C 前 7 处 `.ok()`） | 3 | MFT 能力探测 3 |
| C2（15 处 `unwrap_or_default()`） | 8 | Option/缺失语义 7、热路径时序统计 1 |
| D1（本批 master 欠账 + 小 crate） | 20 | 测试收尾 12、启动日志 2、rustls 进程级安装 2、QUIC 写半边关闭 3、`should_panic` 用例 1 |
| **合计** | **31** | |

## 2. 逐条清单

### C1（3 条）

| # | 文件:行 | 原代码形态 | 放行理由 | 判定 |
|---|---|---|---|---|
| 1 | `crates/rdc-codec/src/platform/windows/mf.rs:279` | `let events: Option<IMFMediaEventGenerator> = transform.cast().ok();` | 同步 MFT 没有事件源，`cast` 失败是能力探测的合法结果，不是错误 | AI |
| 2 | `crates/rdc-codec/src/platform/windows/mf.rs:342` | `let codec_api: Option<ICodecAPI> = transform.cast().ok();` | 部分 MFT 不实现 `ICodecAPI`，`None` 表示合法能力缺失 | AI |
| 3 | `crates/rdc-decode/src/platform/windows/mf_decoder.rs:451` | `let events: Option<IMFMediaEventGenerator> = transform.cast().ok();` | 同 #1，解码 MFT 无事件源 | AI |

### C2（8 条）

| # | 文件:行 | 原代码形态 | 放行理由 | 判定 |
|---|---|---|---|---|
| 4 | `crates/rdc-bridge/src/api.rs:1405` | `.unwrap_or_default()` | 审计字段 `old_value` 协议可空，缺失即空串 | AI |
| 5 | `crates/rdc-bridge/src/api.rs:1410` | `.unwrap_or_default()` | 审计字段 `new_value` 协议可空，缺失即空串 | AI |
| 6 | `crates/rdc-bridge/src/clipboard_service.rs:220` | `Ok(text.unwrap_or_default())` | `read_text` 已成功，`None` 表示剪贴板非文本（空/图片），属正常状态 | AI |
| 7 | `crates/rdc-bridge/src/clipboard_service.rs:238` | `unwrap_or_default()` | 本机无文本时返回空快照，属「无内容可同步」 | AI |
| 8 | `crates/rdc-bridge/src/clipboard_service.rs:271` | `last_hash.map(hex).unwrap_or_default()` | 尚未同步过条目，空串即「无哈希」正常值 | AI |
| 9 | `crates/rdc-bridge/src/control_service.rs:1127` | `in_flight.pop_front().unwrap_or_default()` | 只影响单包时序统计；热路径不逐帧打日志，缺失样本用零值 | AI |
| 10 | `crates/rdc-bridge/src/session_service.rs:203` | `Ok(last.clone().unwrap_or_default())` | 从未建立会话时没有上次报告，空报告是该分支正常取值 | AI |
| 11 | `crates/rdc-decode/src/video_decoder.rs:36` | `std::env::var("RDC_DECODER").unwrap_or_default()` | 环境变量未设置是正常默认路径，空串落到默认分支 | AI |

### D1（20 条，本批新增）

| # | 文件:行 | 原代码形态 | 放行理由 | 判定 |
|---|---|---|---|---|
| 12 | `crates/rdc-bridge/src/api.rs:134` | `let _ = std::fs::create_dir_all(parent);` | 启动日志目录创建属尽力而为，失败不得影响启动（函数语义即「写失败一律静默」） | AI |
| 13 | `crates/rdc-bridge/src/api.rs:143` | `let _ = writeln!(file, ...);` | 启动日志写失败属尽力而为，日志仅供排障，不影响启动 | AI |
| 14 | `crates/rdc-bridge/src/api.rs:1942` | `let _ = std::fs::remove_dir_all(&dir);` | 测试前置清理固定临时目录，失败不影响断言结论 | AI |
| 15 | `crates/rdc-bridge/src/api.rs:1948` | `let _ = std::fs::remove_dir_all(&dir);` | 测试收尾删除固定临时目录，失败不影响断言结论 | AI |
| 16 | `crates/rdc-core/src/s5/settings.rs:364` | `let _ = std::fs::remove_dir_all(&dir);` | 测试收尾删除独立临时目录，失败不影响断言结论 | AI |
| 17 | `crates/rdc-core/src/s5/settings.rs:379` | 同上 | 同上 | AI |
| 18 | `crates/rdc-core/src/s5/settings.rs:395` | 同上 | 同上 | AI |
| 19 | `crates/rdc-core/src/s5/settings.rs:414` | 同上 | 同上 | AI |
| 20 | `crates/rdc-core/src/s5/settings.rs:434` | 同上 | 同上 | AI |
| 21 | `crates/rdc-core/src/s5/settings.rs:479` | 同上 | 同上 | AI |
| 22 | `crates/rdc-connect/src/quic.rs:76` | `let _ = ring::default_provider().install_default();` | rustls 0.23 进程级 CryptoProvider；已安装时返回 `Err(already installed)`，重复安装无害 | AI |
| 23 | `crates/rdc-connect/src/quic.rs:112` | 同上 | 同上 | AI |
| 24 | `crates/rdc-connect/src/session.rs:202` | `let _ = self.send.finish();` | 关闭路径已无恢复动作，`finish` 失败由随后的 `conn.close` 一并收敛 | AI |
| 25 | `crates/rdc-connect/src/signal_client.rs:104` | `let _ = send.finish();` | 写半边的关闭失败会由随后的 `read_frame` 以连接错误暴露，无需重复处理 | AI |
| 26 | `crates/rdc-connect/src/signal_client.rs:166` | `let _ = send.finish();` | 同上 | AI |
| 27 | `crates/rdc-filetransfer/src/digest.rs:82` | `let _ = std::fs::remove_file(&path);` | 测试收尾删除临时文件，失败不影响断言结论 | AI |
| 28 | `crates/rdc-filetransfer/src/receiver.rs:243` | 同上 | 同上 | AI |
| 29 | `crates/rdc-filetransfer/src/receiver.rs:269` | 同上 | 同上 | AI |
| 30 | `crates/rdc-filetransfer/src/receiver.rs:270` | 同上 | 同上 | AI |
| 31 | `crates/rdc-runtime/src/pool.rs:222` | `let _ = FramePool::new(0, 16);` | `#[should_panic]` 用例，构造即应 panic，丢弃返回值是本用例目的 | AI |

## 3. 同一批中**未放行、改显式分支**的真吞错

| 文件:行 | 原形态 | 改法 | 行为变化 |
|---|---|---|---|
| `crates/rdc-core/src/s5/settings.rs:259`（写入失败分支） | `let _ = std::fs::remove_file(&temp);` | `let cleanup = std::fs::remove_file(&temp);`，`match` 把清理错误并入返回的 `SettingsError::Io` | 主错误仍为写入失败；清理也失败时错误文本追加「清理临时文件失败：…」，不再静默 |
| `crates/rdc-core/src/s5/settings.rs:270`（rename 失败分支） | 同上 | 同上 | 同上，追加「重命名失败后清理临时文件失败：…」 |
| `crates/rdc-input/src/platform/linux.rs:306` | `let _ = libc::ioctl(..., UI_DEV_DESTROY);` | `if ioctl < 0 { debug!(error, "Drop 阶段销毁 uinput 设备失败…") }` | 失败时多一条 debug 日志；`Drop` 仍无法向上传播。**Linux-only，需 Linux 线复核** |
| `crates/rdc-input/src/platform/linux.rs:805` | `let _ = device.move_absolute(...);` | `if let Err(err) = ... { debug!(x, y, error, "无校正源时绝对移动失败") }` | 失败时多一条 debug 日志，控制流不变。**Linux-only，需 Linux 线复核** |

## 4. 抽查要点

1. 12 条测试收尾（#14–21、#27–30）只存在于 `#[cfg(test)]` 模块，删除的是用例自己建的临时路径，失败不影响断言——不涉及产品错误路径。
2. 7 条 Option/缺失语义（#4–8、#10–11）的 `None` 是协议或业务定义的「无此字段/无内容」正常态，不是被吞掉的 `Err`。
3. 3 条 MFT 能力探测（#1–3）的 `cast().ok()` 失败等价于「该 MFT 不需要事件源/不实现该接口」，属探测结果。
4. 2 条启动日志、2 条 rustls 进程级安装、3 条 QUIC 写半边关闭：均属重复执行无害或由后续调用链兜底，无静默丢失的可诊断信息。
5. 如需推翻任一条，请给出具体反例（什么情况下该 `Err` 会导致用户可见故障），我会改回显式分支并补日志，而不是继续放行。
