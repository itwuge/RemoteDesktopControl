# NVIDIA NVENC 接入技术备忘（Windows / D3D11 / 远程桌面低延迟）

> 适用目标：1080p60、玻璃到玻璃 p99 ≤ 30 ms、D3D11 纹理直入编码器、码率可控。
> 本文只写接口与参数结论，不含项目实现代码；所有结构体/字段名以 **Video Codec SDK 13.1** 的 `nvEncodeAPI.h` 为准（旧版字段可能不存在）。
>
> 标注规则：
> - **【官方文档】**：NVIDIA 官方文档、SDK 头文件注释、SDK 样例中的明确表述；
> - **【推断】**：由接口语义、字段注释或社区实践推导，未在官方文档中直接写明，必须在目标机器实测确认。
>
> 官方文档版本说明：SDK 13.1 是 2026-07 发布的最新版，Windows 驱动门槛 610+。项目若要兼容更老的驱动，需要把 SDK/头文件降到 12.2 或 13.0（见 §1.4）。

---

## 0. 结论速查

| 问题 | 结论 | 依据 |
|---|---|---|
| 运行库从哪来 | 随 NVIDIA 显示驱动安装；x64 进程实际加载 `C:\Windows\System32\nvEncodeAPI64.dll`（文件名为实践确认）；**不要随应用分发** | 【官方文档】[PG-Intro](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/nvenc-video-encoder-api-prog-guide/) +【推断】文件名 |
| 能否动态加载 | 可以，且推荐；`LoadLibrary` + `NvEncodeAPICreateInstance` | 【官方文档】同上 |
| 版本判断 | `NvEncodeAPIGetMaxSupportedVersion` 返回 `major<<4 \| minor`，与 `NVENCAPI_VERSION` 宏**不是同一种编码** | 【官方文档】[头文件](https://github.com/FFmpeg/nv-codec-headers/blob/master/include/ffnvcodec/nvEncodeAPI.h) |
| D3D11 绑定 | `device = ID3D11Device*`，`deviceType = NV_ENC_DEVICE_TYPE_DIRECTX`，Win10+ | 【官方文档】[PG-DX11](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/nvenc-video-encoder-api-prog-guide/) |
| 纹理直入 | `NV_ENC_REGISTER_RESOURCE` + `NV_ENC_INPUT_RESOURCE_TYPE_DIRECTX`，`pitch=0`；BGRA8 用 `NV_ENC_BUFFER_FORMAT_ARGB` | 【官方文档】[头文件字段注释](https://github.com/FFmpeg/nv-codec-headers/blob/master/include/ffnvcodec/nvEncodeAPI.h)、SDK 样例 |
| RGB 直入的 codec | 官方能力表只承诺 **H.264** 支持 ARGB 输入；HEVC/AV1 需自行转 NV12/P010 | 【官方文档】[AN-Capabilities](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/nvenc-application-note/) |
| 低延迟基线 | `tuningInfo=ULTRA_LOW_LATENCY`、`preset=P1`、`frameIntervalP=1`、`zeroReorderDelay=1`、关 lookahead/multipass、`gopLength=120`（2s） | 【官方文档】头文件字段语义 |
| CBR 生效关键 | 必须设 `averageBitRate`；CBR 下 `maxBitRate` **被忽略**；`vbvBufferSize` 要按“帧数”而不是秒数给，0 表示驱动默认（通常过大） | 【官方文档】头文件字段注释、【推断】VBV 取值 |
| 输出格式 | 带起始码的 Annex-B 字节流，SPS/PPS 默认随每个 IDR/关键帧输出；API 不产出 AVCC | 【官方文档】[PG-Sequence](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/nvenc-video-encoder-api-prog-guide/)、【推断】Annex-B |
| 首帧是否零排队 | `frameIntervalP=1` + `zeroReorderDelay=1` 时无重排缓冲；但 `NvEncEncodePicture` 返回不等于编码完成，`NvEncLockBitstream` 才等到出包 | 【官方文档】+【推断】 |
| 并发会话 | 非 qualified 卡：SDK 13.1 为 12 个/系统；SDK 12.2 为 8 个；更早为 5 个 | 【官方文档】[AN-Licensing](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/nvenc-application-note/)、[论坛答复](https://forums.developer.nvidia.com/t/how-to-increase-number-of-nvenc-concurrent-sessions/169367) |
| 许可 | 可分发你自己的应用；不得单独分发 SDK/DLL；NVIDIA 不授予音视频编解码专利许可 | 【官方文档】[License](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/license/) |

---

## 1. 接入方式与版本

### 1.1 头文件与运行库

- SDK 包内提供 `Interface/nvEncodeAPI.h` 与 Windows stub 库 `Lib/win/x64/nvencodeapi.lib`；SDK 13.1 的样例与库只支持 64 位（x64/ARM64），32 位应用被建议迁移。【官方文档】[Read Me](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/read-me/)
- 运行库不是 SDK 的一部分，而是**随显示驱动安装**：64 位进程对应 `nvEncodeAPI64.dll`（在 `System32`），32 位为 `nvEncodeAPI.dll`（在 `SysWOW64`）。编程指南明确：Windows 用 `LoadLibrary()`，Linux 用 `dlopen()`。【官方文档】[PG-Intro](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/nvenc-video-encoder-api-prog-guide/)
- 因此“没有 N 卡”的机器上 `LoadLibrary` 会失败，这本身就是最可靠的“本机无 NVENC”判据；不要用驱动版本号或注册表猜测。【推断】

**结论**：用动态加载，不要静态链接 `nvencodeapi.lib`。静态导入会让进程在无驱动环境直接起不来；动态加载则可以把 NVENC 作为可选后端，失败时回退到 MF/AMF/软编。

### 1.2 动态加载的调用骨架

```c
HMODULE h = LoadLibraryW(L"nvEncodeAPI64.dll");
if (!h) { /* 无 NVIDIA 驱动或非 N 卡：回退 */ }

typedef NVENCSTATUS (NVENCAPI *PFN_CREATE)(NV_ENCODE_API_FUNCTION_LIST*);
PFN_CREATE create = (PFN_CREATE)GetProcAddress(h, "NvEncodeAPICreateInstance");
```

- `NvEncodeAPIGetMaxSupportedVersion`、`NvEncodeAPICreateInstance` 是 **DLL 直接导出**的函数；其余 `NvEnc*` 都通过 `NV_ENCODE_API_FUNCTION_LIST` 函数表调用。【官方文档】[PG-Intro](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/nvenc-video-encoder-api-prog-guide/)
- 函数表结构体必须先设 `version = NV_ENCODE_API_FUNCTION_LIST_VER`，再传给 `NvEncodeAPICreateInstance`。版本字段设错会得到 `NV_ENC_ERR_INVALID_VERSION`。【官方文档】头文件

### 1.3 版本兼容判断（最容易写错的一处）

- `NvEncodeAPIGetMaxSupportedVersion(uint32_t* version)` 的文档原文：返回值的**低 4 位是 minor，其余位是 major**，即 `major<<4 | minor`。【官方文档】[头文件](https://github.com/FFmpeg/nv-codec-headers/blob/master/include/ffnvcodec/nvEncodeAPI.h)
- 但 `NVENCAPI_VERSION` 宏是另一种打包：`(MAJOR | (MINOR << 24))`（13.1 即 `0x0100000D`）。**两者不能直接比较**。【官方文档】头文件第 118–126 行
- 正确写法（FFmpeg 的实际判断逻辑）：

```c
uint32_t max_ver = 0;
NvEncodeAPIGetMaxSupportedVersion(&max_ver);          // 13.1 -> 0xD1
uint32_t need = (NVENCAPI_MAJOR_VERSION << 4) | NVENCAPI_MINOR_VERSION;
if (max_ver < need) { /* 驱动太老，拒绝初始化 */ }
```

【官方文档】[FFmpeg nvenc.c](https://github.com/FFmpeg/FFmpeg/blob/master/libavcodec/nvenc.c)（这不是 NVIDIA 官方代码，但用的是 NVIDIA 头文件语义）

- `NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS::apiVersion` 要填 `NVENCAPI_VERSION` 宏，不要填 `max_ver`。【官方文档】头文件字段注释
- 向后兼容承诺：用旧 API 编译的应用能在新驱动上运行；反过来新 API 在旧驱动上会被拒绝。因此**编译用的头文件版本 = 你要求的最低驱动门槛**。【官方文档】[PG-Intro](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/nvenc-video-encoder-api-prog-guide/)

### 1.4 驱动版本门槛

| SDK | Windows 驱动最低版本 | 出处 |
|---|---|---|
| 13.1 | 610+ | 【官方文档】[Read Me 13.1](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/read-me/) |
| 13.0 | 570+ | 【官方文档】[Read Me 13.0](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.0/read-me/) |
| 12.2 | 551.76+ | 【第三方/推断】NVIDIA 论坛答复与 FFmpeg 版本门槛表；未在 12.2 文档正文直接读到 |

建议：项目的“NVIDIA 路径”以 **SDK 12.2 头文件 + 551.76 驱动**为最低线更现实（覆盖面大）；若要 H.264 10-bit、AV1 B 帧、CUDA array 输入等新特性才升 13.x。**最终以你编译时选定的头文件版本为准**。【推断】

### 1.5 Rust/FFI 注意事项

- 头文件里每个结构体都要求填对应的 `*_VER` 宏（由 `NVENCAPI_STRUCT_VERSION(ver)` 生成，内含 `0x7<<28` 标记与版本号），而且各结构体的 `ver` 数值不同。手写结构体极易错位导致 `NV_ENC_ERR_INVALID_VERSION` 或内存越界。【官方文档】头文件
- 这些结构体带大量 `reserved` 数组，**不同 SDK 版本大小不同**。正确做法：用 `bindgen` 从选定的 `nvEncodeAPI.h` 生成 `#[repr(C)]` 绑定，并把头文件版本固定进仓库；不要手抄字段。【推断】
- 若不想把 NVIDIA 头文件纳入仓库，可用 FFmpeg 维护的 `nv-codec-headers` 镜像（字段与官方一致，但其自身许可证需自行核对，本文不代为判断）。【推断】

---

## 2. 设备与编码上下文

### 2.1 用 D3D11 设备打开会话

- 传 `ID3D11Device` 的 `IUnknown*`（不是 `ID3D11DeviceContext`），`deviceType = NV_ENC_DEVICE_TYPE_DIRECTX`；整个 NVENCODE API 在 Windows 上要求 Windows 10 及以上。【官方文档】[PG-3.1.1.3](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/nvenc-video-encoder-api-prog-guide/)
- `NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS` 必须填：`version`、`deviceType`、`device`、`apiVersion = NVENCAPI_VERSION`。`reserved*` 全 0/NULL。【官方文档】头文件
- 编码会话与这个 device 绑定；同一 device 上的纹理才能被直接注册，不需要 `CopyResource`。【推断，基于注册接口要求传入 `ID3D11Texture2D*`】

### 2.2 适配器一致性（零拷贝的前提）

- 采集（DXGI Desktop Duplication / WGC）与 NVENC 必须落在**同一块 DXGI 适配器、同一个 D3D11 device** 上，才是真零拷贝。若采集纹理在 iGPU、编码在 dGPU，只能走 `ID3D11Device::OpenSharedResource` 打开共享句柄，跨适配器时再叠加一次 GPU 拷贝（还可能引入同步开销）。【推断】
- 因此初始化顺序应是：先枚举 DXGI 适配器并选定 NVIDIA 适配器（`VendorId == 0x10DE`），再用它创建 D3D11 device，最后用它打开 NVENC；不要先随便建 device 再“看看能不能编码”。【推断】

### 2.3 输入纹理的格式约束

| 采集纹理 DXGI 格式 | 注册时填的 `bufferFormat` | 备注 |
|---|---|---|
| `DXGI_FORMAT_B8G8R8A8_UNORM`（Desktop Duplication / WGC 默认 BGRA8） | `NV_ENC_BUFFER_FORMAT_ARGB` | SDK 样例 `NvEncoderD3D11` 的映射；**枚举名是 32 位字序，不是内存字节序**，不要按字面猜 |
| `DXGI_FORMAT_R8G8B8A8_UNORM` | `NV_ENC_BUFFER_FORMAT_ABGR` | 同上 |
| `DXGI_FORMAT_NV12` | `NV_ENC_BUFFER_FORMAT_NV12` | 需要自己做 RGB→NV12 |
| `DXGI_FORMAT_P010` | `NV_ENC_BUFFER_FORMAT_YUV420_10BIT`（待实测确认） | 10-bit 路径，DX11 下 P010 与 NVENC 枚举的对应关系官方未明确写 |

- 上表前两行的映射可由头文件枚举注释（`ARGB` = 32 位字中 B 在最低 8 位）推出；SDK 样例与 NVIDIA 论坛均按此使用。【官方文档】头文件 + [NVIDIA 论坛](https://forums.developer.nvidia.com/t/video-codec-buffer-formats-supported-w-d3d11/210169)
- **RGB 直入的 codec 限制**：官方能力表只在 H.264 行写 “Encoding support for ARGB content”；HEVC/AV1 的 RGB 直入没有承诺，必须用 `NvEncGetInputFormats` 逐 codec 查询，查不到就必须先转 NV12/P010。【官方文档】[AN-Capabilities](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/nvenc-application-note/)
- 对远程桌面这意味着：**H.264 4:2:0 8-bit 可以 BGRA 直入（真零拷贝）；HEVC/AV1 默认要在 GPU 上先做一次 RGB→NV12/P010 转换（`ID3D11VideoProcessorBlt` 或 compute shader），仍是 GPU 内零回读，但多 0.2–0.5 ms 量级。**【推断】
- 4:4:4：官方能力表说 HEVC 支持 4:4:4、H.264 4:4:4 只支持 CAVLC；但 D3D11 纹理路径下 10-bit 4:4:4 是否可用，NVIDIA 论坛有“不支持”的说法，未见官方确认。**列为未确认，做能力探测时用真实试编验证。**【官方文档】+【推断】

### 2.4 bind flags / usage / CPU 访问

- 编程指南的“外部输入缓冲”章节只要求：填 `NV_ENC_REGISTER_RESOURCE` → `NvEncRegisterResource` → `NvEncMapInputResource`，并把 `mappedResource` 作为 `NV_ENC_PIC_PARAMS::inputBuffer`；**没有规定纹理的 bind flags**。【官方文档】[PG-4.1.2](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/nvenc-video-encoder-api-prog-guide/)
- 实践约束（需实测）：自建纹理时用 `D3D11_USAGE_DEFAULT`、`CPUAccessFlags = 0`，`MiscFlags` 一般不带（共享纹理见下条）；WGC 帧池纹理自带 `D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE`，一般可直接注册；Desktop Duplication 返回的桌面纹理是否满足要求尚未验证，必须实测。若自行创建纹理，建议至少带 `D3D11_BIND_RENDER_TARGET`，遇到 `NV_ENC_ERR_RESOURCE_REGISTER_FAILED` 再补/换 bind flags。【推断】
- 若同时用 D3D11 VideoProcessor 做转换，device 创建时要加 `D3D11_CREATE_DEVICE_VIDEO_SUPPORT`（这是 D3D11 Video API 的要求，不是 NVENC 的要求）。【推断，依据 D3D11 Video API 文档】
- **共享纹理**（`D3D11_RESOURCE_MISC_SHARED` / `SHARED_NTHANDLE`，或通过 `OpenSharedResource` 打开的纹理）：官方没有为 shared 属性单独设限，只要它最终是**同一 `ID3D11Device` 上的 `ID3D11Texture2D`**，就走同一套 `Register/Map` 路径；若创建时带了 keyed mutex，调用 `MapInputResource` 前必须 `AcquireSync`，`Unmap` 之后才 `ReleaseSync`，否则会出现花屏或 `MAP_FAILED`。跨适配器共享（iGPU↔dGPU）还要多一次拷贝，见 §2.2。【推断，需实测】
- `pitch` 对 `NV_ENC_INPUT_RESOURCE_TYPE_DIRECTX` 必须填 0；`subResourceIndex` 指定数组纹理/多平面纹理的子资源。【官方文档】头文件字段注释

### 2.5 注册与映射的生命周期

- 注册是**按资源**的，不是按帧的：WGC 的帧池会轮换 N 张纹理，应在池创建后把每张纹理各注册一次，之后每帧只做 `Map/Unmap`。每帧重复 `Register/Unregister` 会引入明显开销。【推断】
- `mappedResource` 在 map 期间不得用于编码器之外的任何 D3D 操作（官方明确“未定义行为”）；`Unmap` 之后再归还帧池。【官方文档】[PG-4.1.2](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/nvenc-video-encoder-api-prog-guide/)
- 顺序：`Map` → `EncodePicture` →（编码完成）→ `Unmap`；不要在编码还没完成时把纹理还给采集池。【推断，与 `NvEncEncodePicture` 异步语义一致】
- 关闭会话前必须 `NvEncUnregisterResource`，否则资源被驱动持有。【官方文档】同上

### 2.6 多线程

- 一个 encode session 不是线程安全的：`NvEncEncodePicture`、`NvEncLockBitstream`、`NvEncGetSequenceParams` 应在同一线程（或由你自己串行化）。官方特别要求 `NvEncGetSequenceParams` 与 `NvEncEncodePicture` 在同一线程调用。【官方文档】[NvEncGetSequenceParams 注释](https://github.com/FFmpeg/nv-codec-headers/blob/master/include/ffnvcodec/nvEncodeAPI.h)
- 不同 session（不同线程/不同观看者）可以并行；但共享一个 D3D11 device 时，建议启用 `ID3D11Multithread::SetMultithreadProtected(TRUE)`，避免驱动内部与你的立即上下文竞争。【推断/实践】

---

## 3. 能力探测

### 3.1 能查到什么

| 接口 | 作用 |
|---|---|
| `NvEncGetEncodeGUIDCount` / `NvEncGetEncodeGUIDs` | 当前 GPU+驱动支持的 codec 列表（H.264 / HEVC / AV1 的 GUID） |
| `NvEncGetInputFormatCount` / `NvEncGetInputFormats` | 指定 codec 支持的输入格式（`NV_ENC_BUFFER_FORMAT` 列表）——**决定能不能纹理直入** |
| `NvEncGetEncodeCaps` | 单项能力查询，入参 `NV_ENC_CAPS_PARAM{capsToQuery, version}` |

以上均为官方接口。【官方文档】[PG-Capabilities](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/nvenc-video-encoder-api-prog-guide/)

### 3.2 常用 caps

| caps | 返回含义 |
|---|---|
| `NV_ENC_CAPS_SUPPORTED_RATECONTROL_MODES` | 位掩码，按 `1 << NV_ENC_PARAMS_RC_MODE` 判断 CBR/VBR/CONSTQP |
| `NV_ENC_CAPS_WIDTH_MAX` / `HEIGHT_MAX` | 最大宽/高（像素） |
| `NV_ENC_CAPS_MB_NUM_MAX` | 单帧最大宏块数 |
| `NV_ENC_CAPS_MB_PER_SEC_MAX` | 每秒最大宏块吞吐（**判断 4K60 能否跑的关键**） |
| `NV_ENC_CAPS_SUPPORT_YUV444_ENCODE` | 是否支持 4:4:4 |
| `NV_ENC_CAPS_SUPPORT_10BIT_ENCODE` | 是否支持 10-bit |
| `NV_ENC_CAPS_SUPPORT_INTRA_REFRESH` | 是否支持帧内刷新 |
| `NV_ENC_CAPS_NUM_MAX_BFRAMES` | 最大 B 帧数 |
| `NV_ENC_CAPS_ASYNC_ENCODE_SUPPORT` | 是否支持异步编码 |
| `NV_ENC_CAPS_NUM_ENCODER_ENGINES` | GPU 上有几个 NVENC 引擎（决定并行会话与 split 编码上限） |
| `NV_ENC_CAPS_SUPPORT_LOOKAHEAD` / `SUPPORT_TEMPORAL_AQ` | lookahead / temporal AQ 是否可用 |

【官方文档】头文件 `NV_ENC_CAPS` 枚举注释（[链接](https://github.com/FFmpeg/nv-codec-headers/blob/master/include/ffnvcodec/nvEncodeAPI.h)）

- 1080p60 的宏块吞吐约 `120×68×60 ≈ 490k MB/s`，远低于任何支持 NVENC 的卡；真正要查的是 4K60/4:4:4/10-bit 组合。【推断】
- caps 查询本身只证明“API 声明支持”，不证明“这一档分辨率+码率+profile 能初始化成功”。官方也建议真实试编验证。【官方文档】[AN-Capabilities](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/nvenc-application-note/)（能力表脚注也提示实现/驱动差异）

### 3.3 代际能力（官方能力表摘要）

| 能力 | 情况（13.1 能力表覆盖 Ampere/TU、Ada、Blackwell、Jetson Thor） |
|---|---|
| H.264 8-bit / 10-bit | 8-bit：上述代际均支持；10-bit：**仅 Blackwell** |
| H.264 4:4:4 | 支持，但**仅 CAVLC**（压缩效率差） |
| H.264 ARGB 输入 | 上述代际均支持（这是唯一被官方明确承诺 RGB 直入的 codec） |
| HEVC 8-bit / 10-bit | Main/Main10 均支持（Ampere/TU 及以后） |
| HEVC 4:4:4 | 支持；4:2:2 仅 Blackwell；10-bit 4:4:4 是否可用需实测 |
| AV1 | 仅 Ada（RTX 40）及以后；Main 8/10-bit 均支持 |
| 最大分辨率 | HEVC/AV1 8192×8192；H.264 受 level 限制（确切值用 `WIDTH_MAX/HEIGHT_MAX` 查，能力表未给） |

更老的代际（Maxwell/Pascal/Turing 早期产品）不在 13.1 能力表中，需查 [GPU 支持矩阵](https://developer.nvidia.com/video-encode-and-decode-gpu-support-matrix-new)。【官方文档】

【官方文档】[AN-Capabilities](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/nvenc-application-note/)、[GPU 支持矩阵](https://developer.nvidia.com/video-encode-and-decode-gpu-support-matrix-new)

- 注意：**不是所有 N 卡都有 NVENC**。例如 MX150（GP108）有 NVDEC 但没有 NVENC；选卡/报错时先查支持矩阵。【官方文档】[GPU 支持矩阵](https://developer.nvidia.com/video-encode-and-decode-gpu-support-matrix-new)、[论坛案例](https://forums.developer.nvidia.com/t/mx150-seems-not-to-support-nvdec-nvenc-in-windows-10/77628)
- 1080p 4:2:0 8-bit 的编码吞吐：官方性能表给 P1 低延迟档约 855–977 fps（桌面 GPU 不同型号），折算单帧约 1.0–1.2 ms，对 16.7 ms 帧周期有充足余量。【官方文档】[AN-Performance](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/nvenc-application-note/)、【推断】折算

### 3.4 判定“这块卡能不能用”的最小流程

1. `LoadLibrary(nvEncodeAPI64.dll)` 成功；
2. `GetMaxSupportedVersion` ≥ 你编译头文件的 `(major<<4|minor)`；
3. `NvEncOpenEncodeSessionEx`（D3D11 device）成功；
4. `GetEncodeGUIDCount > 0`，且列表里有你要的 codec GUID；
5. `GetInputFormats(codec)` 里包含你打算直入的格式（BGRA→ARGB / NV12 / P010）；
6. 目标档位真实试编 1–2 秒（初始化成功 + 出包 + 码率落点）。

任何一步失败都视为“该机不可用该档位”，不要只看驱动版本号。【推断】

---

## 4. 低延迟参数

### 4.1 推荐基线（1080p60 远程桌面）

```c
// NV_ENC_INITIALIZE_PARAMS
init.version            = NV_ENC_INITIALIZE_PARAMS_VER;
init.encodeGUID         = NV_ENC_CODEC_H264_GUID;     // 或 HEVC_GUID
init.presetGUID         = NV_ENC_PRESET_P1_GUID;      // P1 最快
init.tuningInfo         = NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY;
init.encodeWidth        = 1920;
init.encodeHeight       = 1080;
init.frameRateNum       = 60;
init.frameRateDen       = 1;
init.enablePTD          = 1;    // 由编码器决定帧类型
init.enableEncodeAsync  = 0;    // 第一版用同步，便于算延迟
init.encodeConfig       = &cfg;

// NV_ENC_CONFIG
cfg.version             = NV_ENC_CONFIG_VER;
cfg.gopLength           = 120;        // 2s @60fps；按需关键帧再 FORCEIDR
cfg.frameIntervalP      = 1;          // IPP，无 B 帧
cfg.rcParams.rateControlMode = NV_ENC_PARAMS_RC_CBR;
cfg.rcParams.averageBitRate  = target_bps;
cfg.rcParams.vbvBufferSize   = target_bps / 60 * 2;   // 约 2 帧的比特数
cfg.rcParams.vbvInitialDelay = cfg.rcParams.vbvBufferSize;
cfg.rcParams.multiPass       = NV_ENC_MULTI_PASS_DISABLED;
cfg.rcParams.zeroReorderDelay= 1;
cfg.rcParams.enableLookahead = 0;
cfg.rcParams.enableTemporalAQ= 0;
cfg.rcParams.enableAQ        = 1;    // 空间 AQ，改善同码率主观质量
cfg.rcParams.aqStrength      = 8;
cfg.rcParams.lowDelayKeyFrameScale = 1;
cfg.rcParams.strictGOPTarget = 1;
cfg.encodeCodecConfig.h264Config.repeatSPSPPS = 1;
cfg.encodeCodecConfig.h264Config.idrPeriod    = 120;
cfg.encodeCodecConfig.h264Config.outputBufferingPeriodSEI = 0;
cfg.encodeCodecConfig.h264Config.outputPictureTimingSEI   = 0;
```

- 上表字段名与语义均来自 SDK 头文件；推荐值中的 VBV/AQ 强度属于工程取值，需按实测调整。【官方文档】字段语义 +【推断】取值

### 4.2 延迟相关字段逐个说明

| 字段（结构体） | 作用 | 副作用 / 注意 |
|---|---|---|
| `tuningInfo = NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY`（INITIALIZE_PARAMS） | 让 preset 按“超低延迟”调参：倾向于无 B 帧、无 lookahead、更激进的码控 | 压缩效率下降，同码率画质不如 HIGH_QUALITY；`PRESET` 仍建议 P1~P3 |
| `presetGUID`（INITIALIZE_PARAMS） | P1 最快 … P7 最慢/最高质量 | 与 `tuningInfo` 组合生效；按官方性能表，P1→P7 单帧编码耗时递增，30 ms 预算优先 P1~P3。B 帧的有无由 tuningInfo 决定（ULL/LL 下默认关闭） |
| `enablePTD = 1` | 由编码器决定 I/P/B 与场景切换插入 | 置 0 则每帧必须自己填 `NV_ENC_PIC_PARAMS::pictureType`，且 `FORCEIDR` 无效 |
| `enableEncodeAsync` | 1 = 异步 + 事件；0 = 同步 | 同步模式便于测端到端；异步要额外管理“每帧一个输出 buffer + event” |
| `frameIntervalP = 1`（NV_ENC_CONFIG） | GOP 模式：0=I、1=IPP、2=IBP、3=IBBP | ≥2 会引入重排延迟，30 ms 预算基本不能碰 |
| `zeroReorderDelay = 1`（RC_PARAMS） | 声明零重排：`num_reorder_frames=0` | 只在有 B 帧时才有实际意义；与 `frameIntervalP=1` 一起用是保险 |
| `enableLookahead = 0` + `lookaheadDepth = 0`（RC_PARAMS） | 关前视 | 开着会多缓存 N 帧，直接吃掉延迟预算；官方要求 lookahead 期间输入帧必须一直可用 |
| `multiPass = DISABLED`（RC_PARAMS） | 单遍编码 | 两遍编码要等第一遍统计，延迟翻倍、GPU 占用更高 |
| `gopLength`（NV_ENC_CONFIG） | GOP 长度（帧） | 越小 IDR 越频繁 → 峰值码率与卡顿越多；远程桌面 2s 起步，掉线/新加入再 `FORCEIDR` |
| `idrPeriod`（H.264/HEVC config） | IDR 间隔；0 时等于 `gopLength` | 低延迟场景可设 `NVENC_INFINITE_GOPLENGTH`，完全靠应用按需强制 IDR |
| `enableIntraRefresh` + `intraRefreshPeriod` + `intraRefreshCnt` | 把 I 帧摊成每帧少量 intra 块，避免 IDR 大包尖峰 | **要求 `gopLength = NVENC_INFINITE_GOPLENGTH`**，否则被驱动禁用；画质略降、部分解码器/播放器兼容性差；抗丢包恢复不如 IDR 干脆 |
| `repeatSPSPPS`（H.264/HEVC config） | 每个 IDR 前重复 SPS/PPS | 13.1 指南称默认就随每个 IDR/关键帧输出；显式置 1 只是兼容旧驱动；每帧多几十字节 |
| `disableSPSPPS` | 完全不写 SPS/PPS（走带外） | 需要你通过 `NvEncGetSequenceParams` 单独传输；WebRTC 场景常用，但增加握手复杂度 |
| `lowDelayKeyFrameScale`（RC_PARAMS） | I 帧比特相对 P 帧的倍数 | 默认 LL=2、ULL=1；调大 → IDR 更清晰但更尖峰；调小 → 尖峰小、IDR 糊 |
| `strictGOPTarget`（RC_PARAMS） | 抑制 GOP 间码率波动 | 让 VBV 更平稳，代价是复杂场景画质略降 |
| `sliceMode` / `sliceModeData`（codec config） | 每帧切片数（H.264 支持固定 MB 数或固定片数） | 切片多 → 抗丢包/可并行解码，但码率略升；若要 `sliceOffsets`，必须 `enableEncodeAsync=0` |
| `reportSliceOffsets`（INITIALIZE_PARAMS） | 在 `NV_ENC_LOCK_BITSTREAM::sliceOffsets` 返回每片偏移 | **要求同步模式**；异步下不可用 |
| `enableSubFrameWrite`（INITIALIZE_PARAMS） | 一片/一 tile 编完就写内存，进一步降延迟 | 需要持续轮询 `NvEncLockBitstream`，CPU 占用上升；第一版不建议 |
| `outputBufferingPeriodSEI` / `outputPictureTimingSEI` | HRD 相关 SEI | 对实时推流无意义，白增字节；置 0 |
| `enableNonRefP` | 自动插入非参考 P 帧（抗丢包） | 掉线恢复有用，但会降低压缩效率；远程桌面可选 |
| `enableUniDirectionalB`（13.x） | 单向 B 帧，改善 LL/ULL 压缩效率 | 仍会引入 B 帧语义；30 ms 目标下不建议开 |
| `splitEncodeMode`（13.x，HEVC/AV1） | 把一帧拆到多个 NVENC 引擎并行编 | H.264 不支持；需 GPU 有 ≥2 引擎；降编码延迟但配置约束多（见头文件 2273–2278 行） |

【官方文档】以上全部字段语义来自 `nvEncodeAPI.h` 注释（[链接](https://github.com/FFmpeg/nv-codec-headers/blob/master/include/ffnvcodec/nvEncodeAPI.h)）；“建议/副作用”部分含工程判断，已分别注明推断。

### 4.3 几个容易混淆的点

- 字段名不是 `repeatSpsPps` 而是 `repeatSPSPPS`（H.264/HEVC）；AV1 对应字段是 `repeatSeqHdr`。写错编译不过，FFI 里更会静默错位。【官方文档】头文件
- `gopLength` 在 `NV_ENC_CONFIG`，`idrPeriod` 在 codec config；两者不是一回事。【官方文档】
- `zeroReorderDelay` 在 `NV_ENC_RC_PARAMS`（不是 `NV_ENC_CONFIG` 顶层）。【官方文档】
- ULL tuning 只影响 preset 的默认组合，**不会自动帮你关掉你显式打开的 lookahead/multipass**；最终以你填的 `encodeConfig` 为准。【官方文档】`encodeConfig` 覆盖 preset 的说明（INITIALIZE_PARAMS 2290 行）

---

## 5. 码率控制

### 5.1 三种模式

| 模式 | 行为 | 延迟影响 | 适用 |
|---|---|---|---|
| `NV_ENC_PARAMS_RC_CONSTQP` | 固定 QP，不看码率 | RC 延迟最低 | 固定画质/LAN；公网会把链路打爆 |
| `NV_ENC_PARAMS_RC_VBR` | 追平均码率，允许峰值到 `maxBitRate` | 帧大小波动大，p99 网络延迟差 | 有缓冲的播放/录制 |
| `NV_ENC_PARAMS_RC_CBR` | 追恒定码率，码率上限由 `averageBitRate` 决定 | 最可控 | **远程桌面应该用这个** |

【官方文档】`NV_ENC_PARAMS_RC_MODE` 枚举与 `NV_ENC_RC_PARAMS` 注释

### 5.2 VBV 与码率的关系

- `vbvBufferSize` 单位是 **bits**，不是 bytes；`0` 表示用驱动默认值（通常远大于低延迟所需）。【官方文档】头文件
- 远程桌面的经验取值：`vbvBufferSize ≈ target_bps / fps * (1~2)`，`vbvInitialDelay = vbvBufferSize`。含义是“最多允许 1~2 帧的码率余量”，这样单帧最坏出包大小被限制在约 2 帧预算内。【推断】
- VBV 太小会让复杂帧（滚动、视频）QP 飙升、糊；VBV 太大则失去对 p99 帧大小的约束。用 1 帧和 2 帧两档实测对比。【推断】

### 5.3 multipass 与 AQ

- `multiPass`：对实时路径必须 `NV_ENC_MULTI_PASS_DISABLED`。两遍统计天然多一帧以上延迟。【官方文档】字段语义 +【推断】延迟
- 空间 AQ（`enableAQ=1`，`aqStrength 1–15`）：同码率下改善平坦/暗部质量，几乎不增加延迟，建议开，强度 6–10。【推断】
- 时间 AQ（`enableTemporalAQ`）：跨帧分配比特，可能改善运动场景，但计算更重、行为不如空间 AQ 可预测；远程桌面先关。【推断】
- `targetQuality` 只在 VBR（CQ 型）下有意义，CBR 不要设。【官方文档】字段注释

### 5.4 “设了 CBR 但码率不生效”的排查顺序

1. **只设了 `maxBitRate`**：CBR 下 `maxBitRate` 被忽略，必须设 `averageBitRate`。这是最常见原因。【官方文档】`maxBitRate` 注释原文 “used for VBR and ignored for CBR mode”
2. **`vbvBufferSize = 0`**：驱动给默认大 VBV，短窗口平均可能接近目标，但单帧峰值不受控，主观上就是“码率没控住”。【推断】
3. **QP 上下限打架**：`enableMaxQP` 把 maxQP 压太低 → 花不出目标比特；`enableMinQP` 太高 → 静态画面也超发。检查 `minQP/maxQP/initialRCQP` 与 `enableMinQP/enableMaxQP/enableInitialRCQP` 是否成对设置。【官方文档】
4. **没有 filler data**：CBR 只是“不超过”，静态桌面会显著低于目标；要严格恒速需 `enableFillerDataInsertion=1`（会浪费带宽，不建议远程桌面用）。【官方文档】字段语义 +【推断】取舍
5. **重配时 `encodeConfig = NULL` 或换了 preset**：reconfigure 会退回该 preset 的默认配置，你原来的 RC 参数被覆盖；动态改码率时必须传完整 `encodeConfig`。【官方文档】`NV_ENC_RECONFIGURE_PARAMS` 注释（2324–2337 行）
6. **单位错误**：码率是 bits/sec，VBV 是 bits；项目里出现过“按 bytes 填”的情况需要排查。【推断】
7. **实际码率统计窗口太短**：IDR 帧会让 1 秒窗口瞬时超标；看 3–5 秒滑动平均 + 最大单帧字节数，而不是单帧值。【推断】

### 5.5 动态改码率（拥塞控制联动）

- 用 `NvEncReconfigureEncoder`：`reInitEncodeParams` 填与初始化相同的字段，`encodeConfig` 指向**完整配置**（仅改 `rcParams.averageBitRate/vbvBufferSize`），`resetEncoder=0`、`forceIDR=0`。这样不重置 RC 状态、不插 IDR，下一帧起生效。【官方文档】`NV_ENC_RECONFIGURE_PARAMS` 字段注释
- `resetEncoder=1` 会重置码控状态；在 `enablePTD=1` 时会强制当前帧为 IDR —— 这会制造一个大包，码率切换时不要随手用。【官方文档】
- 单步调整幅度建议 ≤ ±20%，给 RC 几个帧收敛；否则 VBV 会先欠/超再回摆。【推断】
- 按需关键帧（新观看者加入、NACK 恢复）用 `NV_ENC_PIC_PARAMS::encodePicFlags |= NV_ENC_PIC_FLAG_FORCEIDR`（要求 `enablePTD=1`），或 `NV_ENC_PIC_FLAG_FORCEINTRA`。【官方文档】`NV_ENC_PIC_FLAGS`

---

## 6. 输出、码流与时间戳

### 6.1 输出缓冲与取包

- 输出缓冲由 `NvEncCreateBitstreamBuffer` 分配（`size`/`memoryHeap` 字段已废弃，不要用），拿到 `NV_ENC_OUTPUT_PTR`；每帧 `NV_ENC_PIC_PARAMS::outputBitstream` 指向其中一个。【官方文档】头文件
- 取包：`NvEncLockBitstream(&lock)`，`lock.outputBitstream` 指向该 buffer；成功后读 `lock.bitstreamBufferPtr` / `bitstreamSizeInBytes`；处理完 `NvEncUnlockBitstream`。【官方文档】`NV_ENC_LOCK_BITSTREAM` 注释
- `lock.bitstreamBufferPtr` 是**主机可读指针**，说明输出必然经过一次 GPU→CPU 传输；所谓“零拷贝”只对输入端成立，输出侧无法避免回读。【官方文档】字段语义 +【推断】
- 同步模式下 `LockBitstream` 会阻塞到该帧编码完成；异步模式用 `doNotWait=1` 轮询会得到 `NV_ENC_ERR_LOCK_BUSY`（非致命，隔几毫秒重试）。【官方文档】错误码注释
- 建议准备 3–4 个输出 buffer 轮转；同一 buffer 在未 `Unlock` 前不能再次提交。【推断】

### 6.2 SPS/PPS

- 13.1 编程指南明确：**默认 SPS/PPS 会附在每个 IDR/关键帧的码流里**；因此不要假设“只有第一个 IDR 带参数集”。【官方文档】[PG-Bitstream](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/nvenc-video-encoder-api-prog-guide/)
- 旧字段 `repeatSPSPPS` 仍存在，显式置 1 对老驱动更保险；若要做带外参数集，用 `disableSPSPPS=1` + `NvEncGetSequenceParams` 单独取（最大 `NV_MAX_SEQ_HDR_LEN = 512` 字节）。【官方文档】头文件
- `NV_ENC_PIC_FLAG_OUTPUT_SPSPPS` 可以强制当前帧再写一次参数集，适合“客户端中途加入/重连”。【官方文档】`NV_ENC_PIC_FLAGS`
- `NvEncGetSequenceParams` 有线程约束：必须与 `NvEncEncodePicture` 在同一线程调用。【官方文档】函数注释
- 做 RTP/WebRTC 时建议每个 IDR 都带 SPS/PPS，省去单独的参数集通道；代价是每个 GOP 多几十字节。【推断】

### 6.3 Annex-B 与 AVCC

- NVENC 输出的是**带起始码（00 00 00 01 / 00 00 01）的 Annex-B 字节流**，一个 `NV_ENC_OUTPUT_PTR` 对应一帧（内含 SPS/PPS/SEI/slice NAL）。官方文档只说明“SPS/PPS 写在码流中”，未使用 “Annex-B” 一词；该结论由样本解析方式与 FFmpeg 的 `h264_mp4toannexb`/`hevc_mp4toannexb` 对应关系推断。【推断】
- 若容器需要 AVCC（长度前缀，mp4/flv），必须自己做 Annex-B→AVCC 转换（拆 NAL、写 4 字节长度、参数集单独存放）。NVENC 不提供 AVCC 输出。【推断】
- 远程桌面的传输层（自定义 RTP/FEC）建议直接按 Annex-B 的 NAL 边界切包，避免再做一层转换；注意 `sliceOffsets` 只在同步模式可用。【官方文档】+【推断】

### 6.4 时间戳对应

- `NV_ENC_PIC_PARAMS::inputTimeStamp` 是**不透明数据**，不会写进码流，但会原样出现在 `NV_ENC_LOCK_BITSTREAM::outputTimeStamp`；用它把采集时间戳带到发送侧。【官方文档】两处字段注释
- `frameIdx` 也会回传；开启 lookahead/UHQ/重复编码特性时要求它从 0 单调递增。【官方文档】`frameIdx` 注释
- 有 B 帧时输出是解码顺序，`outputTimeStamp` 仍对应正确的显示帧；远程桌面不用 B 帧，输出顺序 = 输入顺序。【官方文档】+【推断】
- 单位由应用自定（建议用 QPC 100 ns 或微秒），NVENC 不做换算。【官方文档】“opaque data”

### 6.5 首帧出包与“零排队”

- `frameIntervalP=1` + `zeroReorderDelay=1` + `enableLookahead=0` 时，编码器没有重排/前视缓冲：每提交一帧就对应一个输出包，**不存在“攒 N 帧才出第一个包”**。【官方文档】`zeroReorderDelay` 注释（`num_reorder_frames=0`）+【推断】
- 但 `NvEncEncodePicture` 只表示“已提交”，实际出包要等硬件编完；同步模式下这个等待体现在 `NvEncLockBitstream` 的阻塞时间里（1080p 约 1 ms 量级）。【官方文档】接口语义 +【推断】
- 若开 B 帧，前 `frameIntervalP-1` 次调用会返回 `NV_ENC_ERR_NEED_MORE_INPUT`（非致命），且同步模式下此时不能去 Lock 输出 buffer；必须继续喂帧直到 `NV_ENC_SUCCESS`。【官方文档】错误码注释
- 首帧本身是 IDR（最大包），p99 延迟测量里要单独标记 IDR 帧，不能和 P 帧混在一起算。【推断】

### 6.6 异步模式（可选）

- `enableEncodeAsync=1` 时，每个输出 buffer 配一个 `NV_ENC_EVENT_PARAMS` 注册的 event；`NV_ENC_PIC_PARAMS::completionEvent` 指定本次用哪个 event，编码完成后 event 被置位。【官方文档】异步编码章节
- 优点：提交线程不被 `LockBitstream` 阻塞；缺点：要管理 in-flight 帧数、event 复用、以及 `NV_ENC_ERR_NEED_MORE_INPUT`/`ENCODER_BUSY` 的重试。第一版建议同步，先拿到可信的延迟基线再换异步。【推断】

---

## 7. 许可与分发

- SDK 采用 NVIDIA DesignWorks SDK License。授予的权利是：安装/使用 SDK、修改 SDK 样例源码、**把样例源码以目标码形式集成进你的应用**并分发。【官方文档】[License](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/license/)
- 分发要求：应用必须有“实质性的额外功能”（不能只是把 SDK 样例打包卖）；SDK 的可分发部分只能由你的应用访问；需保留版权/许可声明。【官方文档】同上
- 禁止：逆向、反编译、移除版权声明；除许可明确授予外，不得复制/出售/再许可/分发/修改 SDK；**不得把 SDK 作为独立产品分发**。【官方文档】同上
- 关于运行库：`nvEncodeAPI64.dll` 是显示驱动组件，不是可自由分发的 SDK 文件。**不要把它打进安装包**；依赖用户已安装的 NVIDIA 驱动。若用户没装驱动，动态加载失败即可回退。【官方文档】[PG-Intro](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/nvenc-video-encoder-api-prog-guide/)（“installed as part of the NVIDIA display driver”）+【推断】分发边界
- 关于头文件：许可证对“SDK 任何部分的复制/分发”有限制。把 `nvEncodeAPI.h` 放进公开仓库是否属于被允许的“集成”，许可文本没有明确写；保守做法是用 `nv-codec-headers` 或仅在自己的仓库内保存并用 bindgen 生成绑定，必要时走法务确认。【推断】
- **专利**：许可原文明确 “NVIDIA does not grant to you under this license any necessary patent or other rights with respect to any audio and/or video encoders and decoders”，并列举 MPEG-LA、Sisvel、Fraunhofer 等。H.264/HEVC/AV1 的专利许可责任在你自己。【官方文档】[License](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/license/)
- 结论：闭源商业软件可以用 NVENC，但 ①不分发 DLL；②不做成“纯转码器”式产品（需有实质额外功能）；③自行解决编解码专利。【官方文档】+【推断】

---

## 8. 已知坑

### 8.1 并发会话上限

- 非 qualified（消费级 GeForce）卡有**全系统**会话数上限：SDK 13.1 为 **12**，SDK 12.2 为 **8**（需 Windows 551.76+/Linux 550.54.14+），更早版本为 **5**；上限是所有非 qualified 卡合计，不是单卡。【官方文档】[AN-Licensing](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/nvenc-application-note/)、[论坛答复](https://forums.developer.nvidia.com/t/how-to-increase-number-of-nvenc-concurrent-sessions/169367)
- 其他应用（OBS、浏览器硬件加速、会议软件）也会占用会话。屏幕墙/多观看者必须独立 session，很容易撞上限；架构上要准备“JPEG 快照/降级”路径。【推断】
- 会话数不等于引擎数：一个 session 只用一个 NVENC 引擎（除非开 split encoding）；4090 有 2 个引擎但并发上限是 8/12 个 session。【官方文档】`NV_ENC_CAPS_NUM_ENCODER_ENGINES` + [论坛案例](https://forums.developer.nvidia.com/t/fluctuating-performance-with-two-instances-of-nvenc/247778)

### 8.2 常见错误码

| 错误 | 含义 | 常见原因 |
|---|---|---|
| `NV_ENC_ERR_INVALID_VERSION` | 结构体版本不对 | `*_VER` 宏没填/填错；FFI 结构体大小与头文件不一致 |
| `NV_ENC_ERR_INVALID_PARAM` | 参数非法 | 宽高为 0、码率 0、`pitch` 非 0（DX 资源）、preset/encodeGUID 不支持 |
| `NV_ENC_ERR_INVALID_DEVICE` / `DEVICE_NOT_EXIST` | device 无效/已失效 | device 创建失败、TDR、驱动更新后 device 丢失；需重建会话 |
| `NV_ENC_ERR_UNSUPPORTED_PARAM` | 参数不被硬件支持 | 4:4:4/10-bit/AV1 在该卡不支持；RGB 直入用在 HEVC/AV1 上 |
| `NV_ENC_ERR_RESOURCE_REGISTER_FAILED` | 纹理注册失败 | 格式不匹配、跨适配器、bind flags/usage 不合要求、device 不一致 |
| `NV_ENC_ERR_MAP_FAILED` | 映射失败 | 资源未注册、已 map 未 unmap、资源已被释放 |
| `NV_ENC_ERR_NEED_MORE_INPUT` | 需要更多输入才能出包 | 开了 B 帧/lookahead；非致命，继续喂帧；EOS 时传 NULL 冲刷 |
| `NV_ENC_ERR_ENCODER_BUSY` | 硬件忙 | 提交过快；隔几 ms 重试或做背压 |
| `NV_ENC_ERR_LOCK_BUSY` | 非阻塞 lock 还没编完 | `doNotWait=1` 的正常返回；隔几 ms 重试 |
| `NV_ENC_ERR_NOT_ENOUGH_BUFFER` | 输出缓冲不足 | 码率/分辨率变大后未重建输出 buffer |
| `NV_ENC_ERR_OUT_OF_MEMORY` | 内存不足 | buffer 数过多、分辨率过大 |
| `NV_ENC_ERR_INCOMPATIBLE_CLIENT_KEY` | 特性与 license 类型不匹配 | 某些专业特性在消费卡上不可用 |
| `NV_ENC_ERR_GENERIC` | 未分类内部错误 | 用 `NvEncGetLastErrorString` 取详情 |

【官方文档】`NVENCSTATUS` 枚举注释（[链接](https://github.com/FFmpeg/nv-codec-headers/blob/master/include/ffnvcodec/nvEncodeAPI.h)）

### 8.3 驱动与头文件

- 头文件版本高于驱动 → 初始化阶段就可能失败；升级驱动后进程内的 DLL 不会自动重载，**必须重启进程**。【推断】
- 驱动更新/GPU 掉卡（TDR）会使 device 失效，必须销毁会话重建；不能只重试 `EncodePicture`。【官方文档】`NV_ENC_ERR_DEVICE_NOT_EXIST` 注释
- NVIDIA 新驱动可能改变默认 VBV/preset 行为；低延迟配置不要依赖“preset 默认值”，关键字段全部显式设置。【推断】

### 8.4 Optimus / 多 GPU 笔记本

- 混合显卡机器上，DXGI 枚举顺序通常是 iGPU 在前。若用默认适配器建 device，NVENC 会话要么打不开，要么根本不在 NVIDIA GPU 上；必须按 `VendorId 0x10DE` 选适配器并在其上建 device + 采集。【推断】
- 若桌面合成/采集在 iGPU，而编码在 dGPU，纹理跨适配器：需要 shared handle + `OpenSharedResource`，跨适配器还要 keyed mutex/拷贝，延迟与带宽都会变差。【推断】
- `NvOptimusEnablement` 导出只影响 OpenGL 选择，不解决 D3D11 的适配器选择；D3D 必须自己枚举。【推断】（Optimus 指南的结论是“图形上下文要建在 NVIDIA GPU 上”，[官方文档](https://docs.nvidia.com/cuda/archive/11.0_GA/pdf/Optimus_Developer_Guide.pdf)）
- Windows 10 的“图形性能首选项”可以让系统把进程拉到 dGPU，但不可编程控制；不要依赖它。【推断】

### 8.5 其它坑

- **枚举名误导**：`NV_ENC_BUFFER_FORMAT_ARGB` 对应内存里的 BGRA8（`DXGI_FORMAT_B8G8R8A8_UNORM`），不是按名字的 A-R-G-B 字节序。搞反会出现红蓝互换。【官方文档】头文件 + SDK 样例
- **RGB 直入只保证 H.264**：切 HEVC 后仍传 ARGB 会得到 `NV_ENC_ERR_UNSUPPORTED_PARAM` 或画质异常；必须查 `NvEncGetInputFormats`。【官方文档】+【推断】
- **H.264 4:4:4 只有 CAVLC**：想靠 4:4:4 提升文字清晰度时，码率会比 CABAC 高不少；HEVC 4:4:4 更合适。【官方文档】[AN-Capabilities](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/nvenc-application-note/)
- **AV1 只在 Ada+**：RTX 30 及以前没有 AV1 编码；能力探测时不能只看 SDK 版本。【官方文档】[GPU 支持矩阵](https://developer.nvidia.com/video-encode-and-decode-gpu-support-matrix-new)
- **H.264 10-bit 只在 Blackwell**：老卡上把 profile 设成 High10 会失败。【官方文档】[AN-Capabilities](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/nvenc-application-note/)
- **`enableIntraRefresh` 与 `gopLength` 互斥**：不是 INFINITE 时帧内刷新会被静默禁用（不报错）。【官方文档】头文件 1882–1884 行
- **输出回读是固定成本**：`LockBitstream` 必然把码流拷到主机内存；不要把它算进“零拷贝”里，也不要在大码率下忽略这次 copy 的耗时。【推断】
- **`NvEncGetSequenceParams` 线程约束**：必须与 `NvEncEncodePicture` 同线程，跨线程调用可能失败或行为未定义。【官方文档】
- **`NvEncGetEncodePresetConfig`（非 Ex）在驱动 590+ 上返回 `NV_ENC_ERR_UNSUPPORTED_PARAM`**：要用带 `tuningInfo` 参数的 `NvEncGetEncodePresetConfigEx`。【官方文档】头文件注释
- **preset 结构体版本**：`NV_ENC_PRESET_CONFIG` 要填 `NV_ENC_PRESET_CONFIG_VER`，其内嵌的 `config` 还要填 `NV_ENC_CONFIG_VER`；漏填会得到 `NV_ENC_ERR_INVALID_VERSION`。【官方文档】头文件
- **D3D11 debug layer**：带调试层的 device 可能让 NVENC 注册/编码变慢甚至失败；发布路径不要开 debug layer。【推断】

---

## 9. 最小可用实现路径

### 9.1 调用顺序（从枚举设备到第一包码流）

1. `LoadLibraryW(L"nvEncodeAPI64.dll")`，取 `NvEncodeAPICreateInstance`。
2. `NvEncodeAPIGetMaxSupportedVersion`，与 `(MAJOR<<4|MINOR)` 比较；不满足则放弃 NVENC。
3. 填 `NV_ENCODE_API_FUNCTION_LIST{version=NV_ENCODE_API_FUNCTION_LIST_VER}`，调用 `NvEncodeAPICreateInstance`。
4. `CreateDXGIFactory1` → `EnumAdapters1`，挑 `DXGI_ADAPTER_DESC1.VendorId == 0x10DE` 且与采集输出一致的适配器。
5. `D3D11CreateDevice(adapter, D3D_DRIVER_TYPE_UNKNOWN, ..., D3D11_CREATE_DEVICE_BGRA_SUPPORT [| VIDEO_SUPPORT])` 得到 `ID3D11Device`。
6. `fl.nvEncOpenEncodeSessionEx({device=ID3D11Device*, deviceType=NV_ENC_DEVICE_TYPE_DIRECTX, apiVersion=NVENCAPI_VERSION})` → `encoder`。
7. `fl.nvEncGetEncodeGUIDCount/GetEncodeGUIDs` → 选 `NV_ENC_CODEC_H264_GUID`（或 HEVC/AV1）。
8. `fl.nvEncGetInputFormatCount/GetInputFormats` → 确认 `NV_ENC_BUFFER_FORMAT_ARGB`（BGRA 直入）在列表里。
9. `fl.nvEncGetEncodeCaps` → 查 `WIDTH_MAX/HEIGHT_MAX/MB_PER_SEC_MAX/SUPPORTED_RATECONTROL_MODES/SUPPORT_INTRA_REFRESH/NUM_ENCODER_ENGINES`。
10. `fl.nvEncGetEncodePresetConfigEx(encoder, codecGUID, presetGUID, tuningInfo, &presetCfg)` 取默认配置，再按 §4.1 覆盖低延迟字段（比从零手填安全）。
11. 填 `NV_ENC_INITIALIZE_PARAMS`（含 `encodeConfig` 指针），`fl.nvEncInitializeEncoder`。
12. `fl.nvEncCreateBitstreamBuffer` × 3~4 个输出 buffer。
13. 对采集池里的每张纹理：`fl.nvEncRegisterResource`（`resourceType=NV_ENC_INPUT_RESOURCE_TYPE_DIRECTX`、`resourceToRegister=ID3D11Texture2D*`、`bufferFormat=NV_ENC_BUFFER_FORMAT_ARGB`、`bufferUsage=NV_ENC_INPUT_IMAGE`、`pitch=0`、`subResourceIndex=0`）。
14. 首帧：`MapInputResource` → 填 `NV_ENC_PIC_PARAMS`（`inputWidth/Height`、`inputPitch=inputWidth`、`inputBuffer=mappedResource`、`bufferFmt=mappedBufferFmt`、`inputTimeStamp=采集时间`、`outputBitstream=buf[0]`）→ `nvEncEncodePicture`。
15. 同步模式下立即 `nvEncLockBitstream`（阻塞到出包）→ 读 `bitstreamBufferPtr/bitstreamSizeInBytes/outputTimeStamp` → 交给传输层 → `nvEncUnlockBitstream` → `nvEncUnmapInputResource`。

以上接口清单与参数要求来自官方指南与头文件。【官方文档】[PG-BasicFlow](https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/nvenc-video-encoder-api-prog-guide/)、[头文件](https://github.com/FFmpeg/nv-codec-headers/blob/master/include/ffnvcodec/nvEncodeAPI.h)

### 9.2 每帧路径

```text
采集纹理 (BGRA8, 同一 device)
  → NvEncMapInputResource(registered)
  → 填 NV_ENC_PIC_PARAMS（bufferFmt = mappedBufferFmt，inputPitch = width）
  → NvEncEncodePicture（同步；必要时 encodePicFlags |= FORCEIDR）
  → NvEncLockBitstream（等到出包）
  → memcpy/parse Annex-B
  → NvEncUnlockBitstream
  → NvEncUnmapInputResource
  → 纹理归还采集池
```

- 想再降延迟：开启 `enableSubFrameWrite` 并按片轮询；想省线程：改异步 + event。二者都要先把同步基线测出来。【推断】

### 9.3 关闭顺序

`NvEncDestroyBitstreamBuffer`（每个）→ `NvEncUnregisterResource`（每张纹理）→ `NvEncDestroyEncoder` → 释放 D3D11 device/context → `FreeLibrary`。【官方文档】接口注释

### 9.4 必须实现的 FFI 面

- 导出函数：`NvEncodeAPICreateInstance`、`NvEncodeAPIGetMaxSupportedVersion`。
- 函数表方法：`nvEncOpenEncodeSessionEx`、`nvEncGetEncodeGUIDCount`、`nvEncGetEncodeGUIDs`、`nvEncGetInputFormatCount`、`nvEncGetInputFormats`、`nvEncGetEncodeCaps`、`nvEncGetEncodePresetConfigEx`、`nvEncInitializeEncoder`、`nvEncCreateBitstreamBuffer`、`nvEncDestroyBitstreamBuffer`、`nvEncRegisterResource`、`nvEncUnregisterResource`、`nvEncMapInputResource`、`nvEncUnmapInputResource`、`nvEncEncodePicture`、`nvEncLockBitstream`、`nvEncUnlockBitstream`、`nvEncReconfigureEncoder`、`nvEncGetSequenceParams`、`nvEncGetLastErrorString`、`nvEncDestroyEncoder`。
- 结构体：`NV_ENCODE_API_FUNCTION_LIST`、`NV_ENC_OPEN_ENCODE_SESSION_EX_PARAMS`、`NV_ENC_CAPS_PARAM`、`NV_ENC_PRESET_CONFIG`、`NV_ENC_INITIALIZE_PARAMS`、`NV_ENC_CONFIG`（含 H.264/HEVC 子结构）、`NV_ENC_RC_PARAMS`、`NV_ENC_REGISTER_RESOURCE`、`NV_ENC_MAP_INPUT_RESOURCE`、`NV_ENC_CREATE_BITSTREAM_BUFFER`、`NV_ENC_PIC_PARAMS`、`NV_ENC_LOCK_BITSTREAM`、`NV_ENC_RECONFIGURE_PARAMS`、`NV_ENC_SEQUENCE_PARAM_PAYLOAD`。

【官方文档】`NV_ENCODE_API_FUNCTION_LIST` 定义见头文件 4550–4660 行；`bindgen` 生成最稳妥。

---

## 10. 与 AMF 的参数对应表

> AMF 侧的属性名按项目现有封装习惯给出；具体的 `AMFPropertyStorage` 键名需对照 amf 头文件核对（本文未逐项验证）。NVENC 侧字段名已核对 SDK 13.1 头文件。

| 语义 | AMF（原生） | NVENC | 备注 |
|---|---|---|---|
| 使用场景/延迟档 | `AMF_VIDEO_ENCODER_USAGE`（`..._ULTRA_LOW_LATENCY` / `..._LOW_LATENCY` / `..._TRANSCODING`） | `tuningInfo = NV_ENC_TUNING_INFO_ULTRA_LOW_LATENCY` | AMF 的 usage 同时决定 B 帧/预分析默认值；NVENC 由 tuningInfo+preset 决定 |
| 质量/速度档 | `AMF_VIDEO_ENCODER_QUALITY_PRESET`（SPEED/BALANCED/QUALITY） | `presetGUID = P1..P7` | P1≈SPEED，P4≈BALANCED，P7≈QUALITY；ULL 下只用 P1~P3 |
| 码控模式 | `AMF_VIDEO_ENCODER_RATE_CONTROL_METHOD`（CBR / VBR_LATENCY / CQP） | `rcParams.rateControlMode`（CBR / VBR / CONSTQP） | AMF 的 VBR_LATENCY 对应 NVENC 的 VBR + 小 VBV |
| 目标码率 | `AMF_VIDEO_ENCODER_TARGET_BITRATE` | `rcParams.averageBitRate` | 单位都是 bits/sec |
| 峰值码率 | `AMF_VIDEO_ENCODER_PEAK_BITRATE` | `rcParams.maxBitRate` | NVENC 的 CBR 忽略该字段 |
| VBV 大小 | `AMF_VIDEO_ENCODER_VBV_BUFFER_SIZE` | `rcParams.vbvBufferSize` | AMF 常按 bits；两边都要按“帧预算”设小值 |
| VBV 初始填充 | `AMF_VIDEO_ENCODER_INITIAL_VBV_BUFFER_FULLNESS` | `rcParams.vbvInitialDelay` | AMF 常是 0~1 比例；NVENC 是 bits |
| 关键帧间隔 | `AMF_VIDEO_ENCODER_IDR_PERIOD` / `KEY_FRAME_INTERVAL` | `gopLength` / `idrPeriod` | 60fps、2s → 120 |
| B 帧模式 | `AMF_VIDEO_ENCODER_B_PIC_PATTERN` | `frameIntervalP` | 远程桌面都应为 0 B 帧（NVENC 用 1=IPP） |
| 前向参考/重排 | `AMF_VIDEO_ENCODER_MAX_NUM_REFRAMES`（按封装） | `zeroReorderDelay=1` | 目标都是“不重排” |
| 前分析/lookahead | `AMF_VIDEO_ENCODER_RATE_CONTROL_PRE_ANALYSIS_ENABLE` + `PRE_ANALYSIS_SIZE` | `enableLookahead=0` / `lookaheadDepth=0` | 低延迟必须关；AMF 的 pre-analysis 同理 |
| 空间 AQ | AMF 低延迟 usage 内建的 AQ（属性名待核对） | `enableAQ=1` + `aqStrength` | NVENC 可显式调强度 |
| 时间 AQ | AMF 同名特性（待核对） | `enableTemporalAQ=0` | 两边低延迟都建议关 |
| QP 上下限 | `AMF_VIDEO_ENCODER_QP_MIN` / `QP_MAX`（按封装） | `enableMinQP/maxQP` + `minQP/maxQP` | 防止码控被夹死 |
| 帧内刷新 | `AMF_VIDEO_ENCODER_INTRA_REFRESH_NUM_MBS_PER_SLOT` / `INTRA_REFRESH_PERIOD` | `enableIntraRefresh` + `intraRefreshPeriod` + `intraRefreshCnt` | NVENC 要求 `gopLength=INFINITE` |
| 填充数据 | `AMF_VIDEO_ENCODER_RATE_CONTROL_FILLER_DATA_ENABLE` | `enableFillerDataInsertion` | 严格 CBR 才开，远程桌面不建议 |
| 强制 IDR | `AMF_VIDEO_ENCODER_FORCE_PICTURE_TYPE = IDR` | `NV_ENC_PIC_FLAG_FORCEIDR`（需 `enablePTD=1`） | 新观看者加入/丢包恢复 |
| 切片数 | `AMF_VIDEO_ENCODER_SLICES_PER_FRAME` | `sliceMode` + `sliceModeData` | 切片多利于切包/FEC |
| 输入表面 | `AMFSurface`（DX11 纹理） | `NV_ENC_REGISTER_RESOURCE` + `MapInputResource` | 都是“纹理进、不落 CPU” |
| 动态改码率 | `SetProperty(TARGET_BITRATE)` | `NvEncReconfigureEncoder`（`resetEncoder=0`） | 不重建会话、不插 IDR |
| 多实例 | 每个 component 一个实例 | 每个 `NvEncOpenEncodeSessionEx` 一个 session | 受并发上限约束 |

【推断】AMF 列（属性名为常见写法，未在本项目代码中逐一核对）；NVENC 列【官方文档】字段语义。

---

## 11. 未确认 / 必须实测的项

1. **D3D11 输入纹理的 bind flags 下限**：官方只要求“可注册”，未写必须带哪些 flag。实测 WGC/Desktop Duplication 纹理是否可直接注册；若 `RESOURCE_REGISTER_FAILED`，尝试补 `RENDER_TARGET`。
2. **P010 ↔ `NV_ENC_BUFFER_FORMAT_YUV420_10BIT` 的 DXGI 对应关系**：官方未明确，需试编验证。
3. **DX11 下 4:4:4 10-bit**：官方能力表说 HEVC 支持 4:4:4、支持 10-bit，但二者组合在 DX11 纹理路径是否可用，论坛有否定说法，需实测。
4. **RGB 直入 HEVC/AV1**：官方只对 H.264 承诺；切 HEVC 时必须查 `NvEncGetInputFormats` 并准备 GPU 转换路径。
5. **NVENC 的 RGB→YUV 矩阵/范围**（BT.601 还是 BT.709、limited/full）：官方未说明；若颜色偏差明显，改为自己用 shader/VideoProcessor 转 NV12 并在 VUI 里显式标 BT.709。
6. **首帧真实延迟**：受驱动预热、D3D 提交、`LockBitstream` 调度影响；必须在目标卡上打点（提交→event/出包）而不是套用 1 ms 估计。
7. **SDK 12.2 的精确最低驱动**：本文只找到 551.76（论坛/第三方），NVIDIA 12.2 文档正文未直接给出。
8. **本机无 N 卡**：以上所有“能否直入/首帧延迟/4:4:4 10-bit”的结论都要在至少一张 Turing/Ampere/Ada 卡上复测后才能写进产品验收。

---

## 12. 参考链接

- NVENC 编程指南（13.1）：https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/nvenc-video-encoder-api-prog-guide/
- NVENC 应用说明/能力表/许可策略（13.1）：https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/nvenc-application-note/
- SDK 13.1 Read Me（驱动 610+、包内容）：https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/read-me/
- SDK 13.0 Read Me（驱动 570+）：https://docs.nvidia.com/video-technologies/video-codec-sdk/13.0/read-me/
- DesignWorks SDK License：https://docs.nvidia.com/video-technologies/video-codec-sdk/13.1/license/
- SDK 下载（需接受许可）：https://developer.nvidia.com/video-codec-sdk/download
- GPU 编解码支持矩阵：https://developer.nvidia.com/video-encode-and-decode-gpu-support-matrix-new
- `nvEncodeAPI.h` 公开镜像（字段注释以此为准）：https://github.com/FFmpeg/nv-codec-headers/blob/master/include/ffnvcodec/nvEncodeAPI.h
- FFmpeg NVENC 接入实现（版本判断、VBV 默认值参考）：https://github.com/FFmpeg/FFmpeg/blob/master/libavcodec/nvenc.c
- 并发会话上限答复：https://forums.developer.nvidia.com/t/how-to-increase-number-of-nvenc-concurrent-sessions/169367
- D3D11 输入格式映射讨论：https://forums.developer.nvidia.com/t/video-codec-buffer-formats-supported-w-d3d11/210169
- Optimus 图形上下文要求：https://docs.nvidia.com/cuda/archive/11.0_GA/pdf/Optimus_Developer_Guide.pdf
