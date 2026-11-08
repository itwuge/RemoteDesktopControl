# RDC 远程桌面控制

Windows 桌面远程控制工具（预发布版 0.2.0）。Rust 核心 + Flutter 桌面端，画面走自建
QUIC 通道，授权用离线可校验的 `grant_token`，支持多账号共享控制、接管与踢出。

## 三步跑起来

1. **拿到产物**：安装 `dist/RDC-0.2.0-windows-x64-setup.exe`，或解压
   `dist/RDC-0.2.0-windows-portable.zip` 后直接运行 `rdc.exe`（无需安装）。
   产物带同名 `.sha256`，可用 `Get-FileHash <文件> -Algorithm SHA256` 核对。
   安装版固定装到 `C:\Program Files\RDC`，配置与日志在 `C:\Program Files\RDC\config`。
2. **启动**：双击 `rdc.exe`。安装版首次启动会在安装目录的 `config` 下生成本机身份密钥与配置
   （便携版为 `%APPDATA%\RDC`）。
   主界面左侧是设备、屏幕墙、控制、权限四页。
3. **连一台机器**：对方也在「设备」页可见后，在「权限」页给对方签发授权
   （只观看 / 可操作 / 完全控制）；未信任设备需要配对码，已加入信任表的设备走免密
   密钥认证。授权、接管、踢出都会写入审计（安装版 `config\s5-audit.jsonl`，
   便携版 `%APPDATA%\RDC\s5-audit.jsonl`）。

命令行自检（不启动界面）：

```powershell
rdc-cli.exe selftest      # 版本 / 平台 / 协议版本
rdc-cli.exe s5-5          # 授权、接管、踢出、免密的可判定验证
```

## 已知限制（0.1.0）

- **仅 Windows 10/11 x64**。macOS / Linux / Android / iOS 目前只有 Flutter 工程与打包
  脚本骨架，采集与注入后端是待实现的空壳（调用即 `todo!()`），无法建立可用会话。
- **无代码签名证书**。SmartScreen 首次运行会提示「未知发布者」；安装包与 exe 未签名。
- **跨机联调未闭环**。本机回环下的画面链路、输入注入、授权/接管/踢出已自测通过；
  NAT/防火墙下的真实组网仍需手工配置，没有中继服务与自动打洞。
- **锁屏下未做点击级验证**。屏幕墙预览、控制页操作、权限页对话框的 UI 交互需解锁桌面
  后人工复验，自动化只覆盖到内核与数据链路。
- **无自动更新、无崩溃上报、无账号云同步**。账号服务不可达时按本地信任表回退，账号不
  参与端到端加密，授权令牌不进信令包头。
- **FFmpeg 解码为可选组件**：构建机存在 LGPL 共享构建（`FFMPEG_DIR` 或
  `C:\dev\ffmpeg-out`）时产物默认携带 avcodec/avutil；否则仅走 MF 解码。

## 目录说明

| 目录 | 内容 |
|---|---|
| `crates/rdc-core` | 会话编排、授权内核 `s5/`、审计、设置 |
| `crates/rdc-capture` / `rdc-input` | 平台采集与输入注入（Windows 已实现，其余平台空壳） |
| `crates/rdc-transport` / `rdc-protocol` | QUIC 通道、信令与协议类型 |
| `crates/rdc-security` | Ed25519 身份、签名与密钥 |
| `crates/rdc-bridge` / `rdc-cli` | Flutter 桥接层、命令行验证入口 |
| `app/` | Flutter 桌面端（`lib/view` 页面、`lib/controller` 状态、`lib/core` 本地化） |
| `packaging/` | Windows NSIS / 便携 zip，macOS dmg，Linux AppImage/deb，Android APK，iOS xcarchive |
| `dist/` | 打包产物与 sha256（不入库） |
| `docs/` | 设计与规范文档（编号 01–13），含决策待办 |
| `logs/` | 验收判据与实测证据（不入库） |
| `benches/` | 性能基线与脚本 |
| `.github/` | CI 六 job（rust / windows / macos / linux / android / compliance） |

## 构建（Windows）

```powershell
# 前置：Rust (MSVC toolchain)、Flutter 3.47.x、NSIS（可选，缺失时只出便携 zip）
.\packaging\windows\build_installer.ps1
```

产物在 `dist/`：`RDC-<版本>-windows-x64-setup.exe` 与 `RDC-<版本>-windows-portable.zip`，
各自带 `.sha256`；`BUILD_INFO.txt` 记录版本与 commit。

## 许可证

本项目以 MIT 许可发布，见 `LICENSE`。第三方组件与 VC++ 运行库说明见
`THIRD_PARTY_NOTICES.md`，发行包内附两份文件。
