# AIT

AIT 是一个本地优先的多 Agent 管理器，目标是统一在线协作平台、本地 Agent 运行时和面向任务的管理界面。

当前仓库处于工程初始化阶段，实现语言固定为 Rust。核心概念与边界以 `docs/README.md` 中列出的 ADR 为准。

## 开始开发

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

cd apps/desktop
npm install
npm run dev
```

## 本地 API 与 CLI

```bash
cargo run -p ait-daemon -- --database ./ait.sqlite3
cargo run -p ait-cli -- --help
cargo run -p ait-cli -- project list
cargo run -p ait-cli -- project register --id demo --name Demo --workdir /path/to/project
cargo run -p ait-cli -- agent create --id codex --name Codex --provider-id builtin-codex --model gpt-5.6-sol
cargo run -p ait-cli -- session create --id main --project-id demo --agent-id codex
cargo run -p ait-cli -- session send --session-id main --text-stdin < /path/to/prompt.txt
```

`--database` 指定全局目录数据库；对话、Session、Run 和进度保存在各项目的
`.metafab/project.sqlite3`，并自动通过 Git `info/exclude` 排除。旧单文件库首次打开时
先生成 `*.pre-split.sqlite3` 备份再迁移，迁移时所有已注册项目目录必须可访问。
备份需要同时考虑全局库和项目库，详见 [存储与迁移约定](docs/decisions/NEC-235/adr-001-global-and-project-storage.md)。

daemon 按实体与操作暴露本地 HTTP API，例如 `POST /v1/project/register`、
`POST /v1/session/create`；可按 cursor 续读的 SSE 位于 `GET /v1/event/list`。CLI 通过
同一组 API 调用 application service。完整路由表见
`docs/decisions/NEC-166/entity-operation-http-api.md`。

按用户目标组织的操作步骤、失败恢复和当前行为差距见 [CLI 用户流程](workflows/README.md)。
对应验收测试运行 `cargo test -p ait-cli --test workflows`，覆盖真实 CLI 到 HTTP/SQLite 的完整路径。

CLI 按 `project`、`agent-provider`、`agent`、`session`、`message`、`run`、`cron`、`settings`、`event`
分组，每一级都提供 `--help`；标量通过 flags 输入，多行文本支持 `--text-file` / `--text-stdin`。
Provider 凭据使用 `agent-provider save --secret-stdin`，不得粘贴到命令行。
`session send` 返回后必须检查 Run 的 `status` 与 `error`，`ok=true` 不代表执行完成。

权限默认是 `permissions.sandbox=read_only`、`permissions.approval=on_request`。Codex 写代码前应按
[WF-08](workflows/08-settings.md#在代码写入前设置权限) 读取 settings 的最新 revision，用
`settings set --expected-revision <revision> --input <完整values文件>` 保存 `workspace_write`，然后发送输入。
新 Run 固定权限快照，仍受管理员上限约束；普通 API Provider 当前只返回文本，不执行文件工具。
CLI 边界决策见 [NEC-241 ADR](docs/decisions/NEC-241/adr-001-entity-cli.md)。

GitHub Release 会为 Linux x86_64 与 Apple Silicon 构建名为 **Ait desktop** 的桌面产物；
版本准备、打标签、产物校验和故障恢复见 [发布操作指南](docs/operations/releasing.md)。

Project 的无凭证 JSON 归档使用 `ait-cli project export` / `ait-cli project import`；结构化指标位于
`GET /v1/metric/list`。备份恢复、数据保留、附件清理与性能基准见
`docs/operations/reliability-security-observability.md`。

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
- `apps/desktop`：Electron 桌面工作台；main process 只通过 daemon 的本地 API 读写，renderer 只消费受限投影。

更完整的依赖方向见 `docs/decisions/NEC-154/adr-002-rust-workspace-runtime-architecture.md`。
