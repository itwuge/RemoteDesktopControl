# TESTPLAN · GitHub Actions 发布 + 客户端自更新

- 记录时间：2026-11-08
- 工作树：`C:\RDC-update` 分支 `feat/auto-update`（基于 master `672a3c7`，并入既有 `feature/auto-update` 实现）
- 纪律：判据先行；只跑本地可验证部分；无远程仓库，本轮不做 push、不做真实 Release。

## 一、判据（先定后跑，不达标不改口径）

| 编号 | 判据 | 判定方法 | 本轮可否验证 |
| --- | --- | --- | --- |
| C1 | 发布工作流 YAML 语法合法 | 用 Python `yaml.safe_load` 解析 `.github/workflows/release-windows.yml`，无异常且能取到 `jobs` | 可验证 |
| C2 | 版本号单一来源 | 根 `Cargo.toml` `[workspace.package] version` 为唯一真源；`app/pubspec.yaml` 与之一致；所有 crate 继承 workspace 版本；工作流从根 Cargo.toml 读取并校验标签一致 | 可验证 |
| C3 | 产物命名符合约定 | 打包脚本/工作流中安装包为 `RDC-<版本>-windows-x64-setup.exe`，便携包为 `RDC-<版本>-windows-portable.zip`，且版本号参与命名（不与旧包同号） | 可验证（静态核对 + 打包脚本干跑） |
| C4 | Rust 代码可编译 | `cargo build --workspace` 退出码 0 | 可验证 |
| C5 | Clippy 无告警 | `cargo clippy --workspace --all-targets -- -D warnings` 退出码 0 | 可验证 |
| C6 | 更新器单测通过 | `cargo test -p rdc-update` 全绿，且至少覆盖四类场景：模拟 GitHub API 响应、版本相同不下载、哈希不匹配必须拒绝、网络失败返回可读错误 | 可验证 |
| C7 | 更新源可配置 | 仓库 owner/name 不散落在代码深处：仅集中在一处默认常量，且可由参数/环境变量覆盖（`RDC_UPDATE_API` / `--api`） | 可验证（静态 + 单测） |
| C8 | 下载后校验且失败不替换 | 下载完成后先比 SHA-256（并校验已签名清单），不匹配即拒绝、删除半成品、不触碰安装目录；替换失败有备份回滚 | 可验证（单测） |
| C9 | 替换失败有回滚或明确提示 | 安装器退出码非 0 时恢复备份目录，返回 `rolled_back=true` 与错误码 | 可验证（单测，若环境允许） |
| C10 | 文档说明首次发布准备 | `docs/` 下新增独立文档，写清建仓库、填 owner、配 token/权限、打标签流程；不修改既有文档正文 | 可验证（文件核对） |
| C11 | Flutter 侧静态检查 | `flutter analyze` 无 error（warning 单列，不掩盖） | 可验证（依赖 pub get） |
| C12 | 真实 Actions 运行 | GitHub 上真实跑一次 release 工作流并产出 Release | **未验证**（无远程仓库、未 push） |
| C13 | 真实 Release 端到端下载 | 客户端从真实 GitHub Releases 拉取并升级 | **未验证**（无真实 Release） |

## 二、执行记录

（按执行顺序追加，每条写明命令、退出码、关键输出、结论）

### 2026-11-08 · 合并既有实现后本地验证

| 编号 | 命令 / 方法 | 退出码与关键输出 | 结论 |
| --- | --- | --- | --- |
| C1 | `python -c "yaml.safe_load(...release-windows.yml)"`；同法校验 `ci.yml` | `YAML_OK jobs=['release-windows'] steps=13`；`CI_YAML_OK jobs=['rust','windows','macos','linux','android','compliance']` | **通过** |
| C2 | `cargo metadata --no-deps` 列举版本；核对根 `Cargo.toml`、`app/pubspec.yaml`；阅读工作流版本步骤 | 18 个 crate 全部 `0.1.2`；pubspec `0.1.2+1`；工作流从根 Cargo.toml 读版本并校验 `v*` 标签一致 | **通过** |
| C3 | 核对 `build_installer.ps1` 第 305/313 行与工作流资产清单 | `RDC-$Version-windows-x64-setup.exe`、`RDC-$Version-windows-portable.zip`，版本号参与命名 | **通过（静态核对）** |
| C4 | `cargo build --workspace` | `EXIT=0`，`Finished dev profile` | **通过** |
| C5 | `cargo clippy --workspace --all-targets -- -D warnings` | `CLIPPY_EXIT=0`，`Finished dev profile` | **通过** |
| C6 | `cargo test -p rdc-update` | 12 单元 + 3 集成全绿（`same_version_reports_no_update`、`check_download_and_reject_tampered_payload`、`network_failure_returns_readable_error`）；耗时 7.02s（含退避重试） | **通过** |
| C7 | 阅读 `lib.rs`；`cargo test -p rdc-update` 内置 `repo_slug_builds_latest_api_url`、`explicit_api_url_wins` | owner/name 仅 `DEFAULT_REPO_SLUG` 一处；支持 `RDC_UPDATE_REPO`、`RDC_UPDATE_API`、显式参数三级覆盖 | **通过** |
| C8 | 篡改用例第 3 步：哈希不匹配 → `RDC-UPD-HASH-*`，`.part` 不残留；`.sha256` 不一致 → 下载前拒绝 | 测试断言全部通过 | **通过** |
| C9 | `apply::tests::backup_and_restore_round_trip` | 备份/恢复函数单测通过；**「安装器退出非 0 → 自动回滚」端到端未验证** | 部分通过 |
| C10 | 核对新增 `docs/21-GitHub发布与客户端自更新.md` | 文件新增，未修改既有文档正文 | **通过** |
| C11 | `flutter pub get`；`flutter analyze`；`flutter test` | pub get 成功；`No issues found!`；`All tests passed!`（18 项） | **通过** |
| C12 | 真实 GitHub Actions 运行 | 无远程仓库、未 push | **未验证** |
| C13 | 真实 Release 端到端下载 | 无真实 Release | **未验证** |

### 2026-11-08 · 版本冲突修正（0.1.2 → 0.3.0）与全量复跑

背景：已发布 v1 为 **0.2.0**；本线原定 0.1.2 低于已装机版本，客户端会判定「无更新」，等于自更新失效。

改动：根 `Cargo.toml` workspace.package.version、`app/pubspec.yaml`、NSIS 兜底 `!define VERSION`、
`build_installer.ps1` 兜底版本、`README.md` 产物示例、`docs/21` 版本说明与实现归属章节。
18 个 crate 继承 workspace 版本，无需逐个改。

| 判据 | 命令 | 退出码 / 关键输出 | 结论 |
| --- | --- | --- | --- |
| C2 | `cargo metadata --no-deps` | `['0.3.0'] 18`（18 个包全部 0.3.0） | **通过** |
| C4 | `cargo build --workspace` | 退出码 0，`Finished dev profile`（1m31s） | **通过** |
| C5 | `cargo clippy --workspace --all-targets -- -D warnings` | 退出码 0，`Finished dev profile`（35.46s） | **通过** |
| C6 | `cargo test --workspace` | 全部 `test result: ok`；rdc-update 12 单元 + 3 集成通过 | **通过** |
| C11 | `flutter pub get` / `flutter analyze` / `flutter test` | `No issues found!`；`All tests passed!`（18 项） | **通过** |

### 2026-11-08 · 更新实现唯一性核对

| 核对项 | 命令 | 结果 |
| --- | --- | --- |
| master 是否有自更新代码 | `git ls-tree -r --name-only master \| Select-String "rdc-update\|update_service\|update_controller"` | 空 |
| release/v1 是否有自更新代码 | 同上（`release/v1`） | 空（v1 版本 0.2.0） |
| 本分支更新入口 | `git grep UpdateService\|UpdateController` | 仅 `app/lib/model/update/update_service.dart` → `update_controller.dart` → `main.dart`/`settings_page.dart` 一条链 |
| 其它自更新实现 | `git grep "self-update\|self_update\|自更新\|upgrade"` | 仅 `crates/rdc-update`（唯一二进制 `rdc-updater`） |

结论：**只有一套实现、一个入口**，无并存、无重复；`feature/auto-update` 旧分支并入后作废，已写入 `docs/21`。

### 已知差异 / 需要用户提供

1. `DEFAULT_REPO_SLUG` 与 `RELEASE_PUBLIC_KEY_B64` 仍是占位值，需用户按 `docs/21` 第 3 节替换。
2. CI Secret `RDC_RELEASE_SEED_B64` 未配置（本轮不发布，不阻塞本地验证）。
3. NSIS 完整打包未在本轮执行（依赖 Flutter release 构建与本机 NSIS 环境）。
4. 密钥未生成：等用户确认「复用桌面已有密钥」还是「新造」。本轮未执行 `genkey`。

