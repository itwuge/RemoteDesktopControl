# 21 · GitHub 发布与客户端自更新

- 归属：发布线（`feat/auto-update`）
- 状态：文件与本地可验证部分已落地；**真实 Actions 运行与真实 Release 下载未验证**（本轮无远程仓库、未 push）
- 相关实现：`.github/workflows/release-windows.yml`、`crates/rdc-update/`、`packaging/windows/make_update_manifest.ps1`、`app/lib/model/update/`

## 1. 总体链路

```text
开发机 git tag vX.Y.Z ──push──▶ GitHub Actions（windows-2022）
    ├─ 准备 Rust 1.99.1 / Flutter 3.47.9 / NSIS / MSVC
    ├─ 读取根 Cargo.toml 的 workspace.package.version（唯一真源）
    ├─ build_installer.ps1 产出 setup.exe + portable.zip
    ├─ make_update_manifest.ps1 生成并 Ed25519 签名 update-manifest.json
    ├─ 生成 SHA256SUMS.txt + 各资产 .sha256
    └─ gh release create vX.Y.Z（资产见第 4 节）

客户端启动 ──▶ 读取 GitHub Releases/latest
    ├─ 标签、清单版本、安装包名三者一致才继续
    ├─ 验签（内置公钥）→ 比较版本（相同/更低则不下载）
    ├─ 下载安装包 → SHA256 与已签名清单比对，再与 .sha256 交叉校验
    ├─ 不匹配：删除半成品、拒绝替换、返回固定错误码 + 中文原因
    └─ 通过：备份当前安装目录 → 静默安装 → 校验版本 → 失败则回滚
```

服务器不参与分发：版本清单、安装包、签名全部由 GitHub Releases 承载。

## 2. 版本号唯一真源

| 位置 | 作用 |
| --- | --- |
| 根 `Cargo.toml` 的 `[workspace.package] version` | **唯一真源**；所有 crate 继承 |
| `app/pubspec.yaml` 的 `version:` | 与真源一致，打包脚本会按真源覆写 |
| `crates/rdc-update/src/lib.rs::current_version()` | 编译期注入（`CARGO_PKG_VERSION`），客户端上报的当前版本 |
| Git 标签 `v<版本>` | 推送触发发布；标签与真源不一致时工作流直接失败 |

规则：**先升版本再打标签**；禁止同号重复发布（工作流遇到同标签会先删除旧 Release 再重建，资产必须与本次构建一致）。

## 3. 首次发布前，用户必须准备的事项

### 3.1 建仓库并确定 owner/name

1. 在 GitHub 创建仓库 `RemoteDesktopControl`（公开或私有均可）。
2. 记录 `owner/name`，例如 `your-name/RemoteDesktopControl`。
3. owner/name 在代码里**只有一处**声明：

```rust
// crates/rdc-update/src/lib.rs
pub const DEFAULT_REPO_SLUG: &str = "rdc-team/RemoteDesktopControl"; // ← 改成你的 owner/name
```

也可不改代码，改用环境变量（优先级从高到低）：

| 变量 | 含义 |
| --- | --- |
| `RDC_UPDATE_API` | 完整 latest 接口地址（自建镜像/测试服务器用） |
| `RDC_UPDATE_REPO` | `owner/name`，自动拼 `https://api.github.com/repos/<slug>/releases/latest` |

> Flutter 侧不硬编码地址：启动时通过 Rust 桥接读取 `updateDefaultApiUrl()`，UI 只消费结果。

### 3.2 Actions 权限

仓库 `Settings → Actions → General → Workflow permissions` 选择 **Read and write permissions**。
工作流已声明 `permissions: contents: write`，用于 `gh release create`；发布只使用内置 `GITHUB_TOKEN`，无需额外 PAT。

### 3.3 发布签名密钥（一次性）

自更新清单用 Ed25519 签名，客户端只信任编译进 `rdc-update` 的公钥。

1. 生成密钥对（本机执行，输出只打印一次，禁止提交/截图外传）：

```powershell
cargo run -p rdc-update --bin rdc-updater -- genkey
```

2. 把输出里的 `public_key_b64` 写入：

```rust
// crates/rdc-update/src/lib.rs
pub const RELEASE_PUBLIC_KEY_B64: &str = "<public_key_b64>";
```

3. 把输出里的 `seed_b64` 写入仓库 Secret：

- 名称：`RDC_RELEASE_SEED_B64`
- 路径：`Settings → Secrets and variables → Actions → New repository secret`

> 工作流在缺 Secret 时**直接失败**，不产出无法验签的包；`make_update_manifest.ps1` 还会断言“签名公钥 = 客户端内置公钥”，防止发错密钥。
>
> 仅换机器、不换密钥时：只迁移 Secret，公钥常量保持不变。

### 3.4 私有仓库场景

私有仓库的 Release 资产需要鉴权下载。客户端可通过 `RDC_GITHUB_TOKEN` 环境变量提供只读 token（`rdc-updater` 会带上 `Authorization`）；公开仓库无需 token。

## 4. 发布产物（Release 资产）

| 资产 | 说明 |
| --- | --- |
| `RDC-<版本>-windows-x64-setup.exe` | NSIS 安装包（静默升级入口） |
| `RDC-<版本>-windows-x64-setup.exe.sha256` | 安装包哈希（与清单交叉校验） |
| `RDC-<版本>-windows-portable.zip` | 便携包 |
| `RDC-<版本>-windows-portable.zip.sha256` | 便携包哈希 |
| `update-manifest.json` | 版本清单（含安装包名/sha256/size/url/notes） |
| `update-manifest.json.sig` | 清单 Ed25519 签名 |
| `update-manifest.json.sha256` | 清单自身哈希，便于人工核对 |
| `SHA256SUMS.txt` | 全资产汇总 |

`update-manifest.json` 的 `installer.name` 必须与 Release 资产名一致；客户端按名字精确匹配，不猜。

## 5. 客户端更新行为与判据

| 场景 | 行为 | 判据（单测覆盖） |
| --- | --- | --- |
| 有更高版本 | 展示版本与说明，用户确认后下载 | `update_flow::check_download_and_reject_tampered_payload` |
| 版本相同或更低 | `available=false`，**不发起下载** | `update_flow::same_version_reports_no_update` |
| 下载后哈希不符 | 删除 `.part`，保留旧版本，返回 `RDC-UPD-HASH-*` | 同上（篡改安装包） |
| `.sha256` 与清单不一致 | 下载前即拒绝 | 同上（第 4 步） |
| 网络失败 | 固定错误码 `RDC-UPD-NET-001` + 含地址的中文说明 | `update_flow::network_failure_returns_readable_error` |
| 安装失败 | 恢复备份目录，返回 `rolled_back=true` 与错误码 | `apply::tests::backup_and_restore_round_trip` |

“自动检查更新”开关默认开启；关闭后启动期不创建定时器、不联网。手动“检查更新”仍可用。

## 6. 本地演练（不 push、不发 Release）

```powershell
# 1) 版本与构建
cargo build --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo test -p rdc-update

# 2) 工作流语法
python -c "import yaml,sys; yaml.safe_load(open('.github/workflows/release-windows.yml',encoding='utf-8')); print('ok')"

# 3) 本地打安装包（需要 Flutter）
./packaging/windows/build_installer.ps1 -Version <版本>

# 4) 用测试密钥签名清单（仅本地，会断言公钥一致性）
$env:RDC_RELEASE_SEED_B64 = "<测试种子>"
./packaging/windows/make_update_manifest.ps1 -InstallerPath dist\RDC-<版本>-windows-x64-setup.exe -Version <版本>
```

## 7. 尚未验证（不得写成通过）

- GitHub Actions 真实运行（本机无远程仓库、本轮未 push）。
- 真实 GitHub Release 的 `releases/latest` 拉取与资产下载。
- 在干净 Windows 机器上从旧版本静默升级 + 失败回滚的端到端演练。
- NSIS 安装包在本轮合并后的完整打包（依赖 Flutter release 构建与本机 NSIS）。

## 8. 回滚要点

- 升级前 `rdc-updater apply` 会把安装目录备份到暂存根下的 `backup-<时间戳>`。
- 安装器退出码非 0 或安装后版本不符：自动调用 `restore` 并返回 `rolled_back=true`；失败会给出 `RDC-UPD-ROLLBACK-001` 与备份路径，需人工介入。
- 误发版本：在 GitHub 删除对应 Release 与标签，客户端 `latest` 自动回到上一个可用版本（已下载客户端不受影响）。
