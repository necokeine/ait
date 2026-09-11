# WF-01：接入工作目录并开始一个 Session

用户目标：把已有本地目录交给 AIT 管理，选择执行 Agent，得到可以交互的 Session。
前置条件：完成 [演练准备](README.md#手工演练准备)，使用新的数据库及已存在的 `$WF_ROOT/project`。

## 操作

```bash
ait project list
ait project register --id p1 --name 演练项目 --workdir "$WF_ROOT/project" \
  | tee "$WF_ROOT/project.json"
export ROOT_ID="$(jq -r '.result.value.root_message_id' "$WF_ROOT/project.json")"

ait agent create --id agent-demo --name Codex --provider-id builtin-codex --model gpt-5.6-sol --reasoning-effort high
ait project set-default-agent --project-id p1 --agent-id agent-demo
ait session create --id s-main --project-id p1 --agent-id agent-demo
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


## 仅名称新建项目（NEC-195）

CLI 可以省略 `--workdir`（HTTP 请求可省略 `workdir` 或传 `null`）：

```bash
ait project register --id new-project --name 我的项目
```

daemon 在当前用户的 Documents 下独占创建 `我的项目`，初始化 Git 和空初始提交，再原子注册
Project 与根 Message。Desktop 的 Create Project 同样允许不选目录；只填名称并选择 Backend。
显式选择已有目录仍沿用本页原流程，不会被移到 Documents。

同名目录、文件或链接已存在时返回 `PROJECT_PATH_ALREADY_EXISTS`，已有内容保持不变。
Documents 不可用时返回 `PROJECT_DEFAULT_DIRECTORY_UNAVAILABLE`，可改为明确指定已有目录。
无效目录名返回 `INVALID_PROJECT`。分配成功后若 Git 或存储失败，错误会给出保留的目录路径；
检查内容后明确选择该目录恢复注册，或改用新名称。AIT 不会自动清理或覆盖它。

自动化 `wf01_name_only_project_uses_documents_and_fails_closed` 使用注入的临时 Documents，
覆盖 CLI → HTTP → application → Git/SQLite、冲突内容保留和重启后的 Project 恢复。
