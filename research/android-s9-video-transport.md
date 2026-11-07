# 阶段 5 设计：媒体包接入 `video_ingest`

层级：研究/设计文档（不进入 `docs/`）
状态：已实现最小接缝；真实 socket 归属留给会话层（见 §8）

## 1. 为什么不复用控制通道

控制通道是 QUIC 单条双向流 + “计数器帧” AEAD（`rdc-connect::session`）：

* 可靠、有序、无丢包语义；视频需要 UDP 式“可丢 + NACK/FEC + 乱序重排”。
* 控制帧 nonce 由**流内单调计数器**派生，乱序解密会破坏计数器语义；媒体包天然乱序。
* 控制通道与媒体共享 QUIC 连接会让一条拥塞队列同时承载 P0 输入与视频，违背
  `StreamKind` 的优先级表（docs/04 §2）。
* 控制帧上限 4 KB 与 `MAX_FRAME_LEN` 绑定，媒体包上限是 1400 字节的 MTU 约束。

结论：媒体走 `rdc_transport::UdpTransport` 的 32 字节包头路径（与主干同一条线格式），
控制通道保持原样。

## 2. 线格式：S4 头 + AEAD + 视频消息

```
0        32                        32+payload_len        32+payload_len+16
+--------+-------------------------+---------------------+
| S4 头  | 视频消息明文(0x61~0x64)  | AEAD tag (16 字节)  |
+--------+-------------------------+---------------------+
```

* `S4 头`：`rdc_protocol::PacketHeader`（魔数 `RD`、版本 1、stream、seq、ts_us、frame_id）。
  Media 通道只接受 `stream = VideoMain(0)` 或 `VideoSub(7)`，其它通道计数丢弃。
* 载荷：**一条完整视频协议消息**（`TAG_VIDEO_META/FRAME/FRAGMENT/DONE`）。
  *不叠加* `rdc-transport::FragmentHeader`（那套 8 字节分片头是给“整帧 >1352”准备的）。
  视频消息本身已有 `frame_id + frag_index/frag_count` 分片，两套分片混用会产生双重重排窗口，
  因此明确只允许一套：**分片在 `rdc-protocol::video` 内完成**。
* `payload_len` 上限沿用 `FRAGMENT_PAYLOAD_MAX = 1352`，保证 `32 + 1352 + 16 = 1400`。
  单条视频分片消息固定头最大 13 字节，因此每片 Annex-B 实际可用 1352 - 13 = 1339 字节，
  `VideoMessage::Fragment` 编码前已按 `VIDEO_FRAGMENT_BYTES = 1352` 校验（消息总长小于 MTU，
  见 §2.1 的收口说明）。

### 2.1 已知偏差（需要协议下一版收口）

`VIDEO_FRAGMENT_BYTES` 限制的是“分片数据”而不是“整条消息”，因此一条 `Fragment`
消息最长为 `5(固定头) + 1352(数据) = 1357` 字节，超出 1352 的明文上限 5 字节。
当前实现策略：

1. 发送端通过 `media_budget()` 计算单包可用数据长度（1352 - 固定头），保证不产生 >1400 的 UDP 包；
2. 接收端对 `payload_len > 1352` 的包直接计数丢弃，不截断；
3. 若后续把 `VIDEO_FRAGMENT_BYTES` 统一改为 1339，则线与消息上限对齐，属于**协议次版本**变更，
   旧端按本条策略可继续互通。

## 3. nonce 方向隔离

`rdc_security::ChannelKeys` 已经派生 `media_c2h` / `media_h2c` 两把独立密钥：

* 被控端→主控端（视频下行）用 `media_h2c`；
* 主控端→被控端（未来摄像头/上行）用 `media_c2h`。

在此基础上再叠加**显式方向字节**，防止“密钥配置错位”时两个方向碰撞：

```
nonce[0..4] = [0, 0, 0, direction]   // direction: 1 = h2c, 2 = c2h
nonce[4..12] = counter.to_be_bytes() // 见下
counter = (frame_id as u64) << 32 | seq as u64
```

* `seq` 是包头里的通道序号（u32），`frame_id` 是帧号（u16）。两者都在包头里，
  接收端无需额外状态即可复算 nonce；组合后 48 位在 24k 包/秒下可用约 370 年。
* 不能只用 `seq`：u32 回绕会导致 nonce 复用。
* 不能只靠“方向字节”而不换密钥：跨通道密钥隔离是 docs/07 §3 的硬约束。

## 4. 认证范围

* **每个包独立 AEAD**：密文覆盖“视频消息明文 + 16 字节 tag”，不做跨分片连续认证。
  理由：UDP 乱序/丢包，跨分片认证在缺片时无法验证，且会让重传放大开销。
* **AAD = 32 字节 S4 头**：`stream/seq/ts_us/frame_id/payload_len` 任一被篡改都会解密失败。
  接收端先解头再解密，且必须在 AEAD 通过后才把包送入重组。
* 重组层（`VideoReceiver`）只接收已认证的明文，因此“伪造一个 Fragment 打乱 frame_id”
  无法通过 AEAD。
* 重放：媒体不维护 64 位重放窗口（视频包可合法重复/重传），由解码层丢弃重复帧；
  控制通道的重放窗口保持不变。若未来要求防重放攻击，按 `(frame_id, seq)` 位图另开设计。

## 5. 时间戳映射

* `PacketHeader.ts_us` 是采集端单调时钟微秒，**线上不改写**。
* `VideoFrameHeader.pts_us` 与它同源；接收端以包头为准做一致性校验：
  * 一致 → 原样进入重组；
  * 不一致 → 以包头值覆盖帧头的 `pts_us`（保证端到端延迟统计不被旧端/错误实现污染），
    并累加 `ts_mismatches`。
* `pts_us` 只用于延迟与抖动观测，**不参与重排**；重排只依赖 `frame_id`。

## 6. 多码流与分辨率切换

* 能力位：新增 `bit14 = VIDEO_STREAM`（H.264 视频流）。`KNOWN_BITS` 扩到 15 位。
  `DEFAULT` 不含 bit14，由真正有视频实现的端显式 opt-in（Android `local_capabilities`）。
* 码流编号：`stream_id = 0` 主码流；`1` 起为子码流（上限 `VIDEO_MAX_STREAMS`）。
  子码流复用既有的 `bit7 = SUB_STREAM`（屏幕墙/子码流能力）。
* 选择语义：接收端 `video_select_stream(id)` 只接收该 `stream_id` 的消息；
  `None` 表示不过滤（兼容旧调用）。
* 分辨率/码率切换**不是新消息类型**，而是新的 `Meta`：
  发送端递增 `config_id`，接收端 `requires_rebuild` 命中 → 清空在途分片 →
  触发一次 `ConfigChanged` → 等下一个 IDR 才恢复出帧。
* 旧端行为：不认识 `VIDEO_STREAM` 的端协商后位为 0，不应建立视频通道；
  若旧端仍按“收到 Meta 即认为有视频”的隐式判定，会照常解码（bit 只影响**是否发起**，
  不影响已建立通道的解码兼容性）。

## 7. 兼容性说明（旧端）

| 改动 | 旧端表现 |
| --- | --- |
| 能力位 bit14 | 视为未知位，协商后被裁掉 → 不启用视频通道，不影响其它功能 |
| 握手 `Hello/HelloAck` 增加可选 `caps` | 缺省字段，旧端不发送；新端 `None` 时回退“旧端默认能力”（无 bit13/bit14） |
| 媒体包 AEAD | 旧端明文包在新端按“密钥缺失/认证失败”计数丢弃，不 panic；旧端收到密文包同理丢弃 |
| 视频消息 tag 0x61~0x64 | 未定义 tag 的旧端按未知消息丢弃并计数 |

握手增加 `caps` 不改变签名范围（签名只覆盖 `session/eph/ts`）。理由是握手本身运行在
TLS 1.3 over QUIC + SPAKE2 之上，通道已被认证；对端谎报自己的能力只会让双方少启用功能，
不能放大权限。

## 8. 仍缺项

* **缺代码**：媒体 socket 的归属者（会话层）尚未把 `UdpTransport::recv_from` 的循环接到
  `video_ingest_datagram`；Android 侧还没有真实信令/直连客户端。
* **缺协议**：`VIDEO_FRAGMENT_BYTES` 与整包上限的 5 字节偏差（§2.1）；媒体重放窗口未定义。
* **缺设备**：真机上的 UDP 收包、MediaCodec 直出、5 分钟长跑仍需设备验证。
