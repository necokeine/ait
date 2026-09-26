# 工作区改动整合

日期：2026-09-26。基线为 `42fca92`（已合并 PR #109）。本次提交此前保留在本地的全部
可入库改动，保留 PR #109 的非 macOS Clippy 修复和 ADR-050 的后续能力说明。

## 变更范围

- 修复 Terminal 重复订阅相互替换、后台 Git diff 轮询争用前台预算、未创建首个 commit
  的仓库 diff、带空格路径与路径段搜索、未存在 Worktree 子路径的真实前缀解析。
- 修复 Workspace Labels 并发更新覆盖其他字段、归档/删除目标的重复校验、回滚日志
  before-image 和持久化文档大小校验；纳入其他 server 能力的回归测试与上游来源清单。
  契约修订见 ADR-033 和 ADR-037，原有测试移植记录见 [Server Paseo 报告](server-paseo-tests.md)。
- 将 SDK 依赖明确指向本地 workspace，补齐构建顺序和测试依赖；加入浏览器恢复可见、
  聚焦和系统配色变化时的主题同步。
- 将现有 `logo.svg` 字母 A 图形接入 App、favicon、启动页及 Electron 资源；归档 ADR-051
  的飞鸟设计方向及素材。飞鸟生产稿与客户端验收仍待实施。
- 保留此前各阶段的实施报告、测试来源清单和覆盖率 JSON。历史报告中的测量对应各自
  声明的 revision/源码指纹，本次验证结果单独列在下方。

`output/imagegen/ait-day-concepts-20260926/` 的 9 份文件与正式品牌归档重复，已从提交中
排除并加入 `.gitignore`，本地文件保留。正式归档中的提示词末尾多余空行已清理。
构建目录、本地数据库、日志和凭据文件不进入提交。

## 验证

- Rustfmt、严格 workspace/all-targets Clippy、`git diff HEAD --check` 通过。
- 普通 workspace：**1,695 passed / 1 failed / 8 ignored**，包含 1 项通过的 doctest。
  唯一失败是未修改的 `command_approval_secrets_never_reach_durable_or_reconnected_views`
  在 3 秒审批等待处超时；完整 `native_approvals` 目标复验 **9 passed / 0 failed**。
  未增加等待时限，也未将首轮记为全量通过。
- 新采样的 instrumented workspace：**1,695 passed / 0 failed / 8 ignored**，包括上述
  审批测试。覆盖率运行不 instrument doctest；测试数与行覆盖率分开记录。
- `npm run test:sdk`：76 个文件、**969 项通过**（protocol 706、client 216、relay 47）；
  主题同步、外观应用和样式边界三个测试文件共 **22 项通过**。
- SDK 三个包、App、Electron 的类型检查通过；`npm run build:paseo` 和 Electron renderer
  的 Expo Web 导出通过。修改的 7 个源码文件 Oxfmt 通过，SDK 与修改源码共 170 个文件
  Oxlint 检查通过；npm lockfile 离线安装规划检查通过。
- 品牌 PNG 尺寸/签名、ICNS/ICO 容器头、修改文档的相对链接检查通过。
  未重新执行整个 App 单元套件、真实浏览器主题时序、原生平台图标遮罩或打包安装验收。

Rust 精确命令：

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --offline -- -D warnings
cargo test --workspace --offline --no-fail-fast -- --test-threads=1
cargo test -p ait-application --offline --test native_approvals -- --test-threads=1
cargo llvm-cov clean --profraw-only
cargo llvm-cov --workspace --no-clean --html --offline --no-fail-fast -- --test-threads=1
cargo llvm-cov report --json --summary-only --output-path /tmp/ait-consolidation-coverage-summary.json
```

前端主要命令：

```sh
npm run test:sdk
npm exec --workspace=@getpaseo/app -- vitest run --project unit src/appearance/system-theme-sync.test.ts src/appearance/apply.test.ts src/components/appearance-style-boundary.test.ts
npm run build:paseo
npm run typecheck --workspace=@getpaseo/protocol --workspace=@getpaseo/client --workspace=@getpaseo/relay --workspace=@getpaseo/app --workspace=@getpaseo/desktop
EXPO_NO_TELEMETRY=1 EXPO_OFFLINE=1 PASEO_WEB_PLATFORM=electron npm exec --workspace=@getpaseo/app -- expo export --platform web --output-dir ../../.tmp/consolidation-web
npm ci --ignore-scripts --offline --dry-run --no-audit --no-fund
```

## Test coverage

本次全 workspace 行覆盖率 **85.2425%（57,554 / 67,518 行）**。
相比 main 中 PR #109 的历史同平台基线 **84.5388%（56,991 / 67,414）**，
提高 **0.7036 个百分点**；该基线未重测，源码分母也有变化。

| 范围 | 覆盖行 / 总行 | 行覆盖率 |
| --- | ---: | ---: |
| bins/server | 776 / 827 | 93.83% |
| crates/server-api | 1,049 / 1,093 | 95.97% |
| crates/server-browser | 707 / 726 | 97.38% |
| crates/server-filesystem | 7,519 / 8,566 | 87.78% |
| crates/server-metadata | 5,889 / 6,609 | 89.11% |
| crates/server-model | 296 / 310 | 95.48% |
| crates/server-provider | 14,403 / 15,547 | 92.64% |
| crates/server-schedule | 802 / 820 | 97.80% |
| crates/server-terminal | 1,289 / 1,436 | 89.76% |
| crates/server-voice | 1,218 / 1,275 | 95.53% |

测量 revision：`42fca92b0de4ba1ffe57849658eea1e860200487` 加本次工作区；采样前后的 883 份
Rust/Cargo/Python 输入文件 SHA-256 集合指纹为
`8df802b7f445c382b25a06c71829b7b376710cfeefa10c119de55a2d5a1fa025`，复核未变。
范围为 macOS arm64、Rust 1.98.1、默认 features 的完整 Cargo workspace；使用
cargo-llvm-cov 0.8.4 的默认源码过滤，没有额外排除文件。先清理旧 profraw，`--no-clean`
仅复用编译缓存，没有混入旧测量的采样数据。默认过滤仍可能计入测试辅助模块。

[共享覆盖率 JSON](local-workspace-consolidation-coverage.json)包含逐文件/逐 crate 统计、
源码哈希、基线、测试执行计数、忽略测试名称及日志哈希；HTML 已生成在本地
`target/llvm-cov/html/index.html`。可审查 artifact 是提交中的 JSON，不依赖本地 HTML。

8 个原有 opt-in 测试保持忽略，涉及已安装/认证 CLI、付费模型或外部 worker；本轮没有
访问真实模型推理或 OAuth 额度服务。Linux/Windows 行覆盖率未测量；TypeScript 行覆盖率
**not measured**，本轮执行了测试但未启用 JS 覆盖率采集，后续需要单独运行相应覆盖率工具。

剩余空白包括平台专有进程/凭据/PTY 分支、文件系统及持久化 I/O 故障、Directory/Forge RPC
和图标存储的部分解析分支。后续应增加对应故障 fixture，并在 Linux/Windows 和真实平台
上验证；本轮覆盖率不代表真实模型服务、操作系统自动主题切换或图标视觉验收已完成。
