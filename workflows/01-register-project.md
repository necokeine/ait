# WF-01：接入工作目录并开始一个 Session

用户目标：把已有本地目录交给 AIT 管理，选择执行 Agent，得到可以交互的 Session。
前置条件：完成 [演练准备](README.md#手工演练准备)，使用新的数据库及已存在的 `$WF_ROOT/project`。

## 操作

```bash
ait project list
ait command "$(jq -nc --arg workdir "$WF_ROOT/project" \
  '{type:"register_project",id:"p1",name:"演练项目",workdir:$workdir}')" \
  | tee "$WF_ROOT/project.json"
export ROOT_ID="$(jq -r '.result.value.root_message_id' "$WF_ROOT/project.json")"

ait command '{"type":"register_agent","id":"agent-demo","name":"Codex","config":{"provider_id":"builtin-codex","model":"gpt-5.6-sol","reasoning_effort":"high"}}'
ait command '{"type":"set_project_default_agent","project_id":"p1","agent_id":"agent-demo"}'
ait command '{"type":"create_session","id":"s-main","project_id":"p1","agent_id":"agent-demo"}'
ait session list --project-id p1
ait message list --project-id p1
```

本例使用 host sign-in 的 `builtin-codex`；发送输入前需确保本机 Codex 可用。真实端到端生成与提交见
WF-10，DeepSeek Provider 连接见 WF-11。
`repo_url` 可选且只记录来源；注册不会自动克隆或下载仓库。

## 验收与失败恢复

- 初始 Project 列表为空；注册返回规范化绝对路径、有效 `base_commit` 和根 System Message ID。
- 若目录还不是 Git root，AIT 初始化 Git；unborn HEAD 会获得一个初始空提交。
- 新 Session 指向根 Message，`agent_id=agent-demo`、`name=""`、`version=1`、`active_run_id=null`。
  创建 Session 不复制历史。设置默认 Agent 使 Project revision 增长。
- 换一个 Project ID 重复注册同一个规范化路径，返回 `PROJECT_PATH_ALREADY_REGISTERED`；
  路径不存在返回 `PROJECT_PATH_NOT_FOUND`。先修正路径，不要创建重复记录来绕过错误。
- Agent 名称为空返回 `INVALID_AGENT_CONFIGURATION`；失败不新增 Project、Agent、Message 或 Session。

当前 `create_session` 必须显式传 `agent_id`，即使 Project 已有默认值。
后续可改善默认选择体验，但仍应保证返回的 Session 绑定明确的 Agent。

自动化：[`wf01_register_project_and_agent`](../bins/cli/tests/workflows.rs)，另覆盖含空格路径、
实际 Git HEAD 校验、重复路径拒绝后各实体列表不变，以及 Project 范围的 Session/Message 查询。
