# WF-02：发送输入并查看 Agent 最终结果

用户目标：在 Session 中提交一次任务，确认输入保存、Agent 结果可追溯并可以继续交互。
前置条件：完成 WF-01，Project 的 Git index/worktree 干净，包括没有未跟踪文件。

## 操作

```bash
ait command '{"type":"create_session","id":"s-codex","project_id":"p1","agent_id":"agent-demo"}'
ait command '{"type":"send_message","session_id":"s-codex","text":"检查并总结当前项目"}' \
  | tee "$WF_ROOT/codex-run.json"
RUN_ID="$(jq -r '.result.value.id' "$WF_ROOT/codex-run.json")"
ait command "$(jq -nc --arg id "$RUN_ID" '{type:"get_run",run_id:$id}')"
ait snapshot > "$WF_ROOT/after-run.json"
jq '.result.value | {sessions,messages,runs}' "$WF_ROOT/after-run.json"
```

## 验收

输入产生普通 user Message，`git_commit` 为发送时干净的完整 HEAD。普通文本回复的路径为：

```text
System root → user input → assistant final
```

Run 返回 `status=completed`，`last_message_id` 对应最后的 assistant；Session 指向同一节点，
`active_run_id=null`。Run 固定本次 Agent revision。

当真实 runtime 产生工具调用时，ToolUse 仍属于 assistant sub-message，ToolResult 仍是特殊 user
Message；该协议由 runtime 的 scripted tool 测试覆盖，不再通过测试型 built-in Provider 伪造。

发送不再提供 version。daemon 在发送前独占 Session；有 active Run 时立即拒绝新消息。

## 失败与恢复

| 条件 | 当前结果 | 用户下一步 |
| --- | --- | --- |
| Project 中有未提交或未跟踪文件 | `PROJECT_GIT_DIRTY` | 检查 Git diff，按自己的工作意图整理或提交，再发送 |
| Session 有活动 Run | `SESSION_BUSY` | 等待完成或显式取消后再发送 |
| 为模型保存不支持的推理等级 | `INVALID_AGENT_CONFIGURATION` | 按 Provider 模型目录选择等级 |

上述拒绝不得留下输入 Message、Run 或移动后的 Session，不会等待占用释放后自动重发输入。
配置与推理强度见 [ADR-009](../docs/decisions/adr-009-session-exclusion-and-agent-providers.md)。

自动化：[`wf02_send_message_and_inspect_agent_reply`](../bins/cli/tests/workflows.rs)，
覆盖脏目录、能力不匹配、中文/引号/换行输入及 user → assistant 父子链。
