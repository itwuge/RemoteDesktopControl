# RDC 整机迁移交接

> 用途：把本项目从一台 Windows 机器搬到另一台，做到「拷过去就能开工」。
> 适用范围：Windows 10/11 x64；Linux/Android 线只恢复代码，不保证在本机交叉编译。
> 最近校验：2026-11-08，见文末「本次实测记录」。

## 0. 迁移包里有什么

包目录：`E:\rdc-migrate\`（不要放 C 盘，避免系统盘清理时丢文件）

| 内容 | 路径 | 说明 |
|---|---|---|
| 全部代码与历史 | `rdc-all.bundle` | `git bundle --all`，含 8 条分支 + 1 个标签，可直接 clone |
| 工程经验库 | `工程经验库\` | 铁律、坑库、开工清单、门禁模板；本身是独立 git 仓库（含 `.git`） |
| 记忆 | `memory\user_profile.md`、`memory\projects\<项目记忆目录>\` | 原样拷贝，未加工 |
| 交接文档 | 本文件 | 恢复、构建、验收、已知坑 |

**没有随包带走的东西**（新机器上补不回来，只能重新产生）：

| 缺失项 | 影响 | 怎么补 |
|---|---|---|
| 对话历史 / 会话上下文 | 无法追溯当时的讨论细节 | 只有本文件与 `logs/` 下的判据、`docs/决策待办.md` 可查 |
| ToDesk 与虚拟显示器驱动 | 锁屏/无显示器时采集到黑屏或白屏 | 新机器装 ToDesk 并启用虚拟显示器（见 §7） |
| 代码签名证书与私钥 | Release/安装包无法做 Authenticode 签名 | 证书不迁移（安全约束）；需重新申请或导入受控硬件密钥 |
| 自更新发布私钥 | 无法为新版本签 `update-manifest.json` | 见 `docs/` 与 `packaging/windows/make_update_manifest.ps1`；私钥只在 CI Secret `RDC_RELEASE_SEED_B64` |
| FFmpeg LGPL 共享构建 | 出包退回「仅 MF 解码」 | 可选：自建到 `C:\dev\ffmpeg-out`（`build_installer.ps1` 会显式警告） |
| AMF SDK 头文件 + MinGW | 包内无 `rdc-amf.dll`，只能用 MFT 软编 | 可选：见 `crates/rdc-codec/shim/build_amf.ps1` |
| 本机驱动状态、注册表象 | 与采集/编码相关的能力差异 | 按 §7 的顺序重装显卡驱动 |

## 1. 恢复代码（bundle → 多 worktree）

在**新机器**上按顺序执行（路径按需替换，以下用 `D:\work` 作为新根目录）：

```powershell
# 1) 从 bundle 克隆。它自带全部历史，clone 后只有一个本地分支（bundle 的 HEAD）。
mkdir D:\work
git clone E:\rdc-migrate\rdc-all.bundle D:\work\RDC

# 2) 验证完整性：应看到 8 条分支与完整历史。
cd D:\work\RDC
git bundle verify E:\rdc-migrate\rdc-all.bundle   # 末行 "is okay"
git branch -a                                     # origin/* 下应有 8 条

# 3) 把 origin/* 落成本地分支（bundle clone 不会自动建）。
foreach ($b in @("master","linux-x11","android-client","ui-polish","fix/rdr-fallback-window","docs/requirements","observability","feature/auto-update")) {
    git branch $b "origin/$b"
}
git branch                                        # 期望 8 条本地分支

# 4) 按项目惯例重建 worktree：一条线一个目录、一条分支。
git checkout master
git worktree add ..\RDC-linux          linux-x11
git worktree add ..\RDC-android        android-client
git worktree add ..\RDC-ui             ui-polish
git worktree add ..\RDC-render-fallback fix/rdr-fallback-window
git worktree add ..\RDC-docs           docs/requirements
git worktree add ..\RDC-observability  observability
git worktree add ..\RDC-release        feature/auto-update
git worktree list                                 # 核对 8 个目录

# 5) 以后接 GitHub：bundle 的 origin 指向本地 bundle 文件，接入远端时改名保留。
git remote rename origin bundle
git remote add origin https://github.com/<账号>/<仓库>.git
git push -u origin --all
git push origin --tags
```

> 注意：`git worktree add` 之前先 `git checkout master`，否则 master 被占用会报 “already checked out”。

## 2. 工具链（一键脚本）

```powershell
# 先自检：只打印计划，不下载不安装。
powershell -NoProfile -ExecutionPolicy Bypass -File .\packaging\dev\setup-dev.ps1 -WhatIf

# 正式执行（幂等，可重复跑）。
powershell -NoProfile -ExecutionPolicy Bypass -File .\packaging\dev\setup-dev.ps1
```

脚本固定版本（与 CI `.github/workflows/release-windows.yml` 一致）：

| 组件 | 固定版本 | 来源 |
|---|---|---|
| Rust | 1.99.1（`rustc 1.99.1` 实测） | rustup |
| Flutter | 3.47.9 | 官方归档 zip |
| VS 2022 Build Tools | v143 工具集 + Win11 SDK 22621 | winget `--override` |
| NSIS | 3.13（本机实测） | winget |
| Python | 3.12 | winget |
| Git | ≥ 2.30 | winget |

脚本结尾会打印实际版本表；**必须手动装**的组件及原因也在那里（显卡驱动、ToDesk、签名证书、FFmpeg/AMF 可选件）。

## 3. 从零到「能编译」

```powershell
cd D:\work\RDC        # 或在各线自己的 worktree

# 3.1 环境变量：构建产物不要落 C 盘（脚本已写用户级 CARGO_TARGET_DIR=E:\rdc-target）
$env:CARGO_TARGET_DIR = "E:\rdc-target"

# 3.2 Rust 侧
cargo check --workspace
cargo test  -p rdc-core -p rdc-update

# 3.3 Flutter 侧
cd app
flutter pub get
flutter analyze
flutter test

# 3.4 原生/桥接产物（Windows）
cd ..
cargo build --release -p rdc-bridge -p rdc-cli -p rdc-update
```

预期：`cargo check` 无 error；`flutter analyze` 输出 `No issues found!`；`flutter test` 全绿。
首次构建需要下载 crate 与 Flutter 引擎，耗时长属正常；第二次起应显著变快。

## 4. 从「能编译」到「能出包」

```powershell
# 4.1 常规出包（Rust + Flutter + NSIS + 便携 zip + .sha256）
.\packaging\windows\build_installer.ps1

# 4.2 只重打安装包（Flutter 已构建过）
.\packaging\windows\build_installer.ps1 -SkipFlutter

# 4.3 自更新清单（需要发布私钥，仅发布时做）
$env:RDC_RELEASE_SEED_B64 = "<从受控位置读取，勿写入仓库>"
.\packaging\windows\make_update_manifest.ps1 `
    -InstallerPath .\dist\RDC-<版本>-windows-x64-setup.exe `
    -Version <版本> `
    -Url "https://github.com/<账号>/<仓库>/releases/download/v<版本>/RDC-<版本>-windows-x64-setup.exe"
```

产物在 `dist\`：`RDC-<版本>-windows-x64-setup.exe`、`RDC-<版本>-windows-portable.zip` 及各自的 `.sha256`。
版本号唯一真源是根 `Cargo.toml` 的 `workspace.package.version`；`build_installer.ps1` 会把它同步到 `app/pubspec.yaml`，**不要手改两处**。

## 5. 验收清单（每步给预期结果）

| # | 命令 | 预期结果 |
|---|---|---|
| 1 | `cargo build --workspace` | 退出码 0，无 error（warning 允许） |
| 2 | `cargo test -p rdc-core -p rdc-update` | 全部 `ok`；`rdc-update` 的 `update_flow` 集成测试也通过 |
| 3 | `flutter analyze`（在 `app/`） | `No issues found!` |
| 4 | `flutter test`（在 `app/`） | `All tests passed!` |
| 5 | `.\packaging\windows\build_installer.ps1` | `dist\` 出现带版本号的安装包、便携 zip 与 `.sha256`；日志含 `版本 x.y.z` |
| 6 | 四步冒烟（装 → 起 → 首屏正常渲染 → 退出无残留） | 见下 |

四步冒烟的具体判据：

1. **装**：静默安装 `RDC-<版本>-windows-x64-setup.exe /S` 后，`C:\Program Files\RDC\rdc.exe` 存在；`config\` 目录对 `Users` 有修改权限（`icacls "C:\Program Files\RDC\config"` 能看到 `(OI)(CI)(M)`）。
2. **起**：双击 `rdc.exe`，进程存活 ≥ 10 秒且窗口出现。
3. **首屏正常渲染**：能看到完整界面（不是纯白/纯黑）。白屏时先看 `config\logs\`，`first_frame=rendered` 不能证明已修好。
4. **退出无残留**：关闭窗口后 `rdc.exe` 进程消失；`config\` 下不产生新的大文件。

## 6. 分支现状（2026-11-08）

| 分支 | worktree | 归属线 | 最新提交 | 状态 |
|---|---|---|---|---|
| `master` | `E:\rdc-merge` | 主干（合并由指挥做） | `4676826` docs(ui): 纠正冒烟判据、记录合并纪律与正式包位置 | 稳定 |
| `observability` | `..\RemoteDesktopControl` | 可观测性 / 门禁 | `8ed92f3` feat(gate): baseline only-down policy, CI dual jobs and change-record spec | 进行中（领先 master 6） |
| `linux-x11` | `..\RDC-linux` | Linux 线 | `0c43d1f` docs(research): 多显示器/Wayland 探测结论与安卓触摸手势映射 | 进行中（领先 5） |
| `android-client` | `..\RDC-android` | Android 客户端 | `151b995` 阶段5收尾：落盘模拟器复验数字与构建产物清理量 | 进行中（领先 10） |
| `ui-polish` | `..\RDC-ui` | UI 打磨 | `8098a35` ui-polish 第三批：首启引导 / 弹窗键盘约定 | 待合并 |
| `fix/rdr-fallback-window` | `..\RDC-render-fallback` | Windows 渲染保底窗口 | `92840d1` feat(windows): 原生 RDR-001 保底窗口，Flutter 白屏时仍可操作 | 待合并（领先 2） |
| `docs/requirements` | `..\RDC-docs` | 需求/决策记录 | `eb00912` docs: 登记 D57/D58 视频分片分层裁定，关闭 T-3 数值争议 | 进行中（领先 17） |
| `feature/auto-update` | `..\RDC-release` | 发布流水线 + 自更新 + 安装位置改造 | 见 `git log -1 feature/auto-update` | 进行中（本线） |

纪律：各线不自行合并 master（R-T-7）；一个 worktree 只归一条线（R-T-11）。

## 7. 已知坑（照做能少踩）

1. **UTF-8 BOM**：PowerShell 5.1 读取「无 BOM 的 UTF-8」会按 ANSI 解码，中文字符串直接乱码、语法报错。仓库里的 `.ps1` 统一保存为 **UTF-8 with BOM**；新增脚本后若报「哈希文本中的键后面缺少=运算符」之类怪错，先查 BOM。
2. **构建 target 不要落 C 盘**：全量构建可达数 GB。设置用户级 `CARGO_TARGET_DIR=E:\rdc-target`（`setup-dev.ps1` 在 E 盘存在时会自动设）。
3. **白屏 = 没有显示输出**，不一定是渲染 bug：先确认远程会话里有可采集的显示器。锁屏、RDP 断开、无物理显示器都会让 DDA 采不到帧。
4. **锁屏会让 DDA 无帧**：验证采集必须在已解锁且显示器活跃的会话；必要时用 ToDesk 虚拟显示器维持一路输出。
5. **AMF 属性设置要判返回值**：AMF 组件属性设置失败不会抛异常，只会返回非 `AMF_OK`；不判返回值会出现「编码器看似启动但无输出」。
6. **版本只改一处**：改版本只改根 `Cargo.toml`，`pubspec.yaml` 由打包脚本同步；两处手改必然漂移。
7. **NSIS `SetRegView` 必须在 Section/Function 内**：放全局会编译报错 `SetRegView must be used in Section or Function`。
8. **Program Files 下不能整目录可写**：只给 `config\` 授 `Users:(OI)(CI)M`；程序本体保持只读，否则等于拆掉 UAC 边界。
9. **升级前必须先退出应用**：`rdc-updater apply` 带 `--wait-pid`，等旧进程退出再覆盖 exe；否则安装器会因文件占用失败（`RDC-UPD-APPLY-004`）。
10. **`git bundle` 不含未提交改动**：生成 bundle 前先 `git status` 确认各 worktree 都已提交，否则新机器上会缺文件。

## 8. 本次实测记录（2026-11-08）

| 项 | 实测结果 |
|---|---|
| bundle 生成 | `rdc-all.bundle` 2,672,206 B（2.55 MB）；`git bundle verify` 报 `is okay`；含 8 条分支 + 1 个标签 + 完整历史 |
| bundle 恢复 | 克隆到 `E:\rdc-migrate\verify-repo` 成功；`git branch -a` 见 8 条 `origin/*`；逐条建本地分支成功；`git worktree add` 成功（HEAD `4676826`） |
| 工程经验库 | 0.2 MB / 10 个文件（含 `.git`） |
| 记忆 | `user_profile.md` + 项目记忆目录（4 个文件）原样拷贝 |
| setup-dev 自检 | `-WhatIf` 通过；版本表实测：Git 2.41.0、Rust 1.99.1、VS Build Tools 17.14.37827.9、Flutter 3.47.9、NSIS 3.13、Python 3.12.5 |
| 未验证 | 新机器从空环境实际安装（本机工具链已齐，`-WhatIf` 只证明检测/跳过逻辑）；Flutter 归档下载实际解压；Linux/Android 线构建 |
