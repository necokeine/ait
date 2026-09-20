# NEC-344 Project archive 接口移除报告

实现提交为 `2ad8c3e111c23c6b66eb72ae8b9626a7228e6fd2`，设计见
[NEC-344 ADR](../decisions/NEC-344/adr-001-remove-project-archive-interfaces.md)。

## 结果

CLI 不再公开 `project export`、`project import` 及顶层 `export`、`import`；四种写法均由
clap 在本地拒绝。HTTP router 不再注册 `POST /v1/project/export` 和
`POST /v1/project/import`，两条旧路径返回 404。

共享 contract、application read plan/transaction、archive 校验与旧格式升级、导入 Session
worktree 准备代码一并删除，避免保留不可达的第二套恢复协议。Project 恢复统一使用原项目目录中的
`.ait/project.sqlite3` 和既有目录打开流程。Codex 原生 Thread 的发现、导入和同步没有改变。

## Test coverage

以下检查全部通过：

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo build --workspace
cargo test --workspace
cargo llvm-cov --workspace --html
git diff --check
```

workspace 共发现 488 项 Rust 测试：483 项执行并通过，0 失败，5 项因需要真实模型凭据或外部
worker 配置而保持忽略。回归覆盖四种已删除 CLI 写法、两条已删除 HTTP 路由，以及现有 CLI
Command 映射与 HTTP 路由集合。

行覆盖率由 `cargo llvm-cov --workspace --html` 测得：

| 范围 | 覆盖行 / 总行 | 行覆盖率 |
| --- | ---: | ---: |
| Cargo workspace | 23,156 / 29,843 | 77.59% |
| `ait-cli` | 528 / 546 | 96.70% |
| `ait-api-http` | 713 / 976 | 73.05% |
| `ait-application` | 9,365 / 11,453 | 81.77% |
| `ait-contracts` | 488 / 589 | 82.85% |

测量范围为 macOS arm64、默认 features、Rust 1.98.1、cargo-llvm-cov 0.8.4；测量源码为
`2ad8c3e111c23c6b66eb72ae8b9626a7228e6fd2`。没有额外排除源码，工具默认不计测试源码。
[覆盖率摘要](nec-344-coverage-summary.json)保留精确计数，HTML 报告作为 ticket 附件交付。

本次是以删除代码为主的接口退役，没有重新测量相同基线的干净检出，因此不提供可能误导的因果
覆盖率差值。workspace 仍低于 80%；本次涉及的 CLI、application 和 contracts 范围均超过 80%，
HTTP adapter 为 73.05%。旧 JSON archive 不提供兼容转换，这是 ADR 明确接受的删除性边界。
