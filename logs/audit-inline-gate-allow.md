# inline `gate-allow` 审计清单（截至批次 E 段 d）

> 生成日期：2026-11-08
> 口径：`scripts/gate.ps1` 要求 `gate-allow <RULE>: <理由>` 与违规代码**同一行**；本清单由 `grep gate-allow` 逐行核对生成，共 **121 条**（C1=3、C2=8、D1=20、D2=51、E=39）。
> 判定人：本会话（AI 判定）；用户抽查。凡标「AI」均由本会话逐条读上下文后判定，未引用他人结论。
> 目的：确认没有用放行掩盖「真吞错」。真吞错一律改显式分支（见文末 §3），只有下述刻意 best-effort 才允许放行。

## 1. 汇总

| 来源 | 条数 | 类别分布 |
|---|---|---|
| C1（批次 C 前 7 处 `.ok()`） | 3 | MFT 能力探测 3 |
| C2（15 处 `unwrap_or_default()`） | 8 | Option/缺失语义 7、热路径时序统计 1 |
| D1（master 欠账 + 小 crate） | 20 | 测试收尾 12、启动日志 2、rustls 进程级安装 2、QUIC 写半边关闭 3、`should_panic` 用例 1 |
| D2（clipboard/signal/core/bridge） | 51 | 资源收尾 7、线程回收与句柄关闭 13、诊断输出 5、设计性丢弃/降级 10、测试收尾 4、网络对端离线 5、按键复位 3、非错误值 4 |
| E（capture/decode/codec/cli） | 39 | `Drop` 收尾 9、线程回收/退出 5、构造失败收尾 1、显示器枚举降级 1、QPC 降级 3、DPI 重复声明 2、窗口可见状态语义 1、诊断/夹具输出 14、超时后无人接收 3、非错误值/仅建队列 4 |
| **合计** | **121** | |

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

## 5. D2 新增（51 条）

### D2a · rdc-clipboard（7 条，Windows API 资源收尾）

| 文件:行 | 原代码形态 | 放行理由 | 判定 |
|---|---|---|---|
| `crates/rdc-clipboard/src/platform/windows.rs:81` | `let _ = GlobalUnlock(hglobal);` | 读取已完成，解锁失败只代表引用计数已归零，无可恢复动作 | AI |
| `.../windows.rs:84` | `let _ = CloseClipboard();` | 关闭剪贴板必须无条件尝试，失败不影响已读到的文本结果 | AI |
| `.../windows.rs:98` | `let _ = GlobalFree(hmem);` | `GlobalLock` 失败后的释放属收尾，主错误随后返回 | AI |
| `.../windows.rs:102` | `let _ = GlobalUnlock(hmem);` | 交给系统前必须尝试解锁，失败会由 `SetClipboardData` 自行报错 | AI |
| `.../windows.rs:105` | `let _ = GlobalFree(hmem);` | 打开剪贴板失败的收尾释放，主错误随后返回 | AI |
| `.../windows.rs:115` | `let _ = CloseClipboard();` | 关闭剪贴板必须无条件尝试，失败不影响写入结果 | AI |
| `.../windows.rs:117` | `let _ = GlobalFree(hmem);` | `SetClipboardData` 失败时释放所有权内存属收尾，主错误随后返回 | AI |

### D2a · rdc-signal（5 条）

| 文件:行 | 原代码形态 | 放行理由 | 判定 |
|---|---|---|---|
| `crates/rdc-signal/src/server.rs:138` | `let _ = install_default();` | 进程级 CryptoProvider 已装好时返回 `Err`，重复安装无害 | AI |
| `.../server.rs:681` | `let _ = push_to_peer(...)` | 队列满时内部已记 `warn`，`false` 表示对端离线，无需上层重复处理 | AI |
| `.../server.rs:690` | `let _ = push_to_peer(...)` | 同上 | AI |
| `.../server.rs:737` | `let _ = push_to_peer(...)` | 同上 | AI |
| `.../server.rs:788` | `let _ = push_to_peer(...)` | 同上 | AI |

> 本组另有 4 条改显式分支（非放行）：`server.rs` 2 处注册/通用响应写入 + 2 处 oneshot 通知，失败时补 `debug` 日志。

### D2b · rdc-core（4 条，均为测试收尾）

| 文件:行 | 原代码形态 | 放行理由 | 判定 |
|---|---|---|---|
| `crates/rdc-core/src/s5/devices.rs:442` | `let _ = remove_dir_all(&dir);` | `#[cfg(test)]` 收尾删除独立临时目录，失败不影响断言结论 | AI |
| `.../devices.rs:462` | 同上 | 同上 | AI |
| `.../devices.rs:496` | 同上 | 同上 | AI |
| `crates/rdc-core/src/s5/grant.rs:2293` | 同上 | 同上 | AI |

### D2c · rdc-bridge / control_service.rs（18 条）

| 文件:行 | 原代码形态 | 放行理由 | 判定 |
|---|---|---|---|
| `.../control_service.rs:374` | `let _ = running.join.map(...)` | 回收旧管线线程，panic 已由 panic hook 记录，无恢复动作 | AI |
| `.../control_service.rs:391` | `let _ = setup_tx.send(Ok(...))` | 启动结果回传通道接收端可能已超时退出，不影响线程继续 | AI |
| `.../control_service.rs:395` | `let _ = setup_tx.send(Err(...))` | 同上，接收端已超时时由 `start()` 返回超时错误 | AI |
| `.../control_service.rs:403` | `let _ = join.join();` | 回收已报错线程，具体错误已由 setup 通道取回并返回 | AI |
| `.../control_service.rs:456` | `let _ = join.join();` | 文档约定 join 失败不视为业务错误 | AI |
| `.../control_service.rs:738` | `let _ = SetMultithreadProtected(true)` | D3D11 线程保护为可选优化，失败按单线程约束使用 | AI |
| `.../control_service.rs:1435` | `let _ = writeln!(trace...)` | `RDC_FRAME_TRACE` 诊断 CSV，写失败不得中断预览 | AI |
| `.../control_service.rs:1451` | `let _ = SetEvent(events[index])` | 呈现同步事件设置失败无恢复动作，下一帧继续尝试 | AI |
| `.../control_service.rs:1485` | `let _ = writeln!(metrics...)` | 逐秒指标 CSV 属诊断输出，写失败不得中断预览 | AI |
| `.../control_service.rs:1543` | `let _ = readback_us;` | 非错误值，预留计时变量显式消费以避免未使用告警 | AI |
| `.../control_service.rs:1551` | `let _ = join.join();` | 编码线程已收到停止标志，join 失败不改变收尾结论 | AI |
| `.../control_service.rs:1556` | `let _ = CloseHandle(events[0])` | 线程退出时关闭事件句柄，失败仅可能由句柄已失效引起 | AI |
| `.../control_service.rs:1557` | `let _ = CloseHandle(events[1])` | 同上 | AI |
| `.../control_service.rs:1812` | `let _ = ManuallyDrop::take(...)` | 非错误值，take 只为结束借用并释放输入纹理所有权 | AI |
| `.../control_service.rs:1972` | `let _ = QueryPerformanceFrequency(...)` | 失败时 `value` 保持 0，`qpc_us` 已降级返回 0 | AI |
| `.../control_service.rs:1978` | `let _ = QueryPerformanceCounter(...)` | 失败时 `counter` 保持 0，`qpc_us` 已降级返回 0 | AI |
| `.../control_service.rs:2002` | `let _ = writeln!(header)` | 明细 CSV 表头属诊断输出，失败不影响预览 | AI |
| `.../control_service.rs:2023` | `let _ = writeln!(header)` | 指标 CSV 表头属诊断输出，失败不影响预览 | AI |

### D2c · rdc-bridge / control_service_linux.rs（8 条，**Linux-only，需 Linux 线复核**）

| 文件:行 | 原代码形态 | 放行理由 | 判定 |
|---|---|---|---|
| `.../control_service_linux.rs:216` | `let _ = guard.take();` | 非错误值，清空已退出服务的状态槽，旧 Service 随 take 释放 | AI |
| `.../control_service_linux.rs:249` | `let _ = host_main(...)` | 线程入口把错误写入共享快照，返回值无需重复上报 | AI |
| `.../control_service_linux.rs:258` | `let _ = client_main(...)` | 同上 | AI |
| `.../control_service_linux.rs:304` | `let _ = service.host.join()` | 停链路时回收 host 线程，join 失败不改变停止结论 | AI |
| `.../control_service_linux.rs:305` | `let _ = service.client.join()` | 同上，回收 client 线程 | AI |
| `.../control_service_linux.rs:529` | `let _ = window_frames;` | 非错误值，预留计数变量显式消费以避免未使用告警 | AI |
| `.../control_service_linux.rs:544` | `let _ = Packet{...}.write_to(...)` | 冲刷尾包到已建立的流，写失败说明对端已断开 | AI |
| `.../control_service_linux.rs:552` | `let _ = Packet{BYE}.write_to(...)` | 结束通知写给已建立的流，写失败说明对端已断开 | AI |

### D2c · rdc-bridge / 其余文件（9 条）

| 文件:行 | 原代码形态 | 放行理由 | 判定 |
|---|---|---|---|
| `.../input_service.rs:400` | `let _ = join.join();` | 注入线程已收到停止标志，join 失败不改变清理结论 | AI |
| `.../input_service.rs:407` | `let _ = injector.inject(Key(...))` | 停止时必须尽力抬起按键，单键失败继续处理其余按键 | AI |
| `.../input_service.rs:418` | `let _ = injector.inject(Pointer(...))` | 停止时必须尽力抬起鼠标键，单键失败继续处理其余按键 | AI |
| `.../session_service.rs:506` | `let _ = GetProcessHandleCount(...)` | 句柄计数仅用于诊断展示，失败时保持 0 | AI |
| `.../wall_service.rs:177` | `let _ = join.join();` | 回收上一轮投影线程，panic 已由 panic hook 记录 | AI |
| `.../wall_service.rs:231` | `let _ = join.join();` | 投影线程已收到停止标志，join 失败不改变停止结论 | AI |
| `.../wall_service.rs:296` | `let _ = self.tx.try_send(...)` | 投影有界队列满时丢当前帧是设计行为，背压由上游采集降速 | AI |
| `.../wall_service.rs:468` | `let _ = thumbnail.flush();` | 线程退出前冲刷编码器尾包，失败无恢复动作 | AI |
| `.../wall_service.rs:471` | `let _ = decoder.flush();` | 同上，不影响已呈现的投影帧 | AI |

> 本组另有 1 条改显式分支（非放行）：`file_service.rs` 续传成功后清理断点状态文件失败 → `debug` 日志 + 说明「下次传输可能回退到完整校验」，不改变返回结果与用户文案。

## 6. E 段新增（17 条）

> 说明：本段按用户要求压缩放行比例，热路径优先真改。capture 13 条中 9 条放行、decode 16 条中 5 条放行、codec 30 条中 3 条放行；其余 47 条全部改为显式分支（传播或 `debug` 留痕），明细见 `docs/20-变更记录.md` 第 45–47 行。

### E-a · rdc-capture（9 条）

| # | 文件:行 | 原代码形态 | 放行理由（具体依据） | 判定 |
|---|---|---|---|---|
| 83 | `crates/rdc-capture/src/platform/windows.rs:372` | `let _ = handle.join();` | `stop()` 无返回值可传播；线程 panic 已由 Rust 默认 panic hook 输出栈 | AI |
| 84 | `crates/rdc-capture/src/platform/windows.rs:1639` | `let _ = DeleteDC(mem_dc);` | `SelectObject` 失败路径的收尾释放，主错误 `CaptureError::Internal` 随后返回 | AI |
| 85 | `crates/rdc-capture/src/platform/windows.rs:1711` | `let _ = DeleteObject(self.dib);` | `Drop` 无返回值可传播，GDI 句柄释放失败不改变析构结论 | AI |
| 86 | `crates/rdc-capture/src/platform/windows.rs:1712` | `let _ = DeleteDC(self.mem_dc);` | 同上，Drop 阶段资源释放属尽力而为 | AI |
| 87 | `crates/rdc-capture/src/platform/windows.rs:1735` | `let _ = EnumDisplayMonitors(...)` | 失败时回调未执行→返回空列表，`start()`/`probe()` 随即返回 `NoActiveDisplay`（主错误可观测） | AI |
| 88 | `crates/rdc-capture/src/platform/windows.rs:1887` | `let _ = QueryPerformanceFrequency(&mut f);` | 失败时 `f` 保持 0，紧随其后的分支已降级为 1 | AI |
| 89 | `crates/rdc-capture/src/platform/windows.rs:1897` | `let _ = QueryPerformanceCounter(&mut counter);` | 失败时 `counter` 保持 0，仅使本帧时间戳降级为 0，不影响帧内容 | AI |
| 90 | `crates/rdc-capture/src/platform/windows.rs:1906` | `let _ = SetProcessDpiAwarenessContext(...)` | 宿主已声明 DPI 感知时必然返回失败，属正常路径 | AI |
| 91 | `crates/rdc-capture/src/platform/linux.rs:373` | `let _ = handle.join();` | 同 #83（**Linux-only，需 Linux 线复核**） | AI |

### E-b · rdc-decode（5 条）

| # | 文件:行 | 原代码形态 | 放行理由（具体依据） | 判定 |
|---|---|---|---|---|
| 92 | `crates/rdc-decode/src/present.rs:252` | `let _ = ShowWindow(hwnd, SW_SHOW);` | `ShowWindow` 返回的是窗口此前的可见状态，0 不代表调用失败 | AI |
| 93 | `crates/rdc-decode/src/present.rs:736` | `let _ = DestroyWindow(self.hwnd);` | `Drop` 无返回值可传播，窗口销毁失败随进程退出收敛 | AI |
| 94 | `crates/rdc-decode/src/platform/windows/mf_decoder.rs:1076` | `let _ = self.transform.ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, 0);` | `Drop` 无返回值可传播，MFT 即将随解码器析构 | AI |
| 95 | `.../mf_decoder.rs:1078` | `let _ = self.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);` | `Drop` 无返回值可传播，析构阶段的结束流通知属尽力而为 | AI |
| 96 | `.../mf_decoder.rs:1079` | `let _ = self.transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);` | 同上，析构阶段冲刷失败不影响对象释放 | AI |

### E-c · rdc-codec（3 条）

| # | 文件:行 | 原代码形态 | 放行理由（具体依据） | 判定 |
|---|---|---|---|---|
| 97 | `crates/rdc-codec/src/platform/windows/amf.rs:159` | `let _ = FreeLibrary(self._lib);` | `Drop` 无返回值可传播，进程退出时系统回收已加载 DLL | AI |
| 98 | `crates/rdc-codec/src/platform/windows/mf.rs:987` | `let _ = self.transform.ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);` | `Drop` 无返回值可传播，MFT 随 COM 引用计数释放 | AI |
| 99 | `crates/rdc-codec/src/platform/linux.rs:209` | `let _ = self.child.kill();` | `Drop` 兜底击杀，紧随的 `shutdown()` 会再次回收并记录失败（**Linux-only，需 Linux 线复核**） | AI |

### E-d · rdc-cli（22 条）

| # | 文件:行 | 原代码形态 | 放行理由（具体依据） | 判定 |
|---|---|---|---|---|
| 100 | `crates/rdc-cli/src/s3a.rs:145` | `let _ = writeln!(writer, "{line}");` | 诊断 CSV 逐行写入，函数契约即忽略失败，不得中断实测主流程 | AI |
| 101 | `crates/rdc-cli/src/s3a.rs:158` | `let _ = writer.flush();` | 收尾冲刷诊断日志，失败不影响已完成的实测结论 | AI |
| 102 | `crates/rdc-cli/src/s3a.rs:304` | `let _ = done_tx.send(result);` | 主线程超时后已返回 watchdog 错误，结果无人接收属预期 | AI |
| 103 | `crates/rdc-cli/src/s3a.rs:397` | `let _ = TranslateMessage(&msg);` | 失败仅表示无键盘消息可翻译，`DispatchMessage` 仍照常分发 | AI |
| 104 | `crates/rdc-cli/src/s3a.rs:1062` | `let _ = ShowWindow(...);` | 返回此前可见状态，非失败指示 | AI |
| 105 | `crates/rdc-cli/src/s3a.rs:1063` | `let _ = SetTimer(...);` | 定时器仅驱动夹具窗口重绘，失败不影响采集/编码验收 | AI |
| 106 | `crates/rdc-cli/src/s3a.rs:1150` | `let _ = InvalidateRect(...);` | 重绘提示失败只影响夹具窗口观感，不影响验收 | AI |
| 107 | `crates/rdc-cli/src/s3a.rs:1180` | `let _ = FillRect(...);` | 夹具窗口彩条绘制，失败不影响采集/编码验收 | AI |
| 108 | `crates/rdc-cli/src/s3a.rs:1191` | `let _ = FillRect(...);` | 夹具窗口移动白块绘制，失败不影响采集/编码验收 | AI |
| 109 | `crates/rdc-cli/src/s3a.rs:1192` | `let _ = EndPaint(...);` | 与 `BeginPaint` 配对，失败无恢复动作，窗口即将销毁 | AI |
| 110 | `crates/rdc-cli/src/s3b3.rs:365` | `let _ = sender.send(result);` | 接收端在任一实例超时/报错后已提前返回，主错误随后返回 | AI |
| 111 | `crates/rdc-cli/src/s3b4.rs:217` | `let _ = QueryPerformanceCounter(...);` | 失败时 `counter` 保持 0，紧随的 `frequency<=0` 分支返回 0 | AI |
| 112 | `crates/rdc-cli/src/s3b4.rs:218` | `let _ = QueryPerformanceFrequency(...);` | 失败时 `frequency` 保持 0，紧随分支返回 0 | AI |
| 113 | `crates/rdc-cli/src/s3b4.rs:246` | `let _ = create_dir_all(&options.out);` | 紧随其后的 `std::fs::write(...)?` 会在目录不可写时返回错误 | AI |
| 114 | `crates/rdc-cli/src/s3b4.rs:471` | 同上 | 紧随的 `std::fs::write(d2.json)?` 返回主错误 | AI |
| 115 | `crates/rdc-cli/src/s3b4.rs:655` | 同上 | 紧随的 `std::fs::write(host.json)?` 返回主错误 | AI |
| 116 | `crates/rdc-cli/src/s3b4.rs:1000` | 同上 | 紧随的 `std::fs::write(report.json)?` 返回主错误 | AI |
| 117 | `crates/rdc-cli/src/s4demo.rs:503` | `let _ = PeekMessageW(...);` | 仅用于创建线程消息队列，返回值只表示当时是否有消息 | AI |
| 118 | `crates/rdc-cli/src/s4demo.rs:507` | `let _ = tx.send((thread_id, true));` | 接收端在 `install()` 内，超时放弃时安装失败错误已返回 | AI |
| 119 | `crates/rdc-cli/src/s4demo.rs:519` | `let _ = tx.send((thread_id, false));` | 同上 | AI |
| 120 | `crates/rdc-cli/src/s4demo.rs:916` | `let _ = std::fs::remove_file(path);` | `Drop` 无返回值可传播，临时文件清理失败不影响演示结论 | AI |
| 121 | `crates/rdc-cli/src/s4demo.rs:1112` | `let _ = SetProcessDpiAwarenessContext(...);` | 进程已声明 DPI 感知时必然返回失败，属正常路径 | AI |
