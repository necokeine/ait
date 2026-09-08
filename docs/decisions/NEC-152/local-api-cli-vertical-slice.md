## NEC-152 本地 API/CLI 纵向切片

> NEC-203 已移除本切片最初使用的 Tool、Manual、ProviderFailure 与 ApprovalRequired
> built-in Provider；确定性验收改为从测试端口注入，生产目录只保留真实执行适配器。

该切片把 Project、Agent、Session、Message、Run 和 Cron 的可执行路径接到同一个
`LocalControlService`。HTTP 与 CLI 只是传输适配器，不各自实现领域规则。

### 运行

```bash
cargo run -p ait-daemon -- --database ./ait.sqlite3 --listen 127.0.0.1:7314
cargo run -p ait-cli -- project list
```

CLI 的 `command` 子命令接受版本一的 JSON command。例如注册 Agent：

```bash
cargo run -p ait-cli -- command \
  '{"type":"register_agent","id":"agent-1","name":"Demo","config":{"provider_id":"builtin-codex","model":"gpt-5.6-sol","reasoning_effort":"high"}}'
```

完整命令集合由 `ait-contracts::Command` 定义，包括：

- `register_project`：规范化目录，在目录不是独立 Git root 时执行并验证 `git init`；
- `register_agent`：选择固定 revision 的 Agent；
- `create_session`：在 Project root 或任意已有 Message 上创建分支 Session；
- `send_message`、`get_run`、`cancel_run`：交互与 Run 生命周期；
- `create_cron`、`set_cron_enabled`、`trigger_cron`：持久化 Cron、启停与幂等 occurrence 触发；
- `export_project`、`import_project`：版本化导出/原子导入无凭证 Project archive；
- `project list`、`agent list`、`agent-provider list`、`session list`、`message list`、
  `run list`、`cron list`：按实体或 Project 范围读取最终投影（NEC-224 修订）；底层仍复用相同 Command/API。

生产命令不再通过 Provider kind 构造工具、排队、失败或审批状态。ToolUse/ToolResult 与审批恢复由
runtime 的 scripted ports 覆盖；Provider 失败、queued checkpoint 与取消由 application 测试向
executor/store seam 注入。HTTP/CLI 测试同样注入确定性 `WorkspaceAgent`，不会把 fake 注册进目录。

### API 与事件恢复

- HTTP API 已由 NEC-166 改为按实体与操作拆分的路由；完整映射见
  `docs/decisions/NEC-166/entity-operation-http-api.md`。
- `GET /v1/event/list?after=<cursor>&limit=<n>` 返回 SSE。
- `GET /v1/metric/list` 返回带 project/session/run/call 关联字段的进程内计数指标。

实体记录变更和 durable event outbox 在同一 SQLite 事务提交。事件 cursor 单调递增；连接
断开后用最后收到的 SSE `id` 作为 `after` 即可无损续读。实时流不是最终状态权威，客户端
始终可用对应实体 list command 或 `get_run` 从持久化数据恢复。

### 稳定错误

切片明确覆盖 `INVALID_AGENT_CONFIGURATION`、真实执行器返回的 `PROVIDER_FAILED`、
`SESSION_POINTER_CONFLICT`、`RUN_CANCELLED`，以及终态取消时的 `RUN_ALREADY_TERMINAL`。
`TOOL_APPROVAL_REQUIRED` 由 runtime 的真实审批状态机覆盖。错误通过 API、CLI 与 durable Run
投影保持同一 wire code。

### 验收测试

`crates/application/tests/control_plane.rs` 演示注入 executor 的 Codex Session、从 root 切分支、Cron Run、
cursor 分页重连与关闭/重开 SQLite 后恢复；`crates/api-http/tests/http.rs` 验证各实体操作
路由和 SSE 使用同一 application service。
