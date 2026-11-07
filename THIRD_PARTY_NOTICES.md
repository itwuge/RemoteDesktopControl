# 第三方声明（THIRD PARTY NOTICES）

本文件随发行产物分发，用于说明 RDC 使用的第三方组件及其许可。本仓库自身代码以
根目录 `LICENSE`（MIT）授权。依赖的精确版本以 `Cargo.lock` 与 `app/pubspec.lock` 为准。

## Rust 依赖（运行时）

| 组件 | 许可 | 用途 |
|---|---|---|
| serde / serde_json | MIT OR Apache-2.0 | 协议与配置序列化 |
| thiserror | MIT OR Apache-2.0 | 分层错误类型 |
| tokio | MIT | 异步运行时 |
| tracing | MIT | 日志门面 |
| flutter_rust_bridge | MIT | Dart ↔ Rust 桥接 |
| x25519-dalek / ed25519-dalek | BSD-3-Clause | 密钥交换与身份签名 |
| curve25519-dalek | BSD-3-Clause | 上述 dalek 组件的底层实现 |
| hkdf / sha2 / hmac | MIT OR Apache-2.0 | 密钥派生与摘要 |
| aes-gcm / chacha20poly1305 | MIT OR Apache-2.0 | 会话加密 |
| spake2 | MIT OR Apache-2.0 | 配对码 PAKE |
| base64 | MIT OR Apache-2.0 | 密钥/签名文本编码 |
| rand_core | MIT OR Apache-2.0 | 随机源 |
| subtle | BSD-3-Clause | 常量时间比较 |
| zeroize | MIT OR Apache-2.0 | 内存擦除 |
| quinn | MIT OR Apache-2.0 | QUIC 传输 |
| rustls | MIT OR Apache-2.0 | TLS 1.3 |
| ring | ISC 风格许可（含 OpenSSL 例外条款） | rustls 的密码学后端 |
| rcgen | MIT OR Apache-2.0 | 自签证书生成 |
| bytes / futures | MIT | 传输缓冲区与异步工具 |
| windows（Microsoft 官方绑定） | MIT OR Apache-2.0 | Windows 平台 API |

## Dart / Flutter 依赖

| 组件 | 许可 | 用途 |
|---|---|---|
| Flutter Engine / Flutter Framework | BSD-3-Clause | 桌面应用框架 |
| flutter_rust_bridge（Dart 侧） | MIT | 桥接运行时 |
| cupertino_icons | MIT | 图标资源 |

## 微软 VC++ 运行库

发行包内含 `msvcp140.dll`、`vcruntime140.dll`、`vcruntime140_1.dll`（app-local 部署），
按 Microsoft Visual Studio 可再发行组件许可随应用分发；这些文件不属于本仓库源码。

## FFmpeg（LGPL-2.1-or-later，动态链接）

发行包内含自建的精简 FFmpeg 共享库 `avcodec-*.dll` 与 `avutil-*.dll`（n7.1），仅启用
H.264 解码 / H.264 parser / d3d11va 硬解与 file 协议。构建参数：
`--disable-everything --enable-decoder=h264 --enable-parser=h264 --enable-protocol=file
--disable-gpl --disable-libx264 --disable-libx265 --enable-d3d11va
--enable-hwaccel=h264_d3d11va --enable-hwaccel=h264_d3d11va2
--enable-shared --disable-static --disable-programs --disable-doc --disable-network --disable-autodetect`。

- 许可：**LGPL-2.1-or-later**（未启用 GPL 组件，不含 x264/x265/avformat/avfilter/swscale）。
- 链接方式：动态链接，DLL 未修改；使用者可按 LGPL 2.1 第 6 节替换为兼容版本。
- 许可文本随包：`COPYING.LGPLv2.1`、`FFmpeg-LICENSE.md`；构建参数与 SHA-256 记录在
  `FFMPEG-BUILD-INFO.txt`。
- 运行时还带 `libwinpthread-1.dll`（mingw-w64 winpthreads，MIT 风格许可，文本见
  `libwinpthread-COPYING.txt`），它是 `avutil-*.dll` 的直接依赖。
- 源码获取：<https://git.ffmpeg.org/ffmpeg.git>（tag `n7.1`），或向分发者索取对应源码包。

若以未启用 `ffmpeg` feature 的配置构建，发行包不含任何 FFmpeg DLL，此时运行时只能使用
Media Foundation 解码（回退路径）。

## 许可文本获取

以上各组件的完整许可文本随其上游发行物提供（crates.io / pub.dev / 各自项目仓库）。
如需离线归档，请以 `Cargo.lock`、`app/pubspec.lock` 的版本号从上游获取对应文本。

## AMD AMF SDK（rdc-amf.dll）

- 组件：`crates/rdc-codec/shim/`（C++ 薄 shim），编译产物 `rdc-amf.dll`。
- 源码许可：AMD Advanced Media Framework SDK，MIT License，
  Copyright (c) 2018 Advanced Micro Devices, Inc. All rights reserved.
- 分发方式：本仓库不分发 AMF 头文件；运行期 `amfrt64.dll` 由显卡驱动安装提供，
  通过 `LoadLibrary("amfrt64.dll")` + `AMFInit` 动态加载，不随安装包分发。
- 未引入 x264 / x265 / GPL 组件。

## NVIDIA NVENC（rdc-nvenc.dll，Phase 1 骨架）

- 组件：`crates/rdc-codec/shim/nvenc_enc.cpp`，编译产物 `rdc-nvenc.dll`（Phase 1 仅做
  `nvEncodeAPI64.dll` 加载与版本探测，不创建编码会话）。
- 源码许可：Phase 1 shim 不引入 NVENC SDK 头文件，仅按 NVIDIA `nv-codec-headers`
  （MIT License）的 `NVENCAPI_VERSION` 编码规则引用 12.2 版本号；后续若引入头文件，
  其 MIT 许可文本需随包附上。
- 分发方式：系统驱动 DLL（`nvEncodeAPI64.dll` / `nvcuda.dll`）由 NVIDIA 显卡驱动提供，
  运行期 `LoadLibrary("nvEncodeAPI64.dll")` 动态加载，**禁止随安装包分发**。
- 当前状态：会话未实装，NVENC 真启用/1080p60/p99/掉帧/10 分钟稳定性/并发会话上限/
  颜色通道一律「未验证」，须在 N 卡实机按 `logs/PLAN-NVENC-20261108.md` 验收。

MIT License

Permission is hereby granted, free of charge, to any person obtaining a copy of this software
and associated documentation files (the "Software"), to deal in the Software without restriction,
including without limitation the rights to use, copy, modify, merge, publish, distribute,
sublicense, and/or sell copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or
substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED, INCLUDING
BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND
NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM,
DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE SOFTWARE.
