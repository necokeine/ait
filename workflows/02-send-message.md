# WF-02：发送输入并查看工具与最终结果

用户目标：在 Session 中提交一次任务，确认输入保存、工具结果可追溯、最终结果可以继续交互。
前置条件：完成 WF-01，Project 的 Git index/worktree 干净，包括没有未跟踪文件。

## 操作

```bash
ait command '{"type":"register_agent","id":"agent-tool","name":"工具演练","model":"deterministic-v1","mode":"tool"}'
ait command '{"type":"create_session","id":"s-tool","project_id":"p1","agent_id":"agent-tool"}'
ait command '{"type":"send_message","session_id":"s-tool","text":"检查工具输出","expected_version":1}' \
  | tee "$WF_ROOT/tool-run.json"
RUN_ID="$(jq -r '.result.value.id' "$WF_ROOT/tool-run.json")"
ait command "$(jq -nc --arg id "$RUN_ID" '{type:"get_run",run_id:$id}')"
ait snapshot > "$WF_ROOT/after-tool.json"
jq '.result.value | {sessions,messages,runs}' "$WF_ROOT/after-tool.json"
```

## 验收

输入产生普通 user Message，`git_commit` 为发送时干净的完整 HEAD。tool 模式的路径为：

```text
System root → user input → assistant ToolUse → user ToolResult → assistant final
```

`data.tool_use` 和 `data.tool_result` 的 `call_id` 相同。ToolResult 的 role 为 `user`、
kind 为 `tool_result`；工具调用不是第四种 Message role。
Run 返回 `status=completed`，`last_message_id` 对应最后的 assistant；Session 指向同一节点，
`active_run_id=null`。Run 固定本次 Agent revision。

继续发送前，从快照重新读取该 Session 的 `version` 作为 `expected_version`。
不要通过消息数量推算 version：释放活动 Run 也会更新版本。

## 失败与恢复

| 条件 | 当前结果 | 用户下一步 |
| --- | --- | --- |
| Project 中有未提交或未跟踪文件 | `PROJECT_GIT_DIRTY` | 检查 Git diff，按自己的工作意图整理或提交，再发送 |
| 使用过期 `expected_version` | `SESSION_POINTER_CONFLICT` | 重新读取 Session，确认新的指针后再提交 |
| 为 tool/echo 传 `reasoning_effort` | `INVALID_AGENT_CONFIGURATION` | 去掉覆盖参数；该能力只供支持的 Codex Agent 使用 |

上述拒绝不得留下输入 Message、Run 或移动后的 Session，也不能自动重试旧版本输入。
真实 Codex 的推理强度另见 [Run reasoning effort](../docs/decisions/NEC-174/adr-002-codex-run-reasoning-effort.md)。

自动化：[`wf02_send_message_and_inspect_tool_history`](../bins/cli/tests/workflows.rs)，
覆盖脏目录、版本冲突、能力不匹配、中文/引号/换行输入及完整工具父子链。
