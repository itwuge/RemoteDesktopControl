# Intel 硬件编码接口技术备忘：oneVPL / Intel Media SDK

> 项目：RemoteDesktopControl（Rust + Flutter，Windows 桌面采集 → 硬件编码 → 网络）
> 目标：1080p60，玻璃到玻璃 p99 ≤ 30 ms，D3D11 纹理直入编码器，码率可控
> 调研日期：2026-11-07
> 范围：Intel 核显（Gen12 / Xe / Xe2）与 Arc 独显（Alchemist / Battlemage）；只写文档，不写项目实现代码，不安装 SDK 或驱动。

**结论标记约定**

- 【官方文档】：Intel 官方文档、Intel 官方 GitHub、VPL 规范/头文件、微软文档、FFmpeg 官方文档。
- 【推断】：由官方字段/行为组合推导，或工程经验值；落地前必须在本机实测。
- 【未确认】：公开资料无法确认，不编造；需要实测或向 Intel 求证。

---

## 0. 先看这里：硬结论

1. **新项目直接用 oneVPL（libvpl 2.x），不要从 Media SDK 开始。** oneVPL 2.x 是 Media SDK 1.x 的后继，Media SDK 项目已停止更新；Windows 上 14 代及更新平台的驱动包只带 VPL runtime。【官方文档】[Intel VPL 概览](https://www.intel.com/content/www/us/en/developer/tools/vpl/overview.html) / [Upgrade Summary](https://www.intel.com/content/www/us/en/docs/onevpl/upgrade-from-msdk/2023-1/summary.html)

2. **Windows 上应用只随包分发 dispatcher `libvpl.dll`；VPL runtime 由 Intel 显卡驱动安装，不要打包驱动组件。** runtime 在 Driver Store 中的名字是 `libmfx64-gen.dll`（Xe 及更新），旧 Media SDK runtime 是 `libmfxhw64.dll`。【官方文档】[VPL Session / 库搜索](https://intel.github.io/libvpl/latest/programming_guide/VPL_prg_session.html)

3. **用 `MFXLoad` → `MFXCreateConfig` → `MFXSetConfigFilterProperty` → `MFXEnumImplementations` → `MFXCreateSession`。不要用 `MFXInit`/`MFXInitEx`：它们从 API 2.0 起弃用，且会退回 1.x API / Media SDK 路径。**【官方文档】[VPL Session](https://intel.github.io/libvpl/latest/programming_guide/VPL_prg_session.html) / [2021 Release Notes](https://www.intel.com/content/www/us/en/developer/articles/release-notes/oneapi-video-processing-library-release-notes-2021.html)

4. **零拷贝有两条正统路径。** 新路径是 API 2.10+ 的 `MFXGetMemoryInterface` + `ImportFrameSurface`（`mfxSurfaceD3D11Tex2D`），官方示例就是“捕获桌面纹理 → 导入 → 编码”；旧路径是外部 surface + `Data.MemId = ID3D11Texture2D` + `IOPattern = MFX_IOPATTERN_IN_VIDEO_MEMORY`。【官方文档】[Shared Surfaces](https://intel.github.io/libvpl/latest/programming_guide/VPL_prg_surface_sharing.html) / [HW Acceleration](https://intel.github.io/libvpl/latest/programming_guide/VPL_prg_hw.html)

5. **编码器输入必须是 NV12 或 P010，不是 BGRA。** Desktop Duplication 给的是 BGRA8；必须先做一次 GPU 颜色转换（D3D11 VideoProcessor、compute shader，或 VPL VPP），再把 NV12/P010 纹理交给编码器。零拷贝指“不进系统内存”，不是“没有颜色转换”。【官方文档】[Media Capabilities](https://cdrdv2-public.intel.com/773553/onevpl_developer-reference-media-intel-hardware_1.1-772607-773553.pdf) / [Desktop Duplication](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/nn-dxgi1_2-idxg1outputduplication)

6. **远程桌面必须 `AsyncDepth = 1`，并且 `GopRefDist = 1`（无 B 帧）。** 再配合 `NumRefFrame = 1`、`GopPicSize = 60~120`、`IdrInterval = 0`、关闭 lookahead/ExtBRC/pre-analysis、CBR + 小 `BufferSizeInKB`。【官方文档】[Appendix C / Low Latency](https://intel.github.io/libvpl/latest/appendix/VPL_apnds_c.html) / [mfxInfoMFX](https://intel.github.io/libvpl/latest/API_ref/VPL_structs_cross_component.html)

7. **1080p60 的起步码率**：H.264 High CBR 8–12 Mbps；HEVC Main CBR 5–8 Mbps；AV1（仅 Arc）4–6 Mbps。`BufferSizeInKB` 先按 100–200 KB、`InitialDelayInKB` 0–50 KB 起步，再按实测调。【推断】[BufferSizeInKB 字段](https://intel.github.io/libvpl/latest/API_ref/VPL_structs_cross_component.html)

8. **没有 `MFXVideoENCODE_GetBitStream` 这个函数。** 编码输出是 `MFXVideoENCODE_EncodeFrameAsync` 的出参 `mfxBitstream`；`EncodeFrameAsync` 只保证“异步提交”，必须 `MFXVideoCORE_SyncOperation` 之后才能读码流。【官方文档】[VideoENCODE 函数列表](https://intel.github.io/libvpl/latest/API_ref/VPL_func_vid_encode.html)

9. **能力必须 Query，不能只看型号表。** Gen12 核显（Tiger Lake~Raptor Lake）没有 AV1 编码，Arc A 有；AVC 在全系 Intel 硬件上都只有 8-bit 4:2:0；HEVC 4:4:4 在 Gen12 与 Arc 上都有，但 4:2:2 在 Gen12 上是 shader 路径（慢），不适合低延迟。【官方文档】[Media Capabilities](https://cdrdv2-public.intel.com/773553/onevpl_developer-reference-media-intel-hardware_1.1-772607-773553.pdf)

10. **6~10 代（含 Ice Lake）没有 VPL runtime，只能走 Media SDK 1.x；11 代 Tiger Lake 起才有 VPL runtime。** 如果产品必须支持老平台，需要保留 Media SDK 1.x 或 Media Foundation 回退；否则直接把最低平台定在 11 代 / Arc。【官方文档】[libvpl README 平台表](https://github.com/intel/libvpl) / [vpl-gpu-rt README](https://github.com/intel/vpl-gpu-rt)

---

## 1. API 选型：oneVPL 与 Media SDK

### 1.1 两者关系与现状

- oneVPL 2.x 与 Media SDK 1.x **源码兼容**，但不是同一个东西：oneVPL 是新一代 API + dispatcher，Media SDK 是旧 API + 旧 runtime。【官方文档】[VPL for MSDK Users](https://intel.github.io/libvpl/v2.10/VPL_intel_media_sdk.html)
- Media SDK 项目已停止更新，最新 API 1.35；oneVPL 继续更新（当前规范 2.17.0）。【官方文档】[Upgrade Summary](https://www.intel.com/content/www/us/en/docs/onevpl/upgrade-from-msdk/2023-1/summary.html) / [VPL 规范首页](https://intel.github.io/libvpl/latest/)
- oneVPL 现在**只面向 GPU**；oneVPL-CPU 项目已结束，不再随 oneAPI Base Toolkit 分发。这意味着没有“VPL 软件编码器”兜底，没 GPU 就必须换 Media Foundation / 软件编码。【官方文档】[Upgrade Summary](https://www.intel.com/content/www/us/en/docs/onevpl/upgrade-from-msdk/2023-1/summary.html)

**架构关系**

```
应用
  └─ libvpl.dll（dispatcher，MIT，应用可分发）
       ├─ libmfx64-gen.dll（Intel VPL GPU runtime，驱动安装，Xe 及更新）
       └─ libmfxhw64.dll（Intel Media SDK runtime，驱动/旧包安装，老平台）
```

【官方文档】[VPL Session / 库文件名表](https://intel.github.io/libvpl/latest/programming_guide/VPL_prg_session.html)

**平台支持断代**

| 平台 | VPL GPU runtime | Media SDK runtime | 建议 |
|---|---|---|---|
| Broadwell ~ Comet Lake（6~10 代，Gen9/Gen10） | 不支持 | 支持 | 只能用 Media SDK 1.x；无 AV1 |
| Ice Lake（10 代，Gen11） | 不支持 | 支持 | 只能用 Media SDK 1.x |
| Tiger Lake / Rocket Lake（11 代，Gen12） | 支持 | 支持 | 优先 VPL |
| Alder Lake / Raptor Lake（12~13 代） | 支持 | 支持 | 优先 VPL |
| Meteor Lake / Arrow Lake / Lunar Lake | 支持 | 支持 | 优先 VPL |
| DG1 / Arc A（Alchemist，DG2）/ ATSM | 支持 | 支持 | 优先 VPL |
| Arc B（Battlemage）/ PTL / NVL | 支持 | 未确认 | 优先 VPL |

【官方文档】[libvpl README](https://github.com/intel/libvpl) / [vpl-gpu-rt README](https://github.com/intel/vpl-gpu-rt)

### 1.2 Windows 分发、驱动门槛、版本探测

**分发模型**

| 组件 | 文件名（Windows x64） | 谁提供 | 应用是否随包 |
|---|---|---|---|
| dispatcher | `libvpl.dll` | libvpl 源码构建 / Intel 预编译包，MIT | 可以，建议随包 |
| VPL GPU runtime | `libmfx64-gen.dll` | Intel 显卡驱动安装到 Driver Store | 不要打包 |
| Media SDK runtime | `libmfxhw64.dll` | 旧驱动 / 旧 Media SDK 包 | 不要打包 |

【官方文档】[VPL Session / 库文件名与搜索顺序](https://intel.github.io/libvpl/latest/programming_guide/VPL_prg_session.html)

- Windows 搜索顺序：**Driver Store → 当前 exe 目录 → PATH → `ONEVPL_SEARCH_PATH`**；`ONEVPL_PRIORITY_PATH` 可让自定义库获得最高优先级。【官方文档】[VPL Session](https://intel.github.io/libvpl/latest/programming_guide/VPL_prg_session.html)
- 运行时随显卡驱动安装，不需要单独安装 SDK。Intel 驱动安装包内容里明确列出 “Intel oneVPL GPU Runtime 21.0.2.x”（例如 31.0.101.4125、32.0.101.7085 的 release notes）。【官方文档】[驱动 101.4125 Release Notes](https://downloadmirror.intel.com/768896/ReleaseNotes_101.4125_BETA.pdf) / [驱动 101.7085 Release Notes](https://downloadmirror.intel.com/915569/ReleaseNotes_101.7085.pdf)
- 2021 年的 oneVPL 安装指南把 **30.0.100.9805** 作为“runtime 随驱动打包”的分界；更老的驱动可能需要单独 runtime 包。该文档年代较早，当前 Arc/核显驱动（32.0.101.x）均已包含 runtime。【官方文档】[oneVPL Installation Guide](https://www.intel.com/content/www/us/en/developer/articles/guide/onevpl-installation-guide.html) / 【推断】当前驱动已包含
- 14 代 Core 及更新平台的 Windows 驱动包**只带 VPL runtime**，不再带 Media SDK runtime。【官方文档】[Upgrade Summary](https://www.intel.com/content/www/us/en/docs/onevpl/upgrade-from-msdk/2023-1/summary.html)

**驱动门槛（工程建议）**

- 核显 11 代及以上：Intel Graphics Driver ≥ 30.0.100.9805；实际建议直接用 31.0.101.x 以上。
- Arc A/B：使用对应 Arc 驱动（31.0.101.x 起，当前 32.0.101.x）。
- 判据不是版本号本身，而是 `MFXEnumImplementations` 能否枚举到实现；枚举不到就提示用户更新驱动。【推断】[VPL Session / MFXCreateSession](https://intel.github.io/libvpl/latest/programming_guide/VPL_prg_session.html)

**API 版本探测**

- 编译期：`MFX_VERSION_MAJOR` / `MFX_VERSION_MINOR`。
- 运行期（推荐）：`MFXEnumImplementations(loader, i, MFX_IMPL_CAPS_IMPL_DESC, &desc)`，读 `desc->ApiVersion`；也可直接用 dispatcher 过滤器 `mfxImplDescription.ApiVersion.Version`（U32）或 `.Major` / `.Minor`（U16）。【官方文档】[VPL Session / 配置属性表](https://intel.github.io/libvpl/latest/programming_guide/VPL_prg_session.html)
- 会话创建后：`MFXQueryVersion(session, &ver)` 返回**实现**的版本（不是 dispatcher 版本）。【官方文档】[Session Management / MFXQueryVersion](https://intel.github.io/libvpl/latest/API_ref/VPL_func_session.html)
- `mfxVersion` 是 union：`Version = (Major << 16) | Minor`。判断“是否支持 2.10 的 ImportFrameSurface”，用 `ApiVersion.Version >= (2 << 16 | 10)`。【推断，基于 mfxVersion 布局】[Implementation Management](https://intel.github.io/libvpl/latest/API_ref/VPL_structs_impl_manage.html)

### 1.3 `MFXLoad`/`MFXCreateSession` 与 `MFXInit` 的差别

| 维度 | `MFXInit` / `MFXInitEx` | `MFXLoad` + `MFXCreateSession` |
|---|---|---|
| 状态 | API 2.0 起弃用 | 推荐入口 |
| API 能力 | 限制在 1.x API | 可用 2.x API（含 ImportFrameSurface 等） |
| 加载的 runtime | 优先 Media SDK `libmfxhw64.dll`，不是 VPL runtime | 按能力枚举 VPL runtime；老平台才落到 MSDK |
| 选择能力 | 基本只能选 impl 类型 | 可按 codec/profile/分辨率/颜色格式/适配器/API 版本过滤 |
| 多实现 | 难 | `MFXEnumImplementations` |
| 适用 | 老代码兼容、6~10 代平台 | 新项目、11 代+ / Arc |

【官方文档】[VPL Session](https://intel.github.io/libvpl/latest/programming_guide/VPL_prg_session.html) / [2021 Release Notes](https://www.intel.com/content/www/us/en/developer/articles/release-notes/oneapi-video-processing-library-release-notes-2021.html)

**正确加载顺序**

1. `MFXLoad()` 创建 loader。
2. 每个过滤属性创建一个 `MFXCreateConfig(loader)`；**一个 config 对象只能挂一个属性**。
3. `MFXSetConfigFilterProperty(cfg, name, &variant)` 设置过滤。
4. `MFXEnumImplementations(loader, i, MFX_IMPL_CAPS_IMPL_DESC, &desc)` 枚举并记录 `ApiVersion` / `MediaAdapterType` / `DeviceID`。
5. `MFXCreateSession(loader, i, &session)`；`i` 是枚举下标，不是 DXGI 适配器下标。
6. 结束：`MFXClose(session)` → `MFXUnload(loader)`。

【官方文档】[VPL Session](https://intel.github.io/libvpl/latest/programming_guide/VPL_prg_session.html)

**常用过滤属性（工程可直接用）**

| 目的 | 属性名 | 类型 | 值 |
|---|---|---|---|
| 只要硬件实现 | `mfxImplDescription.Impl` | U32 | `MFX_IMPL_TYPE_HARDWARE` |
| 强制 D3D11 | `mfxImplDescription.AccelerationMode` | U32 | `MFX_ACCEL_MODE_VIA_D3D11` |
| 指定 DXGI 适配器 | `DXGIAdapterIndex` | U32 | `IDXGIFactory::EnumAdapters` 的下标 |
| 核显 / 独显 | `mfxImplDescription.mfxDeviceDescription.device.MediaAdapterType` | U16 | `MFX_MEDIA_INTEGRATED` / `MFX_MEDIA_DISCRETE` |
| 指定编码器 | `mfxImplDescription.mfxEncoderDescription.encoder.CodecID` | U32 | `MFX_CODEC_AVC` / `MFX_CODEC_HEVC` / `MFX_CODEC_AV1` |
| 限制 API 版本 | `mfxImplDescription.ApiVersion.Version` | U32 | `(major << 16) | minor` |
| 限制 profile | `mfxImplDescription.mfxEncoderDescription.encoder.encprofile.Profile` | U32 | 如 `MFX_PROFILE_AVC_HIGH` |
| 限制颜色格式 | `...encoder.encprofile.encmemdesc.ColorFormats` | U32 | 如 `MFX_FOURCC_NV12` |

【官方文档】[VPL Session / 配置属性表](https://intel.github.io/libvpl/latest/programming_guide/VPL_prg_session.html) / [Dispatcher 枚举](https://intel.github.io/libvpl/latest/API_ref/VPL_disp_api_enum.html)

- Dispatcher 优先级：硬件 > 软件；通用硬件 > VSI 硬件；API 版本高 > 低。`MFXCreateSession` 找不到匹配实现时返回 `MFX_ERR_NOT_FOUND`，应把“枚举数量为 0”和“过滤器过严”分开排查。【官方文档】[VPL Session](https://intel.github.io/libvpl/latest/programming_guide/VPL_prg_session.html)

---

## 2. 设备绑定与 D3D11 零拷贝

### 2.1 绑定 D3D11 设备

- `MFXVideoCORE_SetHandle(session, MFX_HANDLE_D3D11_DEVICE, (mfxHDL)pD3D11Device)`；如果是 COM 接口，runtime 会增加引用计数，session 关闭时释放。【官方文档】[MFXVideoCORE_SetHandle](https://intel.github.io/libvpl/latest/API_ref/VPL_func_vidcore.html)
- 不设置 handle 时，runtime 只能走系统内存，无法零拷贝。【官方文档】[HW Acceleration / DirectX](https://intel.github.io/libvpl/latest/programming_guide/VPL_prg_hw.html)
- D3D11 设备必须开启多线程保护（`ID3D11Multithread::SetMultithreadProtected(TRUE)`），否则采集线程、转换线程、编码线程并发访问会崩或花屏。【官方文档】[HW Acceleration](https://intel.github.io/libvpl/latest/programming_guide/VPL_prg_hw.html)
- 采集/转换/编码必须用**同一个 D3D11 设备、同一个适配器**；跨适配器纹理共享会引入额外拷贝，甚至直接失败。【官方文档】[HW Acceleration](https://intel.github.io/libvpl/latest/programming_guide/VPL_prg_hw.html)
- 设备丢失（驱动更新、TDR、MUX 切换、显示器热插拔）时，`EncodeFrameAsync` 返回 `MFX_ERR_DEVICE_LOST`；必须销毁 session/encoder/device 并重建。【官方文档】[VideoENCODE 返回码](https://intel.github.io/libvpl/latest/API_ref/VPL_func_vid_encode.html) / 【推断】重建流程

### 2.2 两条零拷贝路径 + 一条回退路径

#### 路径 A：API 2.10+ 的 `ImportFrameSurface`（推荐）

官方示例的流程就是“桌面捕获纹理 → 导入 VPL → 编码”：

1. `MFXGetMemoryInterface(session, &memoryInterface)`。
2. 填 `mfxSurfaceD3D11Tex2D`：
   - `SurfaceInterface.Header.SurfaceType = MFX_SURFACE_TYPE_D3D11_TEX2D`
   - `SurfaceInterface.Header.SurfaceFlags = MFX_SURFACE_FLAG_IMPORT_SHARED | MFX_SURFACE_FLAG_IMPORT_COPY`
   - `SurfaceInterface.Header.StructSize = sizeof(mfxSurfaceD3D11Tex2D)`
   - `texture2D = pTexture2D`
3. `memoryInterface->ImportFrameSurface(memoryInterface, MFX_SURFACE_COMPONENT_ENCODE, external_surface, &imported_surface)`。
4. 给 `imported_surface->Data.TimeStamp` 赋值，然后 `EncodeFrameAsync`。
5. `imported_surface->FrameInterface->Release(imported_surface)`。

【官方文档】[Importing and Exporting Shared Surfaces](https://intel.github.io/libvpl/latest/programming_guide/VPL_prg_surface_sharing.html)

- `IMPORT_SHARED | IMPORT_COPY` 的含义：优先零拷贝共享；共享不支持时允许 runtime 做一次拷贝。官方明确说“zero-copy (shared) 优先，copy 允许”。【官方文档】[同上](https://intel.github.io/libvpl/latest/programming_guide/VPL_prg_surface_sharing.html)
- 仅支持 **internal memory management** 模型；需要 runtime 实现 API 2.10+。低于 2.10 的 runtime 没有这个接口。【官方文档】[同上](https://intel.github.io/libvpl/latest/programming_guide/VPL_prg_surface_sharing.html)

#### 路径 B：1.x 外部 surface + `Data.MemId`（兼容性最好）

1. `mfxVideoParam.IOPattern` 包含 `MFX_IOPATTERN_IN_VIDEO_MEMORY`。
2. 准备 `mfxFrameSurface1`，`Info` 与编码参数一致（Width/Height/Crop/FourCC）。
3. `surface.Data.MemId = (mfxMemId)pID3D11Texture2D`；`surface.Data.Pitch` 对视频内存通常填 0。
4. 直接 `EncodeFrameAsync(session, ctrl, surface, bs, syncp)`。

【官方文档】[HW Acceleration / Work with Video Memory](https://intel.github.io/libvpl/latest/programming_guide/VPL_prg_hw.html) / 【推断】D3D11 纹理指针语义（FFmpeg 文档明确 DXVA2 的 MemId 是 `IDirect3DSurface9*`，D3D11 对应纹理指针）[FFmpeg pixfmt 说明](https://ffmpeg.org/doxygen/trunk/pixfmt_8h.html)

- 编码器 `Init` 只申请内部 reconstruction surface，不会要求应用提供整条外部 surface chain；应用每次调用传入一个带 `MemId` 的 surface 即可。【官方文档】[HW Acceleration](https://intel.github.io/libvpl/latest/programming_guide/VPL_prg_hw.html)
- 这条路径在 MSDK 1.x 和 VPL 上都可用，是老平台唯一可行的零拷贝方式。【推断】

#### 路径 C：内部 surface + `GetNativeHandle` + `CopyResource`（一次 GPU 拷贝）

1. `MFXMemory_GetSurfaceForEncode(session, &surface)` 取内部 surface。
2. 通过 `mfxFrameSurfaceInterface::GetNativeHandle` 拿到内部 `ID3D11Texture2D`。
3. `ID3D11DeviceContext::CopyResource` 从采集纹理拷到内部纹理。
4. `EncodeFrameAsync`。

【官方文档】[Memory Structures / mfxFrameSurfaceInterface](https://intel.github.io/libvpl/latest/API_ref/VPL_structs_memory.html) / [Intel 社区：Texture2D 编码](https://community.intel.com/t5/Media-Intel-oneAPI-Video/Using-D3D11Texture2D-to-encode-using-oneVPL/m-p/1488164)

- 这条路径不是零拷贝，但仍是 GPU 内拷贝，不进系统内存；在 Import 不可用、或跨适配器共享麻烦时，比 `Map` 到系统内存再拷回好得多。【推断】

**选择建议**

| 条件 | 用哪条 |
|---|---|
| runtime ApiVersion ≥ 2.10，纹理同设备同适配器 | A（ImportFrameSurface） |
| runtime < 2.10，或需要兼容 MSDK 1.x | B（MemId） |
| A 失败、B 不稳定，且能接受一次 GPU 拷贝 | C（CopyResource） |

**纹理数组的坑**

- 优先用**单张 2D 纹理**，不要用 Texture2DArray 的 slice 直接喂编码器；社区反馈会出现 `MFX_ERR_LOCK_MEMORY`、错帧或直接挂起。若采集 API 只给数组，先 `CopySubresourceRegion` 到单张纹理。【推断】[Intel 社区 D3D11 纹理问题](https://community.intel.com/t5/Media-Intel-oneAPI-Video/Using-D3D11Texture2D-to-encode-using-oneVPL/m-p/1488164)
- Import 路径对纹理的 bind flags 没有在公开文档里写死；实践上 NV12/P010 纹理用 `D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE`、`D3D11_USAGE_DEFAULT`、无 CPU access 最稳。【推断】[D3D11 纹理创建](https://learn.microsoft.com/en-us/windows/win32/api/d3d11/nf-d3d11-id3d11device-createtexture2d)

### 2.3 格式现实：BGRA 不能直接进编码器

- 编码器输入：`MFX_FOURCC_NV12`（8-bit 4:2:0）或 `MFX_FOURCC_P010`（10-bit 4:2:0）。【官方文档】[Media Capabilities](https://cdrdv2-public.intel.com/773553/onevpl_developer-reference-media-intel-hardware_1.1-772607-773553.pdf) / [mfxFrameInfo](https://intel.github.io/libvpl/latest/API_ref/VPL_structs_memory.html)
- Desktop Duplication 输出：`DXGI_FORMAT_B8G8R8A8_UNORM`（BGRA8）。【官方文档】[Desktop Duplication API](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/nn-dxgi1_2-idxg1outputduplication)
- 因此 1080p60 的真实管线是：

```
Desktop Duplication (BGRA8 texture)
  → GPU 颜色转换（VideoProcessor / compute shader / VPL VPP）
  → NV12 或 P010 texture（同设备）
  → ImportFrameSurface 或 MemId
  → oneVPL encode
```

- 颜色转换的三种做法：
  - D3D11 VideoProcessor：`ID3D11VideoProcessorBlt`，成熟、兼容好，适合 BGRA→NV12；【推断】[ID3D11VideoProcessor](https://learn.microsoft.com/en-us/windows/win32/api/d3d11/nn-d3d11-id3d11videoprocessor)
  - Compute shader：一次 pass 同时写 Y/UV，延迟最低，但要做色彩矩阵和 chroma siting；【推断】
  - VPL VPP：`MFXVideoVPP_ProcessFrameAsync`，可做颜色转换/缩放；是否能直接吃 Import 的 BGRA 纹理需实测。【未确认】[VideoVPP](https://intel.github.io/libvpl/latest/API_ref/VPL_func_vid_vpp.html)
- HDR（FP16 / BT2020）到 P010 需要 tone mapping，oneVPL 不负责；本项目 v1 建议只做 SDR。【推断】

### 2.4 核显 / Arc / Optimus 多显卡

- 用 `DXGIAdapterIndex` 过滤器直接指定适配器；它只过滤硬件实现，下标与 `IDXGIFactory::EnumAdapters` 一致。【官方文档】[VPL Session](https://intel.github.io/libvpl/latest/programming_guide/VPL_prg_session.html)
- 用 `MediaAdapterType` 区分核显/独显（`MFX_MEDIA_INTEGRATED` / `MFX_MEDIA_DISCRETE`），比猜 DeviceID 稳。【官方文档】[VPL Session / 配置属性](https://intel.github.io/libvpl/latest/programming_guide/VPL_prg_session.html)
- **采集和编码要在同一个适配器上**。Desktop Duplication 的 `IDXGIOutputDuplication` 属于“驱动当前显示输出的适配器”；多数笔记本桌面由核显输出，即使游戏由 Arc 渲染，采集仍在核显上。【推断】[Desktop Duplication](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/nn-dxgi1_2-idxg1outputduplication)
- 因此：
  - 普通 Optimus 笔记本：采集在核显 → 编码也在核显，延迟最低；不要为了“用 Arc”而跨适配器拷贝。
  - MUX 直连 Arc / 外接屏由 Arc 输出：采集和编码都在 Arc。
  - 需要 Arc 的 AV1/更高吞吐且采集在核显：接受一次跨适配器共享拷贝，或放弃 AV1。
- 老式 `MFXInit` 会自动搜索显卡设备，在多显卡/切换显卡场景下可能选错；新 API 显式枚举 + 过滤可以避免。【官方文档】[Switchable Graphics](https://intel.github.io/libvpl/latest/appendix/VPL_apnds_d.html)

---

## 3. 能力探测：`MFXVideoENCODE_Query`

### 3.1 Query 的四种模式

| 模式 | in | out | 行为 | 用途 |
|---|---|---|---|---|
| 1 | NULL | 非 NULL | 返回该实现的可配置能力；应用把想查的字段置 0，支持则填非 0 | 查某字段是否可配置 |
| 2 | 非 NULL | 非 NULL | 校验输入并**修正**输出；无法修正则置 0 | 最常用：提交完整参数，看是否被改 |
| 3 | 非 NULL | NULL | 只检查是否支持 | 快速支持性判断 |
| 4 | 非 NULL + `mfxExtEncoderCapability` | 非 NULL | 返回编码器能力（吞吐等） | 查硬件能力上限 |

【官方文档】[MFXVideoENCODE_Query](https://intel.github.io/libvpl/latest/API_ref/VPL_func_vid_encode.html)

- Query 必须在 session 创建后调用；`out->mfx.CodecId` 是应用必须填的字段。【官方文档】[同上](https://intel.github.io/libvpl/latest/API_ref/VPL_func_vid_encode.html)
- 返回 `MFX_ERR_NONE` 不一定代表“完全按你的参数来”：要看 out 是否被改、是否返回 `MFX_WRN_INCOMPATIBLE_VIDEO_PARAM`。【官方文档】[同上](https://intel.github.io/libvpl/latest/API_ref/VPL_func_vid_encode.html)
- 返回 `MFX_WRN_PARTIAL_ACCELERATION` 表示“部分加速”（例如 4:2:2/4:4:4 走 shader），低延迟场景应视为不支持。【官方文档】[同上](https://intel.github.io/libvpl/latest/API_ref/VPL_func_vid_encode.html) / 【推断】低延迟判据

### 3.2 能查什么

| 维度 | 怎么查 | 说明 |
|---|---|---|
| 编码格式 H.264 / HEVC / AV1 | `mfx.CodecId` | 不支持会返回 `MFX_ERR_UNSUPPORTED` 或被置 0 |
| profile（High / Main10 / REXT / 4:4:4） | `mfx.CodecProfile` | 用对应枚举；不支持的 profile 会被置 0 |
| level | `mfx.CodecLevel` | 可设 `MFX_LEVEL_UNKNOWN` 让实现自选；也可指定 4.2/5.1 等 |
| 输入 FourCC / 位深 | `mfx.FrameInfo.FourCC` / `BitDepthLuma` / `BitDepthChroma` | NV12=8-bit，P010=10-bit |
| 分辨率 / 帧率 | `Width` / `Height` / `CropW` / `CropH` / `FrameRateExtN` / `FrameRateExtD` | 传实际值；被修正说明不支持该组合 |
| 低延迟选项 | 附加 `mfxExtCodingOption` / `2` / `3` | 不支持的扩展字段会被置 0 或修正 |
| 码控 | `RateControlMethod` / `TargetKbps` / `MaxKbps` / `BufferSizeInKB` | 不同 codec 支持的模式不同 |
| 硬件吞吐 | 模式 4 + `mfxExtEncoderCapability` | 返回 MB/s 等能力；适合判断 4K/多路是否可行 |

【官方文档】[MFXVideoENCODE_Query](https://intel.github.io/libvpl/latest/API_ref/VPL_func_vid_encode.html) / [mfxInfoMFX](https://intel.github.io/libvpl/latest/API_ref/VPL_structs_cross_component.html)

**推荐探测顺序**

1. 用“目标参数”直接走模式 2：H.264 High / NV12 / 1920x1080 / 60fps / CBR / AsyncDepth=1 / GopRefDist=1 + 低延迟扩展。
2. 如果 `MFX_ERR_UNSUPPORTED` 或 profile 被置 0：降级到 H.264 Main / baseline。
3. 如果仍失败：换 HEVC Main；再失败换 NV12 8-bit；再失败才考虑系统内存。
4. 如果返回 `MFX_WRN_PARTIAL_ACCELERATION`：拒绝该组合（低延迟不合格），不要静默接受。
5. 记录 out 的每个字段与警告，作为运行时日志的一部分。【推断】

### 3.3 核显与 Arc 的能力差异（以 Intel 官方能力表为准）

| 编码能力 | Gen12 核显（TGL~RPL） | Arc A（DG2） | 备注 |
|---|---|---|---|
| AVC 8-bit 4:2:0 | 支持 | 支持 | 全系 AVC 只有这一种 |
| AVC 10-bit / 4:2:2 / 4:4:4 | 不支持 | 不支持 | AVC 没有 High10/High422/High444 |
| HEVC 8/10-bit 4:2:0 | 支持 | 支持 | 主路径 |
| HEVC 4:4:4 8/10-bit | 支持 | 支持 | 文本清晰度可选，但客户端兼容性差 |
| HEVC 4:2:2 8/10-bit | shader（慢） | 硬件 | Gen12 的 4:2:2 不要用于低延迟 |
| AV1 8/10-bit 4:2:0 | 不支持 | 支持 | MTL/LNL 及更新核显未确认 |
| VP9 | 支持 | 支持 | 远程桌面不用 |

【官方文档】[Media Capabilities Supported by Intel Hardware](https://cdrdv2-public.intel.com/773553/onevpl_developer-reference-media-intel-hardware_1.1-772607-773553.pdf)

- 该表是 1.1 版，未覆盖 Meteor Lake / Lunar Lake / Battlemage；新平台的实际能力以 Query 为准。Meteor Lake 数据手册列出 HEVC 4:2:2 10-bit 编码，说明新核显在补齐 4:2:2。【官方文档】[Meteor Lake Datasheet](https://edc.intel.com/content/www/cn/zh/design/products/platforms/details/meteor-lake-u-p/core-ultra-processor-datasheet-volume-1-of-2/003/hardware-accelerated-video-encode/) / 【推断】AV1 编码在新核显上可能存在
- Arc 的优势：AV1 编码、更多媒体引擎吞吐、硬件 4:2:2；不等于“编码延迟更低”。低延迟主要靠参数，不靠型号。【推断】

### 3.4 用 dispatcher 直接筛选能力（不创建 session）

dispatcher 支持按编码器能力过滤，能在 `MFXCreateSession` 之前就排除不合适的实现：

- `...mfxEncoderDescription.encoder.CodecID`（U32）
- `...encoder.encprofile.Profile`（U32）
- `...encoder.encprofile.encmemdesc.ColorFormats`（U32）
- `...encoder.encprofile.encmemdesc.Width` / `Height`（`mfxRange32U*`）
- `...encoder.BiDirectionalPrediction`（U16，可判断 B 帧能力）

【官方文档】[VPL Session / 配置属性表](https://intel.github.io/libvpl/latest/programming_guide/VPL_prg_session.html)

工程上仍建议“dispatcher 粗筛 + `MFXVideoENCODE_Query` 精查”两层，因为 dispatcher 的能力描述与具体参数组合（分辨率+帧率+BRC+扩展缓冲）不是一回事。【推断】

---

## 4. 低延迟参数逐项

### 4.1 参数表

| 参数 | 作用 | 代价 | 远程桌面建议 |
|---|---|---|---|
| `mfxVideoParam.AsyncDepth` | 允许在途/缓冲的帧数；1 = 不缓冲，每帧同步 | 吞吐下降；1 时每帧 `SyncOperation` | **必须 1** |
| `mfxInfoMFX.GopRefDist` | I/P（或 GPB）间距；1 = 无常规 B 帧 | 压缩率略降 | **1** |
| `mfxInfoMFX.GopPicSize` | 关键帧（I/P）间距 | 太密浪费码率；太疏恢复慢 | 60~120 帧（1~2 s @60fps） |
| `mfxInfoMFX.IdrInterval` | 每隔多少个 I 帧出一个 IDR；0 = 每个 I 都是 IDR | IDR 多会增加码率波动 | **0**（便于丢包恢复） |
| `mfxInfoMFX.NumRefFrame` | 参考帧数量；0 = 自动 | 参考帧多压缩好、DPB 大、恢复慢 | **1**（最多 2） |
| `mfxExtCodingOption3.LowDelayBRC` | 让 VBR/QVBR/VCM 严格贴近平均帧大小，减少帧大小波动 | 复杂画面质量下降 | CBR 下无效；VBR 时开 |
| `mfxExtCodingOption3.GPB` | `GopRefDist=1` 时用 GPB（只有前向参考的 B 切片）替代 P | 编码性能略降 | HEVC 可开；AVC 先不开 |
| `mfxExtCodingOption2.LookAheadDepth` | 前置分析帧数 | 直接增加 N 帧延迟 | **0 / 不启用** |
| `mfxExtCodingOption2.ExtBRC` | 外部 BRC（常带 lookahead） | 延迟/复杂度上升 | **OFF** |
| `mfxExtCodingOption2.MaxFrameSize` / `MaxNumRePak` | 限制单帧大小，驱动反复重编码/填充 | 重打包会造成延迟尖峰 | **0 / 不启用** |
| `mfxExtCodingOption.MaxDecFrameBuffering` | 告诉解码器 DPB 大小 | 太小可能影响解码兼容性 | 设为 `NumRefFrame`（=1） |
| `mfxExtCodingOption.NalHrdConformance` | 是否强制 HRD/CPB 一致性 | 开：码率更稳但可能填充/约束；关：更自由 | 局域网可关；弱网可开 |
| `mfxExtCodingOption.VuiNalHrdParameters` | 是否写 VUI HRD 参数 | 影响解码器预缓冲策略 | 低延迟可关；需兼容时开 |
| `mfxExtCodingOption2.RepeatHeaders` | 每个 I 帧前重复 SPS/PPS | 每帧多几十字节 | **ON**（便于中途加入/丢包恢复） |
| `mfxInfoMFX.TargetUsage` | 1=质量 … 7=速度 | 速度档质量差；低功耗模式可能加延迟 | 4 或 5，实测 |
| `mfxExtCodingOption3.ScenarioInfo` | 告诉编码器场景（远程桌面/游戏串流等） | 驱动实现差异 | `MFX_SCENARIO_DISPLAY_REMOTING` |
| `mfxExtCodingOption3.ContentInfo` | 内容类型（普通视频/非视频屏幕） | 驱动实现差异 | 桌面办公 `MFX_CONTENT_NON_VIDEO_SCREEN` |
| `mfxInfoMFX.BRCParamMultiplier` | 码率字段倍增器，突破 65535 Kbps | 1080p 不需要 | 1 |

【官方文档】[Appendix C / Low Latency](https://intel.github.io/libvpl/latest/appendix/VPL_apnds_c.html) / [mfxInfoMFX](https://intel.github.io/libvpl/latest/API_ref/VPL_structs_cross_component.html) / [Encode Structures](https://intel.github.io/libvpl/latest/API_ref/VPL_structs_encode.html) / [2017 MSDK Release Notes（LowDelayBRC）](https://www.intel.com/content/dam/develop/external/us/en/documents/mediasdk-release-notes-2017-759513.pdf)

### 4.2 为什么 `AsyncDepth = 1` 是硬要求

- 官方对低延迟的定义：`AsyncDepth = 1` 可以关闭输出帧缓冲；应用必须在每一帧操作后同步。【官方文档】[Appendix C](https://intel.github.io/libvpl/latest/appendix/VPL_apnds_c.html) / [Low Latency White Paper](https://cdrdv2-public.intel.com/681829/681829-1.0.pdf)
- 队列延迟近似：`额外延迟 ≈ (AsyncDepth - 1) / fps`。60 fps 下 `AsyncDepth=2` 约 +16.7 ms，`3` 约 +33 ms，直接吃掉 p99 预算。【推断】[AsyncDepth 字段](https://intel.github.io/libvpl/latest/API_ref/VPL_structs_cross_component.html)
- 代价：每帧 `MFXVideoCORE_SyncOperation` 会串行化，吞吐下降；1080p60 的编码时间通常仍小于 16.7 ms，可以接受；4K60/多路要重新评估。【推断】

### 4.3 B 帧 / GPB 的延迟

- `GopRefDist = 1` 时没有常规 B 帧；如果 `GPB = ON`，用 GPB（只有前向参考的 B 切片）代替 P，压缩率更好且**不需要重排序延迟**。【官方文档】[mfxInfoMFX / GopRefDist](https://intel.github.io/libvpl/latest/API_ref/VPL_structs_cross_component.html) / [GPB 字段](https://intel.github.io/libvpl/latest/API_ref/VPL_structs_encode.html)
- `GopRefDist > 1` 会引入 B 帧重排序延迟，且编码器需要缓存未来帧；远程桌面不要用。【官方文档】[EncodeFrameAsync](https://intel.github.io/libvpl/latest/API_ref/VPL_func_vid_encode.html) / 【推断】延迟量随 B 帧数增加
- FFmpeg 的 QSV 实现说明 GPB 是 HEVC 的默认类型，性能略低于普通 P，但压缩率更高。【官方文档】[FFmpeg qsvenc](https://ffmpeg.org/doxygen/trunk/qsvenc_8h.html) / 【推断】AVC 上先关闭 GPB，避免兼容性问题

### 4.4 低延迟预算（1080p60，估算）

| 阶段 | 估算 | 说明 |
|---|---|---|
| 桌面捕获（含 cursor 合成） | 1–3 ms | Desktop Duplication 返回纹理 |
| BGRA→NV12 GPU 转换 | 0.5–2 ms | VideoProcessor 或 shader |
| 编码提交→sync 完成 | 3–8 ms | 核显/Arc，CBR + AsyncDepth=1 |
| 打包/网络发送 | 3–10 ms | 局域网/专线；弱网更高 |
| 客户端解码 | 3–8 ms | 硬件解码更好 |
| 渲染/显示 | 1–3 ms | 受客户端 vsync 影响 |
| **合计** | **约 12–34 ms** | p99 30 ms 需要每段都留余量 |

【推断】基于各阶段典型值；必须用时间戳埋点实测，不能拿估算当验收。

---

## 5. 码率控制

### 5.1 CBR / VBR / CQP

| 模式 | 行为 | 优点 | 缺点 | 适用 |
|---|---|---|---|---|
| CBR | 目标码率恒定，CPB 约束 | 网络友好、可预测 | 复杂画面质量下降 | **远程桌面默认** |
| VBR | 平均码率 + 峰值 | 质量更好 | 帧大小波动、缓冲可能加延迟 | 有带宽余量、能接受抖动 |
| CQP | 固定 QP，不做码率控制 | 无 BRC 缓冲、延迟最低 | 码率随内容大幅波动 | 局域网/有带宽上限保护 |

【官方文档】[mfxInfoMFX / RateControlMethod](https://intel.github.io/libvpl/latest/API_ref/VPL_structs_cross_component.html) / [Streaming Features](https://intel.github.io/libvpl/latest/appendix/VPL_apnds_c.html)

- `MFX_RATECONTROL_CBR` 下 `MaxKbps` 被忽略并等于 `TargetKbps`。【官方文档】[mfxInfoMFX / MaxKbps](https://intel.github.io/libvpl/latest/API_ref/VPL_structs_encode.html)
- VBR 下 `MaxKbps` 是峰值；`WinBRCSize` 控制滑动窗口。【官方文档】[mfxInfoMFX / MaxKbps](https://intel.github.io/libvpl/latest/API_ref/VPL_structs_encode.html)
- CQP 用 `QPI` / `QPP`（有 B 帧才用 `QPB`）；1080p 文本建议 I 20–24、P 24–28。【推断】

### 5.2 `BufferSizeInKB`、`TargetKbps`、`MaxKbps`、`InitialDelayInKB` 的关系

- `BufferSizeInKB` 的官方定义：**任何压缩帧的最大可能大小**；它同时决定码流输出缓冲的下限和 HRD/CPB 的缓冲规模。【官方文档】[EncodeFrameAsync / BufferSizeInKB](https://intel.github.io/libvpl/latest/API_ref/VPL_func_vid_encode.html) / [mfxInfoMFX](https://intel.github.io/libvpl/latest/API_ref/VPL_structs_cross_component.html)
- `InitialDelayInKB`：CPB 初始填充量；解码器/HRD 需要先攒到该值才开始播放。【官方文档】[mfxInfoMFX](https://intel.github.io/libvpl/latest/API_ref/VPL_structs_cross_component.html)
- `TargetKbps`：平均码率；`MaxKbps`：峰值码率（CBR 忽略）。【官方文档】[mfxInfoMFX](https://intel.github.io/libvpl/latest/API_ref/VPL_structs_cross_component.html)

**延迟估算（HRD 初始缓冲）**

```
初始缓冲延迟(ms) ≈ InitialDelayInKB × 8 / TargetKbps × 1000
```

例：`InitialDelayInKB=100`、`TargetKbps=10000` → 100×8/10000×1000 = 80 ms。

【推断，基于 KB/kbps 单位换算】[BufferSizeInKB 字段](https://intel.github.io/libvpl/latest/API_ref/VPL_structs_cross_component.html)

**为什么 buffer 过大会造成延迟**

1. CPB 大 → 允许更大的帧和更长的突发；解码端必须按 HRD 预缓冲，起播延迟增加。
2. 网络发送端看到更大的瞬时帧，排队/抖动增加；p99 变差。
3. 反向也成立：`BufferSizeInKB` 太小 → 大 I 帧放不下，返回 `MFX_ERR_NOT_ENOUGH_BUFFER`，或编码器 panic/降质。

【官方文档】[EncodeFrameAsync / 输出缓冲](https://intel.github.io/libvpl/latest/API_ref/VPL_func_vid_encode.html) / 【推断】网络侧影响

### 5.3 1080p60 起步值

| 编码 | 码率 | `BufferSizeInKB` | `InitialDelayInKB` | 备注 |
|---|---|---|---|---|
| H.264 High | 8–12 Mbps | 120–200 KB | 0–50 KB | 兼容性最好 |
| HEVC Main | 5–8 Mbps | 100–180 KB | 0–50 KB | 同画质省 30–40% |
| AV1（Arc） | 4–6 Mbps | 80–160 KB | 0–50 KB | 客户端解码支持要先确认 |
| 文本/办公 | 偏上值 | 偏大一点，容纳 I 帧 | 0–30 KB | 文本边缘需要码率 |
| 游戏/视频 | 偏上值 | 偏大一点 | 0–50 KB | 运动/噪声更吃码率 |

【推断】经验起步值；必须以实际内容、分辨率和客户端解码器实测校准。

- `TargetKbps > 65535` 时才需要 `BRCParamMultiplier`；1080p 用不到。【官方文档】[mfxInfoMFX](https://intel.github.io/libvpl/latest/API_ref/VPL_structs_cross_component.html)
- VBR 如果非用不可：`MaxKbps ≈ 1.3–2× TargetKbps`，`BufferSizeInKB` 仍保持 150–250 KB，不要开到 0.5 秒以上。【推断】
- CQP 如果非用不可：只在局域网/有带宽余量时用；帧大小可能瞬时到 CBR 的 2–3 倍。【推断】

### 5.4 动态改码率

- `MFXVideoENCODE_Reset` 可以改 `TargetKbps`（CBR/AVBR）或 `TargetKbps + MaxKbps`（VBR）。
- 如果 `NalHrdConformance = ON`，通常只允许 VBR，且改码率可能插入关键帧/新 sequence header；如果关闭 HRD，CBR 改 `TargetKbps` 可以不插关键帧。
- 改码率后要重新读 `GetVideoParam`，确认实际生效值。【官方文档】[Streaming Features / Dynamic Bitrate Change](https://intel.github.io/libvpl/latest/appendix/VPL_apnds_c.html)

---

## 6. 输出：码流、SPS/PPS、首帧、时间戳

### 6.1 没有 `MFXVideoENCODE_GetBitStream`

- oneVPL 的 VideoENCODE 函数只有：`Query`、`QueryIOSurf`、`Init`、`Reset`、`Close`、`GetVideoParam`、`GetEncodeStat`、`EncodeFrameAsync`。**没有 `GetBitStream`。**【官方文档】[VideoENCODE 函数列表](https://intel.github.io/libvpl/latest/API_ref/VPL_func_vid_encode.html)
- 正确做法：把 `mfxBitstream` 作为 `EncodeFrameAsync` 的出参传进去；返回 `MFX_ERR_NONE` 且 `syncp` 非空时，`SyncOperation` 完成后读 `bs`。【官方文档】[EncodeFrameAsync](https://intel.github.io/libvpl/latest/API_ref/VPL_func_vid_encode.html)

### 6.2 `EncodeFrameAsync` 的真实语义

- 不是每次调用都出包：编码器可能缓存帧，返回 `MFX_ERR_MORE_DATA`；只有返回 `MFX_ERR_NONE` 时才有“一帧的码流”。【官方文档】[EncodeFrameAsync](https://intel.github.io/libvpl/latest/API_ref/VPL_func_vid_encode.html)
- 结束流：传 `NULL` surface，重复调用排空内部缓存，直到返回 `MFX_ERR_MORE_DATA`（一次一帧）。【官方文档】[同上](https://intel.github.io/libvpl/latest/API_ref/VPL_func_vid_encode.html)
- 输出缓冲必须至少能容纳 `BufferSizeInKB` 指定的最大帧；否则 `MFX_ERR_NOT_ENOUGH_BUFFER`。【官方文档】[同上](https://intel.github.io/libvpl/latest/API_ref/VPL_func_vid_encode.html)
- `MFX_WRN_DEVICE_BUSY`：GPU 忙；先 `MFXVideoCORE_SyncOperation` 或等几毫秒，再**重试同一帧**，不要丢帧。【官方文档】[同上](https://intel.github.io/libvpl/latest/API_ref/VPL_func_vid_encode.html)

**码流生命周期（建议）**

1. 每次编码前：`bs.DataOffset = 0`，`bs.DataLength = 0`，`bs.MaxLength` 保持分配大小。
2. `EncodeFrameAsync` 返回 `MFX_ERR_NONE` → `SyncOperation(session, syncp, MFX_INFINITE)`。
3. 读 `bs.Data + bs.DataOffset`，长度 `bs.DataLength`；这就是本帧 Annex-B 码流。
4. 发送/入队后，重置 offset/length 复用；在 sync 完成前不要复用同一 `mfxBitstream`。【推断】[mfxBitstream 字段](https://intel.github.io/libvpl/latest/API_ref/VPL_structs_memory.html)
- `mfxBitstream::Data` 建议按 32 字节对齐；`MaxLength` 至少 `BufferSizeInKB * 1024`，再留 2 倍余量。【推断】

### 6.3 SPS/PPS 与头信息

- 默认情况下，编码器在第一个 IDR 前写入 SPS/PPS（H.264）或 VPS/SPS/PPS（HEVC），码流是 Annex-B 起始码格式；客户端解码器通常直接可用。【推断】[EncodeFrameAsync](https://intel.github.io/libvpl/latest/API_ref/VPL_func_vid_encode.html)
- 需要单独拿 SPS/PPS：`MFXVideoENCODE_GetVideoParam` 时附加 `mfxExtCodingOptionSPSPPS`，它会返回 SPS/PPS 的拷贝；注意缓冲区要足够大。【官方文档】[GetVideoParam](https://intel.github.io/libvpl/latest/API_ref/VPL_func_vid_encode.html) / [Intel 社区确认](https://community.intel.com/t5/Media-Intel-Video-Processing/find-a-API-which-can-get-sps-and-pps-data-before-encode-a-frame/m-p/1019568)
- 想让每个 I 帧都带头（客户端中途加入、丢包恢复）：`mfxExtCodingOption2.RepeatHeaders = MFX_CODINGOPTION_ON`。代价是每帧多几十字节。【官方文档】[Encode Structures / RepeatHeaders](https://intel.github.io/libvpl/latest/API_ref/VPL_structs_encode.html)
- 不需要手动拼 SPS/PPS 再塞进码流；让编码器 in-band 输出即可。除非客户端要求 AVCC/带外头。【推断】
- HEVC 的 `GopOptFlag = MFX_GOP_CLOSED | MFX_GOP_STRICT` 在 HEVC 上不被支持，不要依赖 closed GOP 保证随机接入。【官方文档】[2018 MSDK Release Notes](https://www.intel.com/content/dam/develop/external/us/en/documents/mediasdk-release-notes-2018r1-759513.pdf)

### 6.4 首帧出包时机

- 官方只保证“可能不出包，返回 `MFX_ERR_MORE_DATA`”；没有承诺第一帧就出。低延迟配置下，第一帧通常仍可能被编码器内部流水线缓存 1 帧左右，第二/第三次提交才拿到第一包。【推断】[EncodeFrameAsync](https://intel.github.io/libvpl/latest/API_ref/VPL_func_vid_encode.html)
- 实现上不要“提交第一帧就阻塞等包”；正确做法是持续喂帧，收到 `MFX_ERR_NONE` 就 sync 并发送，收到 `MORE_DATA` 就继续。【推断】
- 停止时用 NULL surface drain，确保最后一帧不丢。【官方文档】[EncodeFrameAsync](https://intel.github.io/libvpl/latest/API_ref/VPL_func_vid_encode.html)

### 6.5 时间戳

- `mfxFrameSurface1.Data.TimeStamp` 与 `mfxBitstream.TimeStamp` 的单位都是 **90 kHz**；除以 90000 得秒。`MFX_TIMESTAMP_UNKNOWN` 表示无时间戳。【官方文档】[mfxFrameData / mfxBitstream](https://intel.github.io/libvpl/latest/API_ref/VPL_structs_memory.html)
- 60 fps 每帧时间戳步进 `90000 / 60 = 1500`；用采集时刻的单调时钟换算，不要用“帧计数 × 1500”代替真实时间（会漂移）。【推断】
- 有 B 帧时 `DecodeTimeStamp` 与 `TimeStamp` 可能不同；`GopRefDist=1` 时二者通常一致。发送端应以 `TimeStamp` 为显示时间，`DecodeTimeStamp` 供解码器排序。【官方文档】[mfxBitstream 字段](https://intel.github.io/libvpl/latest/API_ref/VPL_structs_memory.html) / 【推断】无 B 帧时一致
- 用 Import 路径时，导入后的 surface 也要设置 `Data.TimeStamp` 再提交；不要依赖采集纹理自带的时间。【推断】

### 6.6 错误码处理表

| 返回码 | 含义 | 动作 |
|---|---|---|
| `MFX_ERR_NONE` | 有输出 | `SyncOperation`，读码流，发送 |
| `MFX_ERR_MORE_DATA` | 需要更多输入 | 继续喂下一帧；drain 时结束 |
| `MFX_WRN_DEVICE_BUSY` | GPU 忙 | Sync 或等几毫秒，重试同一帧 |
| `MFX_ERR_NOT_ENOUGH_BUFFER` | 码流缓冲不足 | 增大 `bs.MaxLength` / `BufferSizeInKB` |
| `MFX_ERR_DEVICE_LOST` | 设备丢失 | 销毁并重建 device/session/encoder |
| `MFX_WRN_INCOMPATIBLE_VIDEO_PARAM` | 参数被修正 | 检查 out 的实际值，记录并接受/降级 |
| `MFX_WRN_PARTIAL_ACCELERATION` | 部分加速 | 低延迟场景拒绝，换 profile/format |
| `MFX_ERR_INVALID_VIDEO_PARAM` | 参数非法 | 修正参数（对齐、profile、BRC 组合） |
| `MFX_ERR_UNSUPPORTED` | 不支持 | 降级 codec/profile/format |

【官方文档】[EncodeFrameAsync 返回码](https://intel.github.io/libvpl/latest/API_ref/VPL_func_vid_encode.html) / [Hardware Device Error Handling](https://intel.github.io/libvpl/latest/programming_guide/VPL_prg_err.html) / 【推断】恢复动作

---

## 7. 许可与分发

- libvpl dispatcher：**MIT**。可以随应用分发 `libvpl.dll`，并附 MIT 许可证文本。【官方文档】[libvpl README / License](https://github.com/intel/libvpl)
- Intel VPL GPU runtime：**MIT**（源码仓库），但 Windows 上它是显卡驱动的一部分，由驱动安装到 Driver Store；应用不应打包 `libmfx64-gen.dll`，否则会和驱动版本冲突。【官方文档】[vpl-gpu-rt README / License](https://github.com/intel/vpl-gpu-rt) / 【推断】分发策略
- Media SDK runtime/开发包：旧项目，停止更新；不要在新产品里依赖它，除非必须支持 6~10 代。【官方文档】[Media SDK 仓库](https://github.com/Intel-Media-SDK/MediaSDK) / [Upgrade Summary](https://www.intel.com/content/www/us/en/docs/onevpl/upgrade-from-msdk/2023-1/summary.html)
- Intel 显卡驱动本身受 Intel 驱动 EULA 约束；不要提取/再分发驱动组件。【推断】
- H.264/HEVC/AV1 的**内容/专利许可**不由 MIT 覆盖；产品分发前需要做专利许可评估（尤其 HEVC/AV1）。【推断，非法律意见】

**随包清单建议**

| 文件 | 是否随包 | 说明 |
|---|---|---|
| `libvpl.dll` | 是 | MIT；放 exe 同目录 |
| `libvpl.dll` 的 LICENSE | 是 | 合规 |
| `libmfx64-gen.dll` | 否 | 由驱动提供 |
| `libmfxhw64.dll` | 否 | 由驱动/旧包提供 |
| Intel 驱动 | 否 | 让用户自行安装/更新 |

---

## 8. 已知坑

1. **`AsyncDepth > 1` 固定加延迟。** 每多一帧约 `1/fps`；60 fps 下 2 就当头 +16.7 ms。远程桌面必须 1。【官方文档】[Appendix C](https://intel.github.io/libvpl/latest/appendix/VPL_apnds_c.html) / 【推断】队列换算
2. **`MFXInit` 会把你好心带回 1.x。** 它加载 Media SDK runtime，限制在 1.x API；Arc 上会丢掉 2.x 能力（包括 ImportFrameSurface）。【官方文档】[2021 Release Notes](https://www.intel.com/content/www/us/en/developer/articles/release-notes/oneapi-video-processing-library-release-notes-2021.html)
3. **B 帧/重排序延迟。** `GopRefDist > 1` 会缓存未来帧；远程桌面不要用。【官方文档】[GopRefDist](https://intel.github.io/libvpl/latest/API_ref/VPL_structs_cross_component.html)
4. **LookAhead / ExtBRC / pre-analysis 是延迟神器。** 全部关闭；不要为了“质量”打开。【官方文档】[ExtBRC](https://intel.github.io/libvpl/latest/programming_guide/VPL_prg_encoding.html)
5. **`BufferSizeInKB` / `InitialDelayInKB` 开大会造成几百毫秒延迟。** 按 5.2 的公式算；不要用“0.5 秒 CPB”这种直播默认值。【推断】
6. **`MaxFrameSize` / `MaxNumRePak` 会触发重打包。** 单帧超限时驱动反复编码/填充，延迟尖峰；低延迟不要用。【官方文档】[MaxFrameSize](https://intel.github.io/libvpl/latest/API_ref/VPL_structs_encode.html)
7. **4:4:4 / 4:2:2 / 10-bit 可能落到 shader/partial acceleration。** 看到 `MFX_WRN_PARTIAL_ACCELERATION` 就换 8-bit 4:2:0 或换 Arc，不要静默接受。【官方文档】[Query 返回码](https://intel.github.io/libvpl/latest/API_ref/VPL_func_vid_encode.html) / [Media Capabilities](https://cdrdv2-public.intel.com/773553/onevpl_developer-reference-media-intel-hardware_1.1-772607-773553.pdf)
8. **纹理数组/slice 不能随便喂。** 用单张 2D 纹理；数组先 `CopySubresourceRegion` 到单张。【推断】[Intel 社区](https://community.intel.com/t5/Media-Intel-oneAPI-Video/Using-D3D11Texture2D-to-encode-using-oneVPL/m-p/1488164)
9. **D3D11 没开 multithread protected。** 采集/转换/编码并发会崩；必须 `SetMultithreadProtected(TRUE)`。【官方文档】[HW Acceleration](https://intel.github.io/libvpl/latest/programming_guide/VPL_prg_hw.html)
10. **采集和编码不在同一适配器。** 跨适配器纹理共享要么失败，要么多一次拷贝；Optimus 笔记本优先核显采集+核显编码。【推断】[Switchable Graphics](https://intel.github.io/libvpl/latest/appendix/VPL_apnds_d.html)
11. **设备丢失不重建。** MUX 切换、驱动更新、TDR 都会 `MFX_ERR_DEVICE_LOST`；必须重建 device/session/encoder。【官方文档】[HW Acceleration / Device Lost](https://intel.github.io/libvpl/latest/programming_guide/VPL_prg_hw.html)
12. **老平台没有 VPL runtime。** 6~10 代（含 Ice Lake）只有 Media SDK 1.x；不要指望 `MFXLoad` 能给你 2.x 能力。【官方文档】[libvpl README](https://github.com/intel/libvpl)
13. **驱动太旧/没装 runtime。** `MFXEnumImplementations` 返回 0，`MFXCreateSession` 返回 `MFX_ERR_NOT_FOUND`；提示更新驱动，不要回退到软件编码还宣称低延迟。【推断】
14. **`MFX_WRN_DEVICE_BUSY` 丢帧。** 必须 sync/重试同一帧；丢帧会让 BRC 和画面都乱。【官方文档】[EncodeFrameAsync](https://intel.github.io/libvpl/latest/API_ref/VPL_func_vid_encode.html)
15. **码流缓冲太小。** `MaxLength < BufferSizeInKB` 会 `MFX_ERR_NOT_ENOUGH_BUFFER`；按 `BufferSizeInKB` 分配并留余量。【官方文档】[EncodeFrameAsync](https://intel.github.io/libvpl/latest/API_ref/VPL_func_vid_encode.html)
16. **`AsyncDepth=1` 吞吐下降。** 1080p60 通常没问题；4K/多路/多显示器要测；必要时只在“低延迟流”上用 1，其他流用 2+。【推断】
17. **HEVC closed GOP 标志不被支持。** 不要依赖 `MFX_GOP_CLOSED` 保证 HEVC 随机接入；用 IDR/RepeatHeaders/强制关键帧。【官方文档】[2018 MSDK Release Notes](https://www.intel.com/content/dam/develop/external/us/en/documents/mediasdk-release-notes-2018r1-759513.pdf)
18. **驱动默认值会变。** 同一份参数在不同驱动上可能被修正；每次启动都 Query + 记录 out，不要硬编码“驱动一定接受”。【推断】
19. **`MFX_WRN_INCOMPATIBLE_VIDEO_PARAM` 不是成功。** 它表示参数被静默修正；必须比对 out，不能直接拿 in 继续用。【官方文档】[Query](https://intel.github.io/libvpl/latest/API_ref/VPL_func_vid_encode.html)
20. **Import 可能退化成 copy。** `IMPORT_SHARED | IMPORT_COPY` 允许拷贝兜底；如果 p99 要求极严，要确认实际走的是 shared（用 GPUView/ETW 或时间戳测量）。【官方文档】[Shared Surfaces](https://intel.github.io/libvpl/latest/programming_guide/VPL_prg_surface_sharing.html) / 【推断】验证方法

---

## 9. 最小可用实现路径

### 9.1 步骤

**阶段 1：枚举与选择**

1. 枚举 DXGI 适配器，找 VendorID=0x8086 的 Intel 适配器；记录 `DXGIAdapterIndex`、`MediaAdapterType`、LUID。
2. 用 `MFXLoad` + 过滤属性：
   - `mfxImplDescription.Impl = MFX_IMPL_TYPE_HARDWARE`
   - `mfxImplDescription.AccelerationMode = MFX_ACCEL_MODE_VIA_D3D11`
   - `DXGIAdapterIndex = 选中的下标`
   - `...encoder.CodecID = MFX_CODEC_AVC`
3. `MFXEnumImplementations` 打印每个实现的 `ApiVersion`、`MediaAdapterType`、`DeviceID`；选 `ApiVersion` 最高的匹配项。

**阶段 2：会话与设备**

4. `MFXCreateSession(loader, index, &session)`；`MFXQueryVersion`。
5. 用同一个 DXGI 适配器创建 D3D11 设备：`D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT`；开启 multithread protected。
6. `MFXVideoCORE_SetHandle(session, MFX_HANDLE_D3D11_DEVICE, device)`。

**阶段 3：参数与初始化**

7. 填 `mfxVideoParam`：
   - `mfx.CodecId = MFX_CODEC_AVC`
   - `mfx.CodecProfile = MFX_PROFILE_AVC_HIGH`
   - `mfx.CodecLevel = MFX_LEVEL_UNKNOWN`（或 4.2）
   - `mfx.FrameInfo.FourCC = MFX_FOURCC_NV12`
   - `Width=1920, Height=1080, CropW=1920, CropH=1080`
   - `FrameRateExtN=60, FrameRateExtD=1`
   - `IOPattern = MFX_IOPATTERN_IN_VIDEO_MEMORY`
   - `AsyncDepth = 1`
   - `mfx.RateControlMethod = MFX_RATECONTROL_CBR`
   - `TargetKbps = 10000`, `MaxKbps = 10000`
   - `BufferSizeInKB = 150`, `InitialDelayInKB = 0`
   - `GopPicSize = 60`, `GopRefDist = 1`, `IdrInterval = 0`, `NumRefFrame = 1`
   - `TargetUsage = 4`
8. 附加扩展缓冲：
   - `mfxExtCodingOption`：`MaxDecFrameBuffering = 1`，`NalHrdConformance = OFF`（局域网），`VuiNalHrdParameters = OFF`
   - `mfxExtCodingOption2`：`RepeatHeaders = ON`，`ExtBRC = OFF`，`LookAheadDepth = 0`，`MaxFrameSize = 0`
   - `mfxExtCodingOption3`：`ScenarioInfo = MFX_SCENARIO_DISPLAY_REMOTING`，`ContentInfo = MFX_CONTENT_NON_VIDEO_SCREEN`，`LowDelayBRC = OFF`（CBR 下无效）
9. `MFXVideoENCODE_Query(session, &in, &out)`；检查返回码、out 的每个字段、警告。
10. `MFXVideoENCODE_QueryIOSurf(session, &out, &req)`；按 `req.NumFrameSuggested` 准备 surface 环（Import 路径也要按这个数量准备输入纹理）。
11. `MFXVideoENCODE_Init(session, &out)`。

**阶段 4：出包**

12. `MFXVideoENCODE_GetVideoParam` + `mfxExtCodingOptionSPSPPS`，缓存 SPS/PPS 供客户端中途加入。
13. 循环：
    - 捕获 BGRA8 纹理；
    - GPU 转换到 NV12 纹理；
    - 走路径 A 或 B 包装 surface；
    - `Data.TimeStamp += 1500`（60fps，90kHz）；
    - `MFXVideoENCODE_EncodeFrameAsync`；
    - `MFX_ERR_NONE` → `MFXVideoCORE_SyncOperation(syncp, MFX_INFINITE)` → 读码流 → 发送；
    - `MFX_ERR_MORE_DATA` → 继续；
    - `MFX_WRN_DEVICE_BUSY` → sync 后重试同一帧。
14. 停止：`EncodeFrameAsync(NULL surface)` drain → `MFXVideoENCODE_Close` → `MFXClose` → `MFXUnload`。

### 9.2 伪代码（只表达调用顺序，不是项目实现）

```c
// 1) 选择 Intel 适配器
IDXGIAdapter1 *adapter = pick_intel_adapter(&adapter_index);
ID3D11Device *dev = create_d3d11_device(adapter);   // BGRA_SUPPORT | VIDEO_SUPPORT
enable_multithread_protect(dev);

// 2) 创建 loader 与 session
mfxLoader loader = MFXLoad();
for each filter in { Impl=HW, AccelMode=D3D11, DXGIAdapterIndex=adapter_index, CodecID=AVC } {
    mfxConfig cfg = MFXCreateConfig(loader);
    MFXSetConfigFilterProperty(cfg, name, &value);   // 一个 cfg 只挂一个属性
}
mfxU32 n = 0;
while (MFXEnumImplementations(loader, n, MFX_IMPL_CAPS_IMPL_DESC, &desc) == MFX_ERR_NONE) {
    log(desc->ApiVersion, desc->mfxDeviceDescription.device.MediaAdapterType);
    n++;
}
mfxSession session;
MFXCreateSession(loader, chosen_index, &session);
MFXQueryVersion(session, &ver);
MFXVideoCORE_SetHandle(session, MFX_HANDLE_D3D11_DEVICE, dev);

// 3) 参数 + Query + Init
mfxVideoParam in = low_latency_h264_1080p60_cbr();  // 见 9.1 第 7~8 步
mfxVideoParam out = in;
mfxStatus sts = MFXVideoENCODE_Query(session, &in, &out);
if (sts == MFX_ERR_UNSUPPORTED || sts == MFX_WRN_PARTIAL_ACCELERATION) { fallback(); }
mfxFrameAllocRequest req;
MFXVideoENCODE_QueryIOSurf(session, &out, &req);
MFXVideoENCODE_Init(session, &out);

// 4) 取 SPS/PPS
mfxExtCodingOptionSPSPPS spspps = {};
mfxVideoParam got = out; attach(&got, &spspps);
MFXVideoENCODE_GetVideoParam(session, &got);
cache_headers(spspps.SPSBuffer, spspps.PPSBuffer);

// 5) 每帧
for each captured_frame {
    nv12 = convert_bgra_to_nv12_gpu(captured_frame);
    mfxFrameSurface1 *surf = import_or_wrap(nv12);      // 路径 A 或 B
    surf->Data.TimeStamp = next_90khz_timestamp();
    mfxBitstream bs = alloc_bitstream(/* >= BufferSizeInKB * 1024 */);
    mfxSyncPoint sync = NULL;
    do {
        sts = MFXVideoENCODE_EncodeFrameAsync(session, NULL, surf, &bs, &sync);
    } while (sts == MFX_WRN_DEVICE_BUSY);
    if (sts == MFX_ERR_NONE) {
        MFXVideoCORE_SyncOperation(session, sync, MFX_INFINITE);
        send(bs.Data + bs.DataOffset, bs.DataLength);
        reset_bitstream(&bs);
    }
    release_surface(surf);
}

// 6) drain
while (MFXVideoENCODE_EncodeFrameAsync(session, NULL, NULL, &bs, &sync) == MFX_ERR_NONE) {
    MFXVideoCORE_SyncOperation(session, sync, MFX_INFINITE);
    send(bs.Data + bs.DataOffset, bs.DataLength);
}
MFXVideoENCODE_Close(session);
MFXClose(session);
MFXUnload(loader);
```

【推断，按官方 API 组合】，关键调用与字段见 [VideoENCODE](https://intel.github.io/libvpl/latest/API_ref/VPL_func_vid_encode.html) / [Shared Surfaces](https://intel.github.io/libvpl/latest/programming_guide/VPL_prg_surface_sharing.html)

### 9.3 验收判据（先定义再测试）

| 判据 | 通过条件 | 证据 |
|---|---|---|
| 枚举 | 至少 1 个硬件实现，`ApiVersion` 记录到日志 | 启动日志 |
| 初始化 | Query 无 `UNSUPPORTED`/`PARTIAL_ACCELERATION`；out 参数未被关键字段修正 | 初始化日志 |
| 第一包 | 启动后 3 帧内出现第一个 `MFX_ERR_NONE` | 时间戳日志 |
| SPS/PPS | 能拿到非空 SPS/PPS；客户端可直接解码 | 头缓存日志 |
| 稳定性 | 连续 10 分钟 1080p60 无设备丢失/无丢帧 | 运行日志 |
| 延迟 | 编码 submit→sync p99 ≤ 8 ms（核显/Arc 目标） | 埋点 |
| 玻璃到玻璃 | p99 ≤ 30 ms | 端到端埋点 |
| 码率 | 实测码率在目标 ±10%，无持续超峰 | 码率统计 |

【推断】判据阈值需按目标机器校准；测试前先写进测试用例，不允许事后改。

---

## 10. 与 AMF 的参数对应表

> AMF 属性名以所用 AMF 版本头文件为准；下表按“语义”映射。AMF 时间戳单位 100 ns，oneVPL 是 90 kHz。

| 语义 | AMF（AMD） | oneVPL（Intel） | 换算 / 备注 |
|---|---|---|---|
| 码控模式 | `AMF_VIDEO_ENCODER_RATE_CONTROL_METHOD`：CBR / VBR_PEAK / CQP | `mfxInfoMFX.RateControlMethod`：`MFX_RATECONTROL_CBR` / `VBR` / `CQP` | 语义相近，非一一对应 |
| 目标码率 | `AMF_VIDEO_ENCODER_TARGET_BITRATE`（bps） | `TargetKbps`（kbps） | `TargetKbps = bps / 1000` |
| 峰值码率 | `AMF_VIDEO_ENCODER_PEAK_BITRATE`（bps） | `MaxKbps`（kbps） | CBR 下 VPL 忽略 `MaxKbps` |
| VBV/CPB 大小 | `AMF_VIDEO_ENCODER_VBV_BUFFER_SIZE`（bits） | `BufferSizeInKB`（KB） | `KB = bits / 8 / 1024` |
| 初始缓冲 | `AMF_VIDEO_ENCODER_INITIAL_VBV_BUFFER_FULLNESS`（0~1） | `InitialDelayInKB`（KB） | `InitialDelayInKB = fullness × BufferSizeInKB` |
| 低延迟模式 | `AMF_VIDEO_ENCODER_USAGE` = `LOW_LATENCY` / `ULTRA_LOW_LATENCY`；`LOWLATENCY_MODE` | `AsyncDepth = 1` + 小 `BufferSizeInKB` + `LowDelayBRC`（VBR 时） | VPL 没有单一“ULL”开关 |
| 质量/速度 | `AMF_VIDEO_ENCODER_QUALITY_PRESET`：SPEED / BALANCED / QUALITY | `mfxInfoMFX.TargetUsage`：7 / 4 / 1 | 近似映射，需实测 |
| B 帧 | `AMF_VIDEO_ENCODER_B_PIC_PATTERN = 0`；`MAX_CONSECUTIVE_BPICTURES = 0` | `GopRefDist = 1`（可选 `GPB = ON`） | 都不要 B 重排序 |
| GOP / IDR | `AMF_VIDEO_ENCODER_IDR_PERIOD`（帧） | `GopPicSize` + `IdrInterval = 0` | 单位都是帧 |
| 参考帧 | `AMF_VIDEO_ENCODER_MAX_NUM_REFRAMES` | `NumRefFrame` | 低延迟都设 1 |
| 预分析 | `AMF_VIDEO_ENCODER_PRE_ANALYSIS = false` | `ExtBRC = OFF`，`LookAheadDepth = 0` | 都关 |
| 重复头 | `AMF_VIDEO_ENCODER_HEADER_INSERTION_SPACING`（按版本） | `mfxExtCodingOption2.RepeatHeaders = ON` | AMF 属性名随版本变化 |
| 强制 IDR | `AMF_VIDEO_ENCODER_FORCE_PICTURE_TYPE = IDR` | `mfxEncodeCtrl.FrameType = I | IDR | REF` | 丢包/加入时用 |
| 输入纹理 | `AMFContext::CreateSurfaceFromDX11Native` / AMF DX11 surface | `ImportFrameSurface(mfxSurfaceD3D11Tex2D)` 或 `Data.MemId` | 都在 GPU 内 |
| 同步 | `SubmitInput` + `QueryOutput` 轮询 | `AsyncDepth=1` + `MFXVideoCORE_SyncOperation` | AMF 无 AsyncDepth 字段 |
| 时间戳 | AMF PTS，100 ns | `Data.TimeStamp` / `Bitstream.TimeStamp`，90 kHz | `ts90 = ts100ns × 9 / 1000` |
| 输入格式 | NV12 / P010 | `MFX_FOURCC_NV12` / `MFX_FOURCC_P010` | 相同 |
| 编码器对象 | AMF context + encoder component | VPL session + `MFXVideoENCODE_*` | 生命周期不同 |

【推断，按两套 API 的语义映射】[AMF 参数语义](https://github.com/GPUOpen-LibrariesAndSDKs/AMF) / [mfxInfoMFX](https://intel.github.io/libvpl/latest/API_ref/VPL_structs_cross_component.html)

---

## 11. 未确认 / 待实测清单

1. **Meteor Lake / Lunar Lake / Arrow Lake 核显的 AV1 编码与 4:4:4 能力**：旧能力表未覆盖；需用 `MFXVideoENCODE_Query` 实测。【未确认】
2. **Arc B（Battlemage）的 HEVC 4:2:2 / 4:4:4 与 AV1 具体 profile**：营销资料与旧表不完全一致；需实测。【未确认】
3. **`ImportFrameSurface` + VPL VPP 直接吃 BGRA 纹理**：VPP 是否接受 Import 的 BGRA、是否零拷贝，需实测。【未确认】
4. **跨适配器共享纹理 Import**：核显采集 + Arc 编码时，`IMPORT_SHARED` 是否支持跨适配器 NT 共享句柄，需实测。【未确认】
5. **Texture2DArray slice 直入**：官方文档没有明确“支持数组 slice”；社区有失败案例，建议单张纹理。【未确认】
6. **低功耗编码器（VDEnc）开关**：oneVPL 公开文档没有稳定的“低延迟/低功耗”布尔开关；由驱动/TargetUsage 决定，需用 GPUView/ETW 测。【未确认】
7. **各代平台的最低驱动版本**：30.0.100.9805 是 2021 年的分界；Arc/新核显的具体最低版本需按机型验证。【未确认】
8. **第一包的确切帧数**：官方只说“可能 MORE_DATA”；不同驱动/参数下是 1 帧还是 2 帧，需实测。【未确认】
9. **p99 30 ms 是否可达**：取决于捕获、转换、编码、网络、客户端解码；本备忘给的是预算，不是承诺。【未确认】

---

## 12. 参考链接

**Intel 官方**

- [Intel VPL 概览](https://www.intel.com/content/www/us/en/developer/tools/vpl/overview.html)
- [libvpl（dispatcher，MIT）](https://github.com/intel/libvpl)
- [vpl-gpu-rt（GPU runtime，MIT）](https://github.com/intel/vpl-gpu-rt)
- [Intel Media SDK（已停止更新）](https://github.com/Intel-Media-SDK/MediaSDK)
- [oneVPL Upgrade Summary（2023-1）](https://www.intel.com/content/www/us/en/docs/onevpl/upgrade-from-msdk/2023-1/summary.html)
- [oneVPL 2021 Release Notes](https://www.intel.com/content/www/us/en/developer/articles/release-notes/oneapi-video-processing-library-release-notes-2021.html)
- [oneVPL Installation Guide](https://www.intel.com/content/www/us/en/developer/articles/guide/onevpl-installation-guide.html)
- [VPL Session / Dispatcher](https://intel.github.io/libvpl/latest/programming_guide/VPL_prg_session.html)
- [Hardware Acceleration](https://intel.github.io/libvpl/latest/programming_guide/VPL_prg_hw.html)
- [Importing and Exporting Shared Surfaces](https://intel.github.io/libvpl/latest/programming_guide/VPL_prg_surface_sharing.html)
- [Memory Structures](https://intel.github.io/libvpl/latest/API_ref/VPL_structs_memory.html)
- [VideoENCODE Functions](https://intel.github.io/libvpl/latest/API_ref/VPL_func_vid_encode.html)
- [Encode Structures](https://intel.github.io/libvpl/latest/API_ref/VPL_structs_encode.html)
- [Cross-component Structures（mfxInfoMFX / mfxVideoParam）](https://intel.github.io/libvpl/latest/API_ref/VPL_structs_cross_component.html)
- [Appendix C：Streaming and Video Conferencing Features](https://intel.github.io/libvpl/latest/appendix/VPL_apnds_c.html)
- [Appendix D：Switchable Graphics and Multiple Monitors](https://intel.github.io/libvpl/latest/appendix/VPL_apnds_d.html)
- [Media Capabilities Supported by Intel Hardware（PDF）](https://cdrdv2-public.intel.com/773553/onevpl_developer-reference-media-intel-hardware_1.1-772607-773553.pdf)
- [Low Latency Video Encoding and Decoding by Intel Media SDK（PDF）](https://cdrdv2-public.intel.com/681829/681829-1.0.pdf)
- [2017 Media SDK Release Notes（LowDelayBRC）](https://www.intel.com/content/dam/develop/external/us/en/documents/mediasdk-release-notes-2017-759513.pdf)
- [2018 Media SDK Release Notes（HEVC GOP 限制）](https://www.intel.com/content/dam/develop/external/us/en/documents/mediasdk-release-notes-2018r1-759513.pdf)
- [驱动 101.4125 Release Notes（含 oneVPL GPU Runtime）](https://downloadmirror.intel.com/768896/ReleaseNotes_101.4125_BETA.pdf)
- [驱动 101.7085 Release Notes（含 oneVPL GPU Runtime）](https://downloadmirror.intel.com/915569/ReleaseNotes_101.7085.pdf)

**微软**

- [Desktop Duplication API](https://learn.microsoft.com/en-us/windows/win32/api/dxgi1_2/nn-dxgi1_2-idxg1outputduplication)
- [D3D11CreateDevice](https://learn.microsoft.com/en-us/windows/win32/api/d3d11/nf-d3d11-d3d11createdevice)
- [ID3D11DeviceContext::CopyResource](https://learn.microsoft.com/en-us/windows/win32/api/d3d11/nf-d3d11-id3d11devicecontext-copyresource)
- [ID3D11VideoProcessor](https://learn.microsoft.com/en-us/windows/win32/api/d3d11/nn-d3d11-id3d11videoprocessor)

**参考实现 / 社区**

- [FFmpeg QSV 编码器文档](https://ffmpeg.org/doxygen/trunk/qsvenc_8h.html)
- [FFmpeg pixfmt 中 QSV/VAAPI 的 MemId 说明](https://ffmpeg.org/doxygen/trunk/pixfmt_8h.html)
- [VLC qsv.c（IdrInterval 语义）](https://sources.debian.org/src/vlc/3.0.22-0+deb12u1/modules/codec/qsv.c)
- [Intel 社区：D3D11Texture2D 编码](https://community.intel.com/t5/Media-Intel-oneAPI-Video/Using-D3D11Texture2D-to-encode-using-oneVPL/m-p/1488164)
- [Intel 社区：SPS/PPS 获取](https://community.intel.com/t5/Media-Intel-Video-Processing/find-a-API-which-can-get-sps-and-pps-data-before-encode-a-frame/m-p/1019568)
