# RDC 整机迁移交接

> 用途：把本项目从一台 Windows 机器搬到另一台，做到「拷过去就能开工」。
> 适用范围：Windows 10/11 x64；Linux/Android 线只恢复代码，不保证在本机交叉编译。
> 最近校验：2026-11-08（本文件 §8 为实测记录；未跑过的项明确标"未验证"）。
> 包根目录：`C:\RDC-MIGRATE\`（整个文件夹可原样拷到 U 盘；脚本内不写死盘符）。

## 0. 迁移包里有什么

| 内容 | 路径 | 说明 |
|---|---|---|
| 唯一入口 | `一键开工.ps1` | 自检 → 装工具链 → 恢复仓库 → 构建 → 可选出包与冒烟；幂等，可重复跑 |
| 三行上手 | `START-HERE.md` | 中英各三行 |
| 交接文档 | `MIGRATION.md` | 本文件 |
| 全部代码与历史 | `rdc-all.bundle` | `git bundle --all`，含 8 条分支 + 1 个标签，可直接 clone |
| 工具链安装 | `env\setup-dev.ps1` | 固定版本、幂等、带版本核对表 |
| 工程经验库 | `经验库.zip` | 铁律、坑库、开工清单、门禁模板（原样打包） |
| 记忆 | `memory.zip` | `user_profile.md` + 项目记忆目录（原样打包，未加工） |
| 团队与职责 | `TEAM.md` | 人员编制、分线、当前任务、遇到什么找谁 |
| 硬规矩全文 | `RULES.md` | 59 条铁律全文 + 23 条可自动化清单 + 坑库附录（与经验库同步，不摘要） |
| 指挥与验收规范 | `WORKFLOW.md` | 判据先行、报告、合并、出包、清理、授权边界 |
| 门禁脚本 | `gate\gate.ps1`、`gate\gate.config.example.json` | 新项目第一步：复制门禁、改 `ruleIds`，不要重写脚本 |
| 版本包 | `dist\` | 各版本安装包/便携包 + `.sha256`；用途见 `dist\README.md` |
| 测试证据 | `logs\` | 判据文件、跑测日志、截图与样本（用户要求带走） |
| 关键文档快照 | `docs\` | 限制表、错误码与 RDR-001、经验与坑、工程铁律、变更记录、决策待办 |

**没有随包带走的东西**（新机器上补不回来，只能重新产生）：

| 缺失项 | 影响 | 怎么补 |
|---|---|---|
| 对话历史 / 会话上下文 | 无法追溯当时的讨论细节 | 只有本文件与 `logs/` 下的判据、`docs/决策待办.md` 可查 |
| ToDesk 与虚拟显示器驱动 | 锁屏/无显示器时采集到黑屏或白屏 | 新机器装 ToDesk 并启用虚拟显示器（见 §7） |
| 代码签名证书与私钥 | Release/安装包无法做 Authenticode 签名 | 证书不迁移（安全约束）；需重新申请或导入受控硬件密钥 |
| 自更新发布私钥 | 无法为新版本签 `update-manifest.json` | 见 `packaging/windows/make_update_manifest.ps1`；私钥只在 CI Secret `RDC_RELEASE_SEED_B64` |
| FFmpeg LGPL 共享构建 | 出包退回「仅 MF 解码」 | 可选：自建到 `C:\dev\ffmpeg-out`（`build_installer.ps1` 会显式警告） |
| AMF SDK 头文件 + MinGW | 包内无 `rdc-amf.dll`，只能用 MFT 软编 | 可选：见 `crates/rdc-codec/shim/build_amf.ps1` |
| 本机驱动状态、注册表象 | 与采集/编码相关的能力差异 | 按 §7 的顺序重装显卡驱动 |

## 1. 磁盘与空间（只有 C 盘的情况）

本机实测：**全量编译产物约 2.6 GB**（`cargo build --workspace` + `flutter build windows --release` 的 `target\` 与 `app\build\`），另有 NuGet/crate 缓存若干。

| 阶段 | 需要可用空间 | 说明 |
|---|---|---|
| 拷入迁移包 | 约 0.45 GB | 迁移包本身（bundle + dist + logs + 文档） |
| 恢复 bundle | ≥ 2 GB | 脚本在 clone 前检查，不足则只解包不构建 |
| 开始构建 | ≥ 8 GB | 2.6 GB 产物 + 依赖缓存 + 临时文件，留余量 |
| 装完工具链后的 C 盘总需求 | **约 12~15 GB** | Rust + Flutter SDK + VS Build Tools + 构建产物 |

只有 C 盘时构建 target 固定在 `C:\RDC-WORK\target`，无法挪走；脚本在构建前后都会打印实际占用，便于判断删哪个能腾空间。
将来换到有 D 盘的机器：只改 `一键开工.ps1` 顶部两行 `$MigrateRoot` / `$WorkRoot`，其余不用动。

## 2. 一句话开工

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File C:\RDC-MIGRATE\一键开工.ps1
```

常用参数：

| 参数 | 作用 |
|---|---|
| （无） | 自检 → 恢复 → 只建 master worktree → 构建 |
| `-Check` | 只做环境自检，不下载、不克隆、不构建 |
| `-All` | 额外建齐其余 7 条线的 worktree |
| `-Package` | 构建后出包（NSIS + 便携 zip + `.sha256`）并做四步冒烟 |
| `-NoBuild` | 只恢复仓库，不构建 |

退出码：`0` 成功；`2` 空间不足（只解包不构建）；`3` 工具链缺失且自动安装失败；`4` 恢复仓库失败；`5` 构建失败；`6` 出包或冒烟失败。

## 3. 手工恢复代码（脚本做的事，逐步版）

```powershell
# 3.1 从 bundle 克隆（bundle 自带全部历史）
git clone C:\RDC-MIGRATE\rdc-all.bundle C:\RDC-WORK\RDC

# 3.2 验证完整性：末行应为 "is okay"，应看到 8 条 branches 与 1 个 tag
git -C C:\RDC-WORK\RDC bundle verify C:\RDC-MIGRATE\rdc-all.bundle

# 3.3 把 origin/* 落成本地分支
cd C:\RDC-WORK\RDC
foreach ($b in @("master","linux-x11","android-client","ui-polish","fix/rdr-fallback-window","docs/requirements","observability","feature/auto-update")) {
    git branch $b "origin/$b"
}

# 3.4 主工作区切 master，其余按线建 worktree（一条线一个目录）
git checkout master
git worktree add ..\RDC-linux           linux-x11
git worktree add ..\RDC-android         android-client
git worktree add ..\RDC-ui              ui-polish
git worktree add ..\RDC-render-fallback fix/rdr-fallback-window
git worktree add ..\RDC-docs            docs/requirements
git worktree add ..\RDC-observability   observability
git worktree add ..\RDC-release         feature/auto-update
git worktree list

# 3.5 以后接 GitHub：bundle 的 origin 指向本地文件，改名保留
git remote rename origin bundle
git remote add origin https://github.com/<账号>/<仓库>.git
git push -u origin --all
git push origin --tags
```

> 注意：`git worktree add` 前先 `git checkout master`，否则 master 被占用会报 "already checked out"。
> 纪律：**一个工作树只归一条线**（R-T-11）。

## 4. 工具链

```powershell
# 只打印计划，不下载不安装
powershell -NoProfile -ExecutionPolicy Bypass -File C:\RDC-MIGRATE\env\setup-dev.ps1 -WhatIf

# 正式执行（幂等，可重复跑）
powershell -NoProfile -ExecutionPolicy Bypass -File C:\RDC-MIGRATE\env\setup-dev.ps1
```

固定版本（与 CI `.github/workflows/release-windows.yml` 一致）：

| 组件 | 固定版本 | 来源 |
|---|---|---|
| Rust | 1.99.1 | rustup |
| Flutter | 3.47.9 | 官方归档 zip |
| VS 2022 Build Tools | v143 工具集 + Win11 SDK 22621 | winget `--override` |
| NSIS | 3.13 | winget |
| Python | 3.12 | winget |
| Git | ≥ 2.30 | winget |

**必须手动装**：显卡驱动（采集依赖 DXGI/DDA 实测）、ToDesk（远程会话与虚拟显示器）、代码签名证书（涉及私钥保管，不能脚本化）。

## 5. 从零到「能编译」

```powershell
cd C:\RDC-WORK\RDC

# 构建产物固定在本机工作区，不落系统临时目录
$env:CARGO_TARGET_DIR = "C:\RDC-WORK\target"

cargo check --workspace
cargo test  -p rdc-core -p rdc-update

cd app
flutter pub get
flutter analyze
flutter test
```

预期：`cargo check` 无 error；`flutter analyze` 输出 `No issues found!`；`flutter test` 全绿。
首次构建要下载 crate 与 Flutter 引擎，耗时长属正常。

## 6. 从「能编译」到「能出包」

```powershell
.\packaging\windows\build_installer.ps1

# 只重打安装包（Flutter 已构建过）
.\packaging\windows\build_installer.ps1 -SkipFlutter
```

产物在 `dist\`：`RDC-<版本>-windows-x64-setup.exe`、`RDC-<版本>-windows-portable.zip` 及各自 `.sha256`。
版本号唯一真源是根 `Cargo.toml` 的 `workspace.package.version`；`build_installer.ps1` 会同步到 `app/pubspec.yaml`，**不要手改两处**。

## 7. 验收清单（每步给预期结果）

| # | 命令 | 预期结果 |
|---|---|---|
| 1 | `cargo build --workspace` | 退出码 0，无 error（warning 允许） |
| 2 | `cargo test -p rdc-core -p rdc-update` | 全部 `ok`；`update_flow` 集成测试通过 |
| 3 | `flutter analyze`（`app/`） | `No issues found!` |
| 4 | `flutter test`（`app/`） | `All tests passed!` |
| 5 | `.\packaging\windows\build_installer.ps1` | `dist\` 出现带版本号的安装包、便携 zip 与 `.sha256` |
| 6 | 四步冒烟 | 见下 |

四步冒烟判据：

1. **装**：静默安装后 `C:\Program Files\RDC\rdc.exe` 存在；`icacls "C:\Program Files\RDC\config"` 含 `(OI)(CI)(M)`。
2. **起**：进程存活 ≥ 10 秒且窗口出现。
3. **首屏**：按像素判据 `spread > 阈值`（纯色屏 spread=0），或日志出现 `render_check=content`。白屏时先看 `config\logs\`，`first_frame=rendered` 不能证明已修好。
4. **退出无残留**：关闭窗口后 `rdc.exe` 进程消失；`config\` 下不新增异常大文件。

## 8. 分支现状（2026-11-08）

| 分支 | 归属线 | 最新提交 | 状态 |
|---|---|---|---|
| `master` | 主干（合并由指挥做） | `a687cd9` docs(ui): 记录首启引导端到端验证 | 稳定 |
| `observability` | 可观测性 / 门禁 | `e0350c0` feat(gate): per-entry baseline and changed-mode debt filter | 进行中（领先 master 7） |
| `linux-x11` | Linux 线 | `0c43d1f` docs(research): 多显示器/Wayland 探测结论 | 进行中（领先 5） |
| `android-client` | Android 客户端 | `151b995` 阶段5收尾 | 进行中（领先 10） |
| `ui-polish` | UI 打磨 | `8098a35` ui-polish 第三批 | 待合并 |
| `fix/rdr-fallback-window` | Windows 渲染保底窗口 | `92840d1` 原生 RDR-001 保底窗口 | 待合并（领先 2） |
| `docs/requirements` | 需求/决策记录 | `eb00912` 登记 D57/D58 视频分片分层裁定 | 进行中（领先 17） |
| `feature/auto-update` | 发布流水线 + 自更新 + 安装位置改造 | `git log -1 feature/auto-update` | 本线，未合并 master |

详细分工与「遇到什么找谁」见 `TEAM.md`；判据/合并/出包/清理纪律见 `WORKFLOW.md`。

## 9. 已知坑（照做能少踩）

1. **UTF-8 BOM**：PowerShell 5.1 读取「无 BOM 的 UTF-8」会按 ANSI 解码，中文字符串乱码、语法报错。仓库内 `.ps1` 统一保存为 **UTF-8 with BOM**。
2. **构建 target**：只有 C 盘时固定在 `C:\RDC-WORK\target`；有 D/E 盘时把 `$WorkRoot` 指过去。不要把 target 留在用户临时目录。
3. **白屏 = 没有显示输出**：先确认远程会话里有可采集的显示器。锁屏、RDP 断开、无物理显示器都会让 DDA 采不到帧。
4. **锁屏会让 DDA 无帧**：验证采集必须在已解锁且显示器活跃的会话；必要时用 ToDesk 虚拟显示器维持一路输出。
5. **AMF 属性设置要判返回值**：失败只返回非 `AMF_OK`，不抛异常；不判会出现「编码器看似启动但无输出」。
6. **版本只改一处**：改根 `Cargo.toml`，`pubspec.yaml` 由打包脚本同步。
7. **NSIS `SetRegView` 必须在 Section/Function 内**：放全局会编译报错。
8. **Program Files 下不能整目录可写**：只给 `config\` 授 `Users:(OI)(CI)M`；程序本体保持只读。
9. **升级前必须先退出应用**：`rdc-updater apply --wait-pid` 等旧进程退出再覆盖，否则 `RDC-UPD-APPLY-004`。
10. **`git bundle` 不含未提交改动**：生成 bundle 前先确认各 worktree 已提交。

## 10. 本次实测记录（2026-11-08）

| 项 | 实测结果 |
|---|---|
| bundle 生成 | 见 `D` 段实测（文件大小 / `git bundle verify` / 分支数） |
| bundle 恢复 | 见 `D` 段实测（克隆、建分支、建 worktree、HEAD） |
| 构建 | 见 `D` 段实测（`-Check` 与完整构建耗时、结果） |
| 冒烟 | 见 `D` 段实测（启动、标题、像素判据、退出） |
| 未验证 | 新机器从空环境实际安装工具链（本机工具链已齐，只验证了检测/跳过逻辑）；Linux/Android 线构建 |
