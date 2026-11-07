# 工程门禁模板（Portable Engineering Gate）

一个只做静态检查的 PowerShell 门禁脚本。脚本本体不含任何项目路径、crate 名、模块名或白名单；所有项目相关信息都来自外部的 `gate.config.json`。

## 1. 怎么拷进新项目

把本目录（`gate.ps1` + `gate.config.example.json`）复制到新项目的 `tools/gate-template/` 或 `scripts/` 下，然后把配置样例复制成项目根目录的 `gate.config.json`：

```powershell
Copy-Item tools\gate-template\gate.ps1 .\scripts\gate.ps1
Copy-Item tools\gate-template\gate.config.example.json .\gate.config.json
```

按项目实际情况改 `gate.config.json`。**不配置也能跑**：脚本会使用内置默认值，并把它假设的路径、crate 目录、日志依赖、磁盘阈值打印出来，便于逐项确认。

要求 PowerShell 5.1 或更高。脚本内含中文提示，文件必须保存为 **UTF-8 with BOM**，否则 Windows PowerShell 5 会按 ANSI 读取导致乱码/解析失败。

## 2. 常用命令

```powershell
# 全量检查
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\gate.ps1

# 只查相对基线分支的改动（本地提交前最常用）
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\gate.ps1 -Changed -BaseRef origin/main

# 只查指定文件/目录
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\gate.ps1 -Paths src\foo.rs,tools

# 只跑某几条规则
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\gate.ps1 -Rules R-C-2,R-L-1

# 门禁自测（在临时目录构造违规样本，必须全部命中）
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\gate.ps1 -SelfTest

# 能自动修的自动修（目前只有 UTF-8 BOM）
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\gate.ps1 -Fix

# 打包/发布前额外检查冒烟记录
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\gate.ps1 -Build

# 指定构建盘与阈值
powershell -NoProfile -ExecutionPolicy Bypass -File scripts\gate.ps1 -TargetDir E:\cargo-target -MinFreeGB 10
```

退出码：`0` 无违规；`1` 发现违规；`2` 脚本错误或自测失败。

输出格式为 `文件:行: 规则编号 说明`，末尾给出按规则分组的汇总。

## 3. CI 接法

```yaml
- name: Engineering gate
  shell: pwsh
  run: |
    ./scripts/gate.ps1 -SelfTest
    ./scripts/gate.ps1 -Changed -BaseRef origin/main
```

`-SelfTest` 先跑，保证规则本身没有失效；再跑 `-Changed` 只检查本次改动。

## 4. 规则编号

编号来自跨项目《铁律》文档，分五个域。`ruleIds` 配置负责“语义键 → 规则编号”的映射；如果经验库编号调整，只改配置，不改脚本逻辑。

| 规则 | 检查内容 | 实现状态 | 自测样本 |
| --- | --- | --- | --- |
| R-C-2 | `let _ =`、`.ok()`、`unwrap_or_default()` 丢弃 IO/编解码/网络错误，或只打日志不返回 | 已实现 | `let _ =` / `.ok()` |
| R-C-3 | `anyhow!` 无 source、`map_err(|_|)`、错误只转字符串 | 已实现 | `anyhow!`、`map_err(|_|)`、`format!("{e}")` |
| R-C-5 | `unbounded` 通道、无容量上限的流式 `VecDeque`/`Vec` | 已实现 | `VecDeque::new` 字段 |
| R-C-14 | 版本号不一致（Cargo / pubspec / NSIS / BUILD_INFO / 产物文件名） | 已实现 | `Cargo.toml 0.1.0` vs `setup.nsi 0.9.9` |
| R-C-15 | target 目录落在系统盘、构建前可用空间低于阈值 | 已实现 | 系统盘 target 路径 |
| R-C-1 | 含非 ASCII 的 `.cpp/.h` 缺少 UTF-8 BOM（`-Fix` 可自动补） | 已实现 | 中文 `.cpp` 无 BOM |
| R-N-1 | 源文件前 N 行缺少头注释块 | 已实现 | 无头注释的 `lib.rs` |
| R-N-2 | 公开函数缺少文档注释 / 参数段 | 已实现 | `pub fn` 无 `///` |
| R-N-4 | 长文件缺少 `# =====** 组件名 =====**` 组件分区标记，或仍用占位名 | 已实现 | 240 行无组件标记 |
| R-N-5 | 更长文件缺少 `# =========** 组件名 =========` 控制分区标记 | 已实现 | 420 行无控制标记 |
| R-N-7 | 注释/分区标记与代码是否同批更新 | 仅人工 | — |
| R-L-1 | `println!` / `eprintln!` / `print` / `std::cout` 等旁路统一日志 | 已实现 | Rust `println!` + Dart `print` |
| R-L-4 | crate 未依赖统一日志层 | 已实现 | 没有 `tracing` 依赖的 crate |
| R-L-9 | 只打日志、不返回/不传播错误 | 已实现 | `if let Err` 只 `warn!` |
| R-T-2 | 未解决的合并冲突标记 | 已实现 | `<<<<<<< HEAD` |
| R-T-4 | 源码有改动但缺少改动记录文件 | 已实现（需配 `changeRecordGlobs`） | 改动源码 + 无记录文件 |
| R-T-8 | 同一文件被两条线同时修改 | 仅人工 | — |
| R-D-9 | 源码变化但版本号未递增 | 已实现（`-Changed` 模式） | 版本保持 `0.1.0` |
| R-D-9 | 构建/发布缺少冒烟记录 | 已实现（`-Build` 模式） | 无 `smoke-*.log` |
| R-T-5 | 里程碑是否按约定上报 | 仅人工 | — |

编号以跨项目经验库的《铁律》文档为准。若本仓库规范文档与经验库编号冲突，以经验库为准，并在使用方的 `docs/` 中记录差异。

## 5. 白名单

路径级白名单放 `logging.allowedPrintGlobs`；逐行/逐文件例外放 `whitelist`，每条必须写 `reason`：

```json
{
  "rule": "R-C-2",
  "glob": "src/legacy/**",
  "pattern": "let _ =",
  "reason": "历史模块，下个迭代删除"
}
```

单行例外也可以直接写在源码行尾：

```rust
let _ = tx.send(frame); // gate-allow R-C-2: 接收端已退出，丢弃是预期行为
```

白名单是债，不是通行证；理由必须能被人复述。

## 6. 配置项速查

- `scan.include/exclude`：扫描范围与生成物排除。
- `rust.cratesDir/sourceGlobs/streamingGlobs/pathSensitiveGlobs/requiredLogDeps`：Rust 规则适用范围。
- `note.*`：头注释正则、扫描行数、分区标记正则、视图/控制层路径（`uiGlobs`/`controlGlobs`）、长文件阈值。
- `team.changeRecordGlobs`：改动记录文件；为空时 R-T-4 视为“未配置”而不是失败。
- `command.smokeRecordGlobs`：冒烟记录；`-Build` 时检查。
- `version.*`：版本源文件、产物目录与扩展名。
- `build.*`：构建目录环境变量、默认 target、可用空间阈值。
- `logging.allowedPrintGlobs`：允许直接 stdout 的路径（示例、测试、CLI 等）。

## 7. 已知限制

- 纯静态、单文件正则扫描，不做数据流分析；宁可漏报也不误伤时，用 `enabled=false` 或白名单收敛。
- `-Fix` 只补 UTF-8 BOM；错误上下文、队列上限等语义修复必须人工。
- R-N-7、R-T-8、R-T-5 无法机器判定，明确标记为“仅人工”。
