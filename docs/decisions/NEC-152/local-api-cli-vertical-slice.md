## NEC-152 本地 API/CLI 纵向切片

该切片把 Project、Agent、Session、Message、Run 和 Cron 的可执行路径接到同一个
`LocalControlService`。HTTP 与 CLI 只是传输适配器，不各自实现领域规则。

### 运行

```bash
cargo run -p ait-daemon -- --database ./ait.sqlite3 --listen 127.0.0.1:7314
cargo run -p ait-cli -- snapshot
```

CLI 的 `command` 子命令接受版本一的 JSON command。例如注册 Agent：

```bash
cargo run -p ait-cli -- command \
  '{"type":"register_agent","id":"agent-1","name":"Demo","model":"deterministic-v1","mode":"tool"}'
```

完整命令集合由 `ait-contracts::Command` 定义，包括：

- `register_project`：规范化目录，在目录不是独立 Git root 时执行并验证 `git init`；
- `register_agent`：选择固定 revision 的 Agent；
- `create_session`：在 Project root 或任意已有 Message 上创建分支 Session；
- `send_message`、`get_run`、`cancel_run`：交互与 Run 生命周期；
- `create_cron`、`set_cron_enabled`、`trigger_cron`：持久化 Cron、启停与幂等 occurrence 触发；
- `snapshot`：从 SQLite 恢复完整最终投影。

`AgentMode::Tool` 是不依赖凭据的验收驱动：它按顺序持久化 assistant ToolUse、user
ToolResult 和最终 assistant Message。`Manual` 留下可取消 Run；`ProviderFailure` 与
`ApprovalRequired` 用于稳定错误路径。

### API 与事件恢复

- `POST /v1/commands` 接受 `Command` 并返回统一 `Response`。
- `GET /v1/events?after=<cursor>&limit=<n>` 返回 SSE。

状态快照和 durable event outbox 在同一 SQLite 事务提交。事件 cursor 单调递增；连接
断开后用最后收到的 SSE `id` 作为 `after` 即可无损续读。实时流不是最终状态权威，客户端
始终可用 `snapshot` 或 `get_run` 从持久化数据恢复。

### 稳定错误

切片明确覆盖 `INVALID_AGENT_CONFIGURATION`、`PROVIDER_FAILED`、
`TOOL_APPROVAL_REQUIRED`、`SESSION_POINTER_CONFLICT`、`RUN_CANCELLED`，以及终态取消时的
`RUN_ALREADY_TERMINAL`。错误通过 API、CLI 与 durable Run 投影保持同一 wire code。

### 验收测试

`crates/application/tests/control_plane.rs` 演示 tool Session、从 root 切分支、Cron Run、
cursor 分页重连与关闭/重开 SQLite 后恢复；`crates/api-http/tests/http.rs` 验证 HTTP command
和 SSE 使用同一 application service。
