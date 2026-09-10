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

CLI 已由 NEC-241 修订为实体子命令，例如：

```bash
cargo run -p ait-cli -- agent create --id agent-1 --name Demo \
  --provider-id builtin-codex --model gpt-5.6-sol --reasoning-effort high
cargo run -p ait-cli -- session send --session-id main --text-stdin < prompt.txt
```

完整命令面可通过各级 `--help` 发现，见 [CLI 流程](../../../workflows/README.md) 与
[NEC-241 ADR](../NEC-241/adr-001-entity-cli.md)。CLI 内部仍构造 `ait_contracts::Command`，
但外部用户不再输入 tagged transport DTO。Project 注册、Agent 配置、Session 分支、Run、
Cron、settings、归档和 durable SSE 均使用相同的 application service 与实体 HTTP API。

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
