# ADR-027：独立 AgentSession 与 AgentManager 生命周期边界

- 状态：Accepted。
- 日期：2026-09-23。
- 来源：`getpaseo/paseo@2c8e8a826810337492cc5a38bb0bbd705b6fb632` 的
  `agent-sdk-types.ts`、`agent-manager.ts` 和 `agent-loading.ts`。
- 范围：新 `server-*` crate；旧 Ait daemon、Session、Agent 和 Provider 组件不参与实现。

## 背景

ADR-026 第六阶段只建立 Paseo Agent 的 durable snapshot、目录和元数据操作。它没有管理原生
Provider session：`agent.create/resume/send/cancel` 仍是 WebSocket 占位方法。目录记录与运行中的
Provider 对象不能混为一谈；恢复 archived Agent 时，Paseo 还要求使用只读 history purpose。

## 决策

`server-ports::agent_session` 定义独立 `AgentClient` 与 `AgentSession` 边界。当前只抽取创建、恢复、
可用性、runtime info、persistence handle 和关闭所需的方法，异步操作使用可发送的 boxed future。
Provider adapter 必须持有原生资源；`close` 只释放当前进程的资源，不删除原生历史。

`server-application::agent_manager::AgentManager` 持有已注册 Provider client、活跃 session 与
`AgentRuntimeRegistry`。创建前检查 Agent ID 没有 live/durable 冲突，先检查 Provider 可用性，再启动
session；runtime info、Provider 名称与 resume handle 一致后才写入 Paseo-shaped snapshot 并公开为
live Agent。注册失败时尝试关闭未登记 session；关闭失败则保留进程内所有权，供稍后重试，避免
遗失可能仍持有 writer 的原生对象。

恢复从最新 durable record 取 handle 和 config。未归档记录使用 `interactive`，已归档记录使用
`history`；恢复不更新时间、activity 或 attention。缺失 handle 直接报错，不把恢复请求伪装为
新建会话。关闭成功后把 durable status 写为 `closed`；关闭失败保持 live 所有权。`close_all`
用于宿主后续接入停机顺序。

本阶段没有具体 Provider adapter、event/timeline store、foreground turn、permission 或 WebSocket
执行入口。`server` binary 暂不组装 manager，也不改变 `implemented_capabilities`；这些方法必须在
真实 adapter 和请求生命周期接通后才公开。现有 Agent directory 仍只报告 durable snapshot，
不推断 Provider 可用性。

现有 `AgentRuntimeRegistry` 是阻塞 port；后续在 Tokio host 中组装 manager 时，必须把这些调用放在
专用 worker 或阻塞任务中，并确保停机前调用 `close_all`。不能直接在 WebSocket reactor 上执行文件 I/O。

## 与 Paseo 的差异

Paseo 的 AgentSession 接口还包含 `run/startTurn/steerActiveTurn`、订阅与历史流、mode、permission、
model、thinking、feature、rewind 等。当前 port 仅覆盖第一段 session 所有权生命周期；没有
Paseo 的 event tail、timeline hydration、并发 lifecycle lane、plugin hook 或 Provider catalog。
Paseo 对没有 handle 的历史记录可能创建首个 session；本阶段明确拒绝此情况，避免在用户要求恢复
时意外启动新的交互式 writer。后续接入 Provider 时应扩展 port，并重新验证归档和停机并发语义。
