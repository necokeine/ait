## ADR-001：本地多 Agent 管理器核心领域模型

- 状态：Accepted，待实现验证
- 日期：2026-09-02
- 基线：NEC-144 的 `core-domain-model.md` 与 NEC-150 最新产品约束
- 归档说明：已由 ADR-001 v4 替代

## 1. 决策摘要

1. 一个 Project 对应一个工作目录；该目录的根必须同时是一个 Git 仓库的根。Project 还保存名称、简介、创建时间等管理元数据。
2. Session 是一棵不可变的节点树；节点的联合类型为 `Message | ToolUse | ToolResult`，而不是全部节点都叫 Message。
3. Message 的 `role` 只允许 `user | system | assistant`。System prompt 是 `role=system` 的 Message；工具调用与工具结果没有 Message role。
4. Agent 是“可运行的节点生成 API 及其版本化配置”。给定 Session 树上的一个基准节点，它持续产生并追加一条节点链，其中可包含 ToolUse、ToolResult 和 assistant Message，直到本次 Run 终止。
5. Run 是 Agent API 的一次有界执行；Session 是长期历史，二者不能合并。
6. Provider/SDK Adapter 是 Agent 的实现细节，不作为 MVP 的一级领域实体。
7. Cron 只负责确定性地产生输入并启动 Run；它不能隐式追随可变的 active head。
8. MVP 使用 SQLite 保存结构化数据和 append-only 节点，以 FTS5 建索引；大附件使用工作目录外的内容寻址文件存储。

## 2. 术语表与边界

### Project

一个 Project 是本地工作上下文、安全边界与默认配置的容器。

必备字段：

```text
Project {
  id, name, description,
  workdir,                 // 规范化后的绝对路径
  default_agent_id?,
  instruction_revision,
  metadata,                // 可扩展、非敏感、可序列化元数据
  created_at, updated_at
}
```

约束：`workdir` 必须存在，且 `git rev-parse --show-toplevel` 的规范化结果必须等于 `workdir`。一个规范化路径最多注册为一个 Project。Git remote、当前分支等属于可刷新派生信息，不是 Project 身份。默认情况下，文件工具不得越出 `workdir`；越界必须经过显式授权。

创建 Session 时，从 Project 指令源生成 System prompt 快照。Project 后续变更不回写旧 Session。

### Agent

Agent 是一个可运行、可版本化的节点生成能力。它封装 provider/model 配置、提示词片段、工具策略和能力声明，并暴露统一 API：

```text
Agent.run(
  agent_revision,
  session_path,
  tool_bridge,
  limits,
  cancellation
) -> ordered stream<ProposedNode | RunSignal>
```

宿主 Run 协调器校验并持久化每个节点；Agent 不能绕过领域规则直接修改树。ToolUse 交给宿主执行，ToolResult 持久化后再反馈给 Agent，循环直到 Agent 发出终止信号或 Run 被取消/失败/超限。

必备字段：

```text
Agent {
  id, name,
  driver_type, model,
  endpoint?, credential_ref?,
  capabilities, default_parameters, tool_policy,
  config_revision, enabled,
  created_at, updated_at
}
```

`credential_ref` 只保存引用，绝不保存明文凭证。Provider Adapter 负责把统一节点协议转换为供应商协议，不拥有 Session 或 Run。

### Session

Session 是一个 Project 内的长期对话/执行历史聚合，内部是一棵不可变节点树。

```text
Session {
  id, project_id, title,
  root_node_id, active_head_node_id,
  default_agent_id?,
  status: active | archived,
  version,
  created_at, updated_at
}
```

`SessionPath(session_id, head_node_id)` 是从根到 head 的唯一有序路径，也是 Agent 的输入上下文。`active_head_node_id` 只是 UI 默认指针；切换、编辑和重新生成均通过创建分支完成，不改写历史。

### SessionNode

树节点使用带判别字段的联合类型：

```text
SessionNode common {
  id, session_id, parent_node_id?,
  kind: message | tool_use | tool_result,
  run_id?, run_seq?,
  created_at, metadata
}

Message payload {
  role: user | system | assistant,
  content_parts,
  origin: project | human | agent | scheduler | system
}

ToolUse payload {
  call_id, tool_name, arguments,
  agent_protocol_metadata?
}

ToolResult payload {
  call_id,
  status: succeeded | failed | denied | cancelled,
  output?, error?
}
```

Message role 不扩展 `tool`。Provider 原生的 tool call/result 由 Adapter 映射为 ToolUse/ToolResult 节点。一个 provider 响应若同时包含 assistant 文本和多个工具调用，规范化为一个 assistant Message 后接若干 ToolUse；结果再按 ToolUse 的稳定顺序串行追加。工具可以并行执行，但持久化顺序必须确定。

### Run

Run 是从一个确定的 `base_node_id` 调用一个确定 Agent revision 的一次执行。

```text
Run {
  id, session_id, base_node_id, last_node_id?,
  agent_id, agent_revision,
  status, stop_reason?, error?,
  step_count, max_steps,
  token_budget?, cost_budget?, usage,
  dedupe_key?,
  started_at?, ended_at?, created_at
}
```

一个 Session 可以在不同 Run 中选择不同 Agent；不提供“修改 Session 的历史 Agent”这一操作。每个 Run 固定 `agent_revision`，其产出的节点通过 `run_id` 获得完整来源。

### ToolExecution

ToolExecution 是工具执行控制与审计记录，不属于给模型看的 Session 树。

```text
ToolExecution {
  id, run_id, call_id, tool_use_node_id,
  tool_name, arguments,
  attempt,
  approval_status,
  status,
  result_summary?, error?,
  started_at?, ended_at?, created_at
}
```

重试产生新的 ToolExecution attempt，但同一个 ToolUse 最多产生一个最终 ToolResult 节点。完整大输出可放附件存储，节点只保存受限内容与引用。

### Cron

Cron 是定时触发配置，不直接调用模型。

```text
Cron {
  id, project_id, name,
  schedule, timezone, enabled,
  target,
  agent_id?, input_template,
  concurrency_policy: allow | forbid | replace,
  misfire_policy: skip | run_once | catch_up,
  max_runtime?, next_run_at?, last_run_at?,
  created_at, updated_at
}

target =
  NewSession { session_template_id? }
  | ContinueFromNode { session_id, base_node_id }
```

每次触发先创建一个 `role=user, origin=scheduler` 的 Message，再从该节点启动 Run。`ContinueFromNode` 必须固定节点 ID；禁止使用隐式 `active_head`，从而避免调度时刻的竞态。默认 target 为 NewSession。触发幂等键为 `cron_id + scheduled_at`。

## 3. 关系图

```text
Project 1 ───── * Session 1 ───── * SessionNode
   │                 │                  │
   │                 │                  └── parent_node_id -> SessionNode
   │                 └──── * Run 1 ─────* ToolExecution
   │                              │
   │                              └── appends an ordered node chain
   ├──── default/binding ──── * Agent(revisioned)
   └──── * Cron ── trigger ──> input Message + Run

SessionNode = Message | ToolUse | ToolResult
Message.role = user | system | assistant
```

## 4. 核心不变量

1. Project 的 `workdir` 等于 Git top-level；规范化路径在系统内唯一。
2. 一个 Session 只属于一个 Project，且恰有一个根节点；根必须是 `role=system` 的 Message。
3. 一个节点只属于一个 Session；其 parent 必须属于同一 Session；节点图必须无环。
4. 节点创建后不可变。编辑旧 Message 会以旧节点的 parent 为基点创建替代分支；重新生成会以旧 assistant Message 的 parent 为 `base_node_id` 启动新 Run。
5. Message role 严格限制为 `user | system | assistant`；ToolUse/ToolResult 不能伪装成 Message。
6. Run 的首个输出节点以 `base_node_id` 为 parent；后续输出以前一个 Run 输出节点为 parent；`run_seq` 从 1 连续递增。并发 Run 因而自然形成分支。
7. ToolResult 必须引用同一 Run、当前路径上尚未完成的 ToolUse；每个 `call_id` 在 Run 内唯一，且最多有一个最终 ToolResult。
8. 每个 Agent 输出节点必须先持久化，才能开始下一模型/工具步骤；崩溃恢复只能从最后一个已提交节点继续。
9. Run 固定 Agent revision 和预算。凭证不得进入节点、日志、导出或 metadata。
10. 更新 `active_head_node_id` 必须以 Session `version` 做 compare-and-swap；冲突时保留新分支并返回冲突，不丢历史。
11. Cron 触发按 dedupe key 幂等；固定节点上的并发触发形成分支，不能静默覆盖 active head。

## 5. Run 状态机

```text
queued
  -> assembling_context
  -> calling_agent
       -> persisting_output -> calling_agent
       -> waiting_approval -> executing_tool -> persisting_output -> calling_agent
       -> executing_tool   -> persisting_output -> calling_agent
  -> completed

queued / assembling_context / calling_agent / persisting_output /
waiting_approval / executing_tool
  -> failed | cancelled | limit_exceeded
```

终止规则：Agent 产生最终 assistant Message 且无待处理 ToolUse 时为 `completed`；用户取消为 `cancelled`；预算或步数达到上限为 `limit_exceeded`；不可恢复错误为 `failed`。`waiting_approval` 可恢复到 `executing_tool`，也可因拒绝先持久化 denied ToolResult 再回到 `calling_agent`。

## 6. 最小 API 契约

```text
createProject(workdir, metadata)
updateProjectMetadata(project_id, patch)
createSession(project_id, agent_id?)
appendMessage(session_id, parent_node_id, role, content)
getSessionPath(session_id, head_node_id)
startRun(session_id, base_node_id, agent_id, limits, dedupe_key?)
cancelRun(run_id)
resumeRun(run_id)
setActiveHead(session_id, head_node_id, expected_version)
createCron(project_id, schedule, timezone, target, input_template)
```

`resumeRun` 不重复已提交节点或已完成工具调用。所有创建型接口接受可选 idempotency key。

## 7. 稳定错误语义

统一错误信封：

```text
DomainError { code, message, retryable, details?, cause_id? }
```

稳定错误码：

- Project：`PROJECT_PATH_NOT_FOUND`、`PROJECT_NOT_GIT_ROOT`、`PROJECT_PATH_ALREADY_REGISTERED`、`PROJECT_PATH_OUT_OF_SCOPE`
- Session/Node：`SESSION_NOT_FOUND`、`NODE_NOT_FOUND`、`NODE_SESSION_MISMATCH`、`INVALID_ROOT_NODE`、`INVALID_MESSAGE_ROLE`、`NODE_IMMUTABLE`、`ACTIVE_HEAD_CONFLICT`
- Agent/Run：`AGENT_NOT_FOUND`、`AGENT_DISABLED`、`AGENT_REVISION_NOT_FOUND`、`AGENT_CAPABILITY_UNSUPPORTED`、`RUN_NOT_RESUMABLE`、`RUN_ALREADY_TERMINAL`、`RUN_CANCELLED`、`RUN_LIMIT_EXCEEDED`
- Tool：`TOOL_USE_NOT_FOUND`、`TOOL_CALL_DUPLICATE`、`TOOL_RESULT_DUPLICATE`、`TOOL_RUN_MISMATCH`、`TOOL_APPROVAL_REQUIRED`、`TOOL_EXECUTION_FAILED`
- Cron：`CRON_TARGET_INVALID`、`CRON_DUPLICATE_FIRE`、`CRON_CONCURRENCY_BLOCKED`

只有暂时性 I/O、限流、锁冲突和可恢复 provider 错误设置 `retryable=true`。参数/不变量错误始终不可重试；重试必须沿用幂等键。

## 8. 五项开放决策的结论

1. **Agent 语义**：采用“统一运行 API + 可运行的版本化配置”；Provider 是适配层，不是一级领域对象；人格和工具策略属于 Agent 配置。
2. **Session 中切换 Agent**：允许按 Run 选择 Agent；Run 固定 revision，历史不被改写。
3. **跨 Provider 工具协议**：领域层使用 ToolUse/ToolResult 节点；Message 不增加 tool role；Adapter 双向转换供应商协议。
4. **Cron target**：默认从模板新建 Session；续写必须显式固定 `session_id + base_node_id`，不隐式追随 active head。
5. **本地持久化**：SQLite + append-only SessionNode + FTS5；大附件采用内容寻址文件存储；schema 使用单调递增 migration，并在迁移前备份数据库。

## 9. 实现备注

此 ADR 固化领域边界，不固定语言、ORM 或 provider SDK。实现阶段若发现 Provider 无法无损映射到上述联合节点协议，应新增兼容性 ADR，而不是扩展 Message role 或原地修改历史节点。
