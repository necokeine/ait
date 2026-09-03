# AIT

AIT 是一个本地优先的多 Agent 管理器，目标是统一在线协作平台、本地 Agent 运行时和面向任务的管理界面。

当前仓库处于工程初始化阶段，实现语言固定为 Rust。核心概念与边界以 `docs/README.md` 中列出的 ADR 为准。

## 开始开发

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## Workspace

- `crates/domain`：纯领域模型与不变量。
- `crates/contracts`、`crates/ports`：进程无关契约与端口。
- `crates/application`：用例编排。
- `crates/project-local`：Project 路径边界、指令文件读取与本地 Git 适配器。
- `crates/runtime`、`crates/scheduler`：Run 与调度生命周期。
- `crates/storage-sqlite`：SQLite 持久化适配器。
- `crates/providers`：统一 Provider 契约、契约测试工具、Mock 与 OpenAI-compatible Adapter。
- `crates/agent-adapters`：完整 Agent harness 适配器；首个实现为 Codex app-server。
- `crates/tools`、`crates/sandbox`：工具与进程隔离适配器。
- `crates/ipc`、`crates/api-http`：传输层。
- `bins/daemon`、`bins/worker`、`bins/cli`：可执行入口。

更完整的依赖方向见 `docs/decisions/NEC-154/adr-002-rust-workspace-runtime-architecture.md`。
