# WF-03：从历史开分支、命名和切换 Agent

用户目标：保留原 Session 的进度，从选定历史节点探索另一个方向，或在空闲时更换执行者。
前置条件：完成 WF-01，`ROOT_ID` 已从注册响应获取。任意同 Project 的 Message 都可替换该基点。

## 操作

```bash
ait command "$(jq -nc --arg at "$ROOT_ID" \
  '{type:"create_session",id:"s-open",project_id:"p1",agent_id:"agent-demo",at_message_id:$at}')"
ait command '{"type":"set_session_title","session_id":"s-open","title":"临时分支标题"}'
ait command '{"type":"rename_session","session_id":"s-open","name":"  我的   分支  "}'
ait command '{"type":"register_agent","id":"agent-alternate","name":"另一个执行者","config":{"provider_id":"builtin-tool","model":"default"}}'
ait command '{"type":"set_session_agent","session_id":"s-open","agent_id":"agent-alternate"}'
ait command '{"type":"send_message","session_id":"s-open","text":"沿这个方向继续"}'

ait command "$(jq -nc --arg at "$ROOT_ID" \
  '{type:"fork_session",id:"s-fork",project_id:"p1",agent_id:"agent-demo",at_message_id:$at,text:"从这里提出另一个方案"}')"
ait snapshot
```

## 验收与失败恢复

- `create_session` 只新增一个引用，不新增或复制 Message；原 Session 不移动。
- `rename_session` 保存成员名称，规范化多余空白为 `我的 分支`；不改变指针或 pointer version。
  `set_session_title` 设置临时展示标题，不替代成员名称。AI 元数据生成不属于本流程。
- 空闲时 `set_session_agent` 以预期版本改绑，version 从 1 增到 2，当前 Message 不变；
  后续 Run 使用新的 Agent，之前的 Run 与 Message 不改写。
- `fork_session` 原子创建新 Session、追加第一条输入并启动 Run，返回值为 **Run**。
  新输入 parent 为指定基点，原 Session 仍留在原位置。
- 跨 Project 的 Message 返回 `SESSION_MESSAGE_PROJECT_MISMATCH`，不留下半个分支。
  选择该 Message 所属 Project，或回到当前 Project 的历史节点。
- 旧版本改绑返回 `SESSION_POINTER_CONFLICT`，应重新读取；活动时改绑返回 `SESSION_BUSY`，
  需要等待 Run 结束或按 WF-04 取消。不要覆盖别的客户端的新选择。

自动化：[`wf03_branch_rename_and_rebind_session`](../bins/cli/tests/workflows.rs)，
同时校验从已完成 assistant 节点打开 Session、从根 fork、历史逐项不变、跨项目失败原子性。
