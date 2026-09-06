# WF-04：判断运行状态、取消并继续

用户目标：知道任务是否完成，遇到等待或失败时能有明确下一步，不遗失已有消息。
前置条件：完成 WF-01。以下 manual 模式专门保留非终态 Run，便于无模型演练取消操作。

## 操作

```bash
ait command '{"type":"register_agent","id":"agent-manual","name":"手动运行演练","model":"deterministic-v1","mode":"manual"}'
ait command '{"type":"create_session","id":"s-manual","project_id":"p1","agent_id":"agent-manual"}'
ait command '{"type":"send_message","session_id":"s-manual","text":"启动一次可取消运行","expected_version":1}' \
  | tee "$WF_ROOT/manual-run.json"
RUN_ID="$(jq -r '.result.value.id' "$WF_ROOT/manual-run.json")"
ait command "$(jq -nc --arg id "$RUN_ID" '{type:"get_run",run_id:$id}')"
ait command "$(jq -nc --arg id "$RUN_ID" '{type:"cancel_run",run_id:$id}')"

SESSION_VERSION="$(ait snapshot | jq -r '.result.value.sessions[] | select(.id=="s-manual") | .version')"
ait command "$(jq -nc --argjson version "$SESSION_VERSION" \
  '{type:"set_session_agent",session_id:"s-manual",agent_id:"agent-echo",expected_version:$version}')" \
  | tee "$WF_ROOT/rebound.json"
SESSION_VERSION="$(jq -r '.result.value.version' "$WF_ROOT/rebound.json")"
ait command "$(jq -nc --argjson version "$SESSION_VERSION" \
  '{type:"send_message",session_id:"s-manual",text:"继续处理",expected_version:$version}')"
```

## 验收与恢复

| Agent 模式/操作 | Run 状态 | 可观察结果与下一步 |
| --- | --- | --- |
| `manual` 首次输入 | `queued` | Session 绑定该 Run；可查询或取消，当前没有执行推进入口 |
| `provider_failure` 首次输入 | `failed` | `run.error.code=PROVIDER_FAILED`，`retryable=true`，Session 已释放；检查失败原因后可发起新输入 |
| `approval_required` 首次输入 | `waiting_approval` | `run.error.code=TOOL_APPROVAL_REQUIRED`，Session 仍被占用；当前 CLI 可取消 |
| 取消 queued/waiting Run | `cancelled` | `run.error.code=RUN_CANCELLED`，释放 Session，保留已写入消息 |
| 再次取消同一个终态 Run | 不变 | 业务拒绝 `RUN_ALREADY_TERMINAL`，退出码 2 |

前四类命令均可能返回退出码 0、`ok=true`，因为它们成功返回了一个 Run；必须再读 Run 的
`status` 和 `error`。`retryable=true` 描述错误性质，不表示 CLI 已自动重试，也不提供 resume 命令。

当前活动 Session 收到再次输入或改绑请求时返回 `SESSION_BUSY`，快照不变。
ADR 期望运行中的新输入进入同一 Run 队列；这是待实现差距。
审批通过、重试恢复和队列消费尚未由这组 CLI 流程实现或验收。

自动化：[`wf04_observe_failure_and_cancel_active_run`](../bins/cli/tests/workflows.rs)，
覆盖三种确定性非成功运行模式、busy 拒绝、取消释放、重复取消与取消后重新交互。
