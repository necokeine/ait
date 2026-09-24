# 独立 server 的 AgentSession 与 AgentManager 首个切片

- 日期：2026-09-23；分支：`new`；代码基线：`abbbf00`。
- Paseo 对照：`getpaseo/paseo@2c8e8a826810337492cc5a38bb0bbd705b6fb632`。
- 决策：[ADR-027](../decisions/adr-027-independent-agent-session-manager.md)。

## 已实现

新 `server-ports::agent_session` 建立 `AgentClient` 与 `AgentSession` 的异步抽象，包含 Provider
可用性、创建、恢复、runtime info、persistence handle 和关闭。没有依赖旧 Ait crate。

新 `server-application::agent_manager` 管理已登记的 Provider client 与 live session：

- 创建前检查 ID 与 durable record 冲突，检查 Provider 可用性；Provider session 建立后，验证
  runtime info / provider / persistence identity，再把 Paseo-shaped Agent snapshot 写入 registry。
- 恢复从最新 durable record 取配置和 handle；active Agent 用 `interactive`，archived Agent 用
  `history`。恢复时保留原有 created/updated/activity/attention/archive 元数据。
- 原生关闭失败时保留 live 所有权以供重试；注册失败会尝试关闭未登记 session，若清理也失败则保留
  进程内所有权，且禁止把它当作成功恢复。关闭成功后 durable status 变为 `closed`；`close_all`
  可以在将来的 host 停机流程中使用。

## 仍未对齐 Paseo

当前没有具体 Provider client，因此 binary 尚未组装 manager，`agent.create/resume/send/wait/cancel`
等 WebSocket 方法仍返回 `not_implemented`，`implemented_capabilities` 未增加。`AgentSession`
目前只覆盖会话所有权，不含 Paseo 的 foreground turn、steer、event subscription、history stream、
permission、mode/model/thinking/feature、rewind、timeline hydration、plugin hook 或并发 lifecycle lane。
没有 handle 的 durable Agent 会明确拒绝恢复；Paseo 的 loader 在部分情况下会为它创建首个会话。
下一步应先接入一个真实 Provider adapter，再贯通 turn 与事件持久化，最后公开对应 WebSocket 能力。

## 测试执行

新增 11 个 manager 单元测试，覆盖创建、重复 ID、Provider 不可用、runtime/handle 校验、registry
失败清理、关闭失败重试、归档 history 恢复、active interactive 恢复、无 handle 拒绝、重复 Provider
登记和批量关闭。Provider 与 registry 均使用独立假实现，没有网络或用户凭据。

`cargo fmt --all -- --check`、`cargo build --workspace`、
`cargo clippy --workspace --all-targets -- -D warnings` 均通过。
`cargo test --workspace --no-fail-fast -j1` 通过，包括新 server 的 API、application、binary
WebSocket、storage 与 workspace 套件。覆盖率运行的 72 个非 doctest target 共 806 passed、0 failed、
5 ignored；5 个跳过项依赖真实登录、付费 API 或额外 worker 可执行文件。

## Test coverage

测量命令：

```text
CARGO_TARGET_DIR=/tmp/ait-phase10-cov-target cargo llvm-cov --workspace --json \
  --summary-only --output-path /tmp/ait-agent-manager-coverage-raw.json \
  --no-fail-fast -j1
CARGO_TARGET_DIR=/tmp/ait-phase10-cov-target cargo llvm-cov report --html
```

测量修订为 `abbbf00` 加本报告对应的 `new` 分支工作树修改；范围为整个 Cargo workspace、
默认 features、生产 Rust 源码；`cargo-llvm-cov` 默认排除 test/build 源文件，未显式排除生产文件；
覆盖率运行不含 doctest，Linux/Windows 未在本机验证。workspace 行覆盖率为
**37,995/47,012（80.82%）**，相比[层级路由基线](server-websocket-hierarchical-routing-coverage.json)
的 37,787/46,763（80.81%）提高约 0.01 个百分点。新 server packages 合计为
14,567/16,881（86.29%，基线 86.20%）；`server-application` 为 2,968/3,371（88.05%），
新增 `agent_manager.rs` 为 230/249（92.37%）。

可审查的分项与基线数据保存在
[coverage artifact](independent-agent-session-manager-coverage.json)；本地 HTML 位于
`/tmp/ait-phase10-cov-target/llvm-cov/html/index.html`。目前尚未覆盖的 manager 分支主要包括
无 live session 的关闭、`close_all` 的部分失败、错误 Provider handle 和持久化失败后的再次关闭
失败；这些需要在接入真实 Provider 与 host 生命周期时补上。
