# WF-03：从历史开分支、命名和切换 Agent

用户目标：保留原 Session 的进度，从选定历史节点探索另一个方向，或在空闲时更换执行者。
前置条件：完成 WF-01，`ROOT_ID` 已从注册响应获取。Codex 当前仅支持从 Project 根创建独立任务，或在 derive 可复用当前原生 Session 时继续；原生历史 fork/steer 待实现。API Provider 可从其支持的同 Project Message 派生。

## 操作

```bash
ait session create --id s-open --project-id p1 --agent-id agent-demo --at-message-id "$ROOT_ID"
ait session set-title --session-id s-open --title '临时分支标题'
ait session rename --session-id s-open --name '  我的   分支  '
ait agent create --id agent-alternate --name '另一个执行者' --provider-id builtin-codex --model gpt-5.6-sol --reasoning-effort medium
ait session set-agent --session-id s-open --agent-id agent-alternate
ait session send --session-id s-open --text '沿这个方向继续'

ait session fork --id s-fork --project-id p1 --agent-id agent-demo \
  --at-message-id "$ROOT_ID" --text '从这里提出另一个方案'
ait session list --project-id p1
ait message list --project-id p1
ait run list --project-id p1
```

需要由 daemon 原子决定复用当前叶子或从历史分叉时，使用：

```bash
ait session derive --id s-derived --project-id p1 --source-session-id s-main \
  --agent-id agent-demo --at-message-id "$ROOT_ID" --text '从选定节点继续'
```

`derive` 返回 Run；从 `result.value` 检查 `status` 和 `error`。

## 验收与失败恢复

- `create_session` 只新增一个引用，不新增或复制 Message；原 Session 不移动。
- `rename_session` 保存成员名称，规范化多余空白为 `我的 分支`；不改变指针或 pointer version。
  `set_session_title` 设置临时展示标题，不替代成员名称。AI 元数据生成不属于本流程。
- 空闲时 `set_session_agent` 以预期版本改绑，version 从 1 增到 2，当前 Message 不变；
  后续 Run 使用新的 Agent，之前的 Run 与 Message 不改写。
- `fork_session` 原子创建新 Session、追加第一条输入并启动 Run，返回值为 **Run**。
  Codex 在新原生 Thread 中发布输入；原 Session 仍留在原位置。
- Codex 非 Project 根的分支返回 `CODEX_FORK_BOUNDARY_UNSUPPORTED`；其他跨 Project 基点校验返回 `SESSION_MESSAGE_PROJECT_MISMATCH`，不留下半个分支。
  选择该 Message 所属 Project，或回到当前 Project 的历史节点。
- 旧版本改绑返回 `SESSION_POINTER_CONFLICT`，应重新读取；活动时改绑返回 `SESSION_BUSY`，
  需要等待 Run 结束或按 WF-04 取消。不要覆盖别的客户端的新选择。

自动化：[`wf03_branch_rename_and_rebind_session`](../bins/cli/tests/workflows.rs)，
校验从 Project 根创建独立任务、命名与同 Provider 改绑、历史逐项不变和非法分支失败原子性。
绑定原生 Thread 后不允许切换 Provider；同 Provider 的模型/Agent 配置仍可在空闲时更新。
