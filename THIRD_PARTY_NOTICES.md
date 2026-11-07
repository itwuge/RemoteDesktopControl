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

## FFmpeg（默认不包含）

`rdc-decode` 的 FFmpeg 解码后端由 Cargo feature `ffmpeg` 控制，**默认关闭**，发行包
不包含任何 FFmpeg DLL。若使用者自行以该 feature 构建，需自行提供 LGPL 共享构建的
FFmpeg 动态库并遵守 LGPL-2.1-or-later；本仓库不分发该二进制。

## 许可文本获取

以上各组件的完整许可文本随其上游发行物提供（crates.io / pub.dev / 各自项目仓库）。
如需离线归档，请以 `Cargo.lock`、`app/pubspec.lock` 的版本号从上游获取对应文本。
