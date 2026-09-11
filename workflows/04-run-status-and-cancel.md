# WF-04：判断运行状态、取消并继续

用户目标：知道任务是否完成，必要时取消仍在运行的任务，然后继续使用 Session。
前置条件：完成 WF-01，并准备两个终端连接同一个 daemon。

## 操作

在终端 A 启动一个耗时任务；该命令会等待 Run 到达终态：

```bash
ait session send --session-id s-main --text '执行一项足够长、便于人工取消的检查'
```

在终端 C 趁 Run 仍活动时读取 Session 的 `active_run_id`，查询并取消：

```bash
ait session list --project-id p1 > "$WF_ROOT/running-sessions.json"
RUN_ID="$(jq -r '.result.value[] | select(.id=="s-main") | .active_run_id' "$WF_ROOT/running-sessions.json")"
ait run get --run-id "$RUN_ID"
ait run cancel --run-id "$RUN_ID"
ait session send --session-id s-main --text '取消后继续处理'
```

## 验收与恢复

| 操作/结果 | Run 状态 | 可观察结果与下一步 |
| --- | --- | --- |
| Agent 正常结束 | `completed` | `last_message_id` 指向最终 assistant，Session 已释放 |
| Provider 调用失败 | `failed` | `run.error.code` 给出稳定原因；Session 已释放，可检查后发起新输入 |
| 取消非终态 Run | `cancelled` | `run.error.code=RUN_CANCELLED`，释放 Session，保留取消前已写入的消息 |
| 再次取消同一个终态 Run | 不变 | 业务拒绝 `RUN_ALREADY_TERMINAL`，退出码 2 |

发送命令可能返回退出码 0、`ok=true`，因为它成功返回了一个 Run；仍必须读取 Run 的 `status`
和 `error`。`retryable=true` 描述错误性质，不表示 CLI 已自动重试。

若任务在终端 C 读取前已经完成，`active_run_id` 会是 `null`；不要伪造 Run ID。换一个耗时任务重试，
或直接检查已完成结果。取消与模型完成发生竞态时，以 `get_run` 返回的持久化终态为准。

当前活动 Session 收到再次输入或改绑请求时返回 `SESSION_BUSY`，相关记录不变。
ADR 期望运行中的新输入进入同一 Run 队列；这是待实现差距。
队列消费仍由 runtime 测试覆盖。原生审批可以通过以下实体入口回复。

原生审批时，先用 `ait run get --run-id "$RUN_ID"` 读取 `native_approvals` 中 pending 审批的 ID、`kind` 和授权目标，
再选择一个动作（从返回值设置 `APPROVAL_ID`）：

```bash
ait run approval approve --run-id "$RUN_ID" --approval-id "$APPROVAL_ID" --scope one-shot
# 拒绝操作但继续该 turn
ait run approval deny --run-id "$RUN_ID" --approval-id "$APPROVAL_ID"
# 拒绝并取消原生 turn
ait run approval cancel --run-id "$RUN_ID" --approval-id "$APPROVAL_ID"
```

以上为互斥选择，不应依次执行。approve 必须显式提供 scope。命令/文件审批支持 `one-shot` 或 `session`；权限档案审批支持
`turn` 或 `session`。`session` scope 还受管理员策略限制，daemon 校验审批记录、Run 权限快照和授权上限。
deny/cancel 不接受 scope；回复仍属于同一 Run，不产生伪造的 ToolUse/ToolResult。

自动化：[`wf04_observe_injected_provider_failure_and_continue`](../bins/cli/tests/workflows.rs)
通过 `WorkspaceAgent` test fake 覆盖持久化 Provider 失败、Session 释放和后续交互；
`crates/application/tests/session_agent_config.rs::cancelling_an_active_call_releases_the_session_and_discards_its_output`
使用 blocking executor 覆盖活动取消。两者都不向生产 Provider 目录注册测试模式。
