## ADR-001：本地多 Agent 管理器核心领域模型

- 状态：Accepted，待实现验证
- 修订：v2
- 日期：2026-09-02
- 替代：NEC-150 首版附件
- 基线：NEC-144 的 `core-domain-model.md` 与 NEC-150 产品确认
- 归档说明：已由 ADR-001 v4 替代

## 1. 决策摘要

1. 一个 Project 对应一个工作目录。创建 Project 时，如果指定目录本身不是 Git 仓库根，就在该目录执行 `git init`；因此注册完成后，工作目录必定同时是 Git root。
2. Session 是一棵不可变的 **Message 树**，树上的每个节点都是 Message。
3. Message 的 `role` 只允许 `user | system | assistant`。
4. Message 内部包含有序的 sub-message 节点。ToolUse 是 assistant Message 内的一种 sub-message；ToolResult 是一种特殊的 user Message，不是独立的 Session 树节点，也不是新的 role。
5. Agent 是“可运行的 Message 生成 API 及其版本化配置”。给定 Session 树上的一个基准 Message，它持续产生一串 Message，其中可经历 ToolUse 与 ToolResult，直到本次 Run 终止。
6. Run 是 Agent API 的一次有界执行；Session 是长期历史，二者不能合并。
7. Provider/SDK Adapter 是 Agent 的实现细节，不作为 MVP 的一级领域实体。
8. Cron 只负责确定性地产生输入并启动 Run；它不能隐式追随可变的 active head。
9. MVP 使用 SQLite 保存结构化数据和 append-only Message，以 FTS5 建索引；大附件使用工作目录外的内容寻址文件存储。

## 2. 术语表与边界

### Project

Project 是本地工作上下文、安全边界与默认配置的容器。

```text
Project {
  id, name, description,
  workdir,                       // 规范化后的绝对路径
  git_initialized_by_manager,
  default_agent_id?,
  instruction_revision,
  metadata,                      // 可扩展、非敏感、可序列化元数据
  created_at, updated_at
}
```

`createProject(workdir, metadata)` 的 Git 语义固定为：

1. 校验 `workdir` 存在、是目录且可访问，然后解析为规范化绝对路径。
2. 若 `git -C <workdir> rev-parse --show-toplevel` 的规范化结果正好等于 `workdir`，直接复用现有仓库。
3. 否则在 `workdir` 执行 `git init`。这也适用于位于另一个仓库内部、但自身不是 Git root 的目录；结果是一个独立的嵌套仓库。
4. 再次校验 Git top-level 等于 `workdir`；只有成功后才持久化 Project。`git init` 失败则 Project 创建失败。

一个规范化路径最多注册为一个 Project。Git remote、当前分支等是可刷新派生信息，不是 Project 身份。默认情况下，文件工具不得越出 `workdir`；越界必须经过显式授权。

创建 Session 时，从 Project 指令源生成 System prompt 快照。Project 后续变更不回写旧 Session。

### Agent

Agent 是一个可运行、可版本化的 Message 生成能力。它封装 provider/model 配置、提示词片段、工具策略和能力声明，并暴露统一 API：

```text
Agent.run(
  agent_revision,
  session_path,
  tool_bridge,
  limits,
  cancellation
) -> ordered stream<ProposedMessage | RunSignal>
```

宿主 Run 协调器校验并持久化每个 ProposedMessage；Agent 不能绕过领域规则直接修改 Session 树。遇到 assistant Message 中的 ToolUse sub-message 时，宿主通过 tool bridge 执行工具，并把结果持久化为一个 ToolResult user Message，再将更新后的路径交还 Agent。循环持续到 Agent 发出终止信号，或 Run 被取消、失败、拒绝或超限。

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

`credential_ref` 只保存引用，绝不保存明文凭证。Provider Adapter 负责把统一 Message/sub-message 协议转换为供应商协议，不拥有 Session 或 Run。

### Session

Session 是一个 Project 内的长期对话/执行历史聚合，内部是一棵不可变 Message 树。

```text
Session {
  id, project_id, title,
  root_message_id, active_head_message_id,
  default_agent_id?,
  status: active | archived,
  version,
  created_at, updated_at
}
```

`SessionPath(session_id, head_message_id)` 是从根到 head 的唯一有序 Message 路径，也是 Agent 的输入上下文。`active_head_message_id` 只是 UI 默认指针；切换、编辑和重新生成均通过创建分支完成，不改写历史。

### Message 与 sub-message

Message 是 Session 树中唯一的节点类型：

```text
Message {
  id, session_id, parent_message_id?,
  role: user | system | assistant,
  message_kind: standard | tool_result,
  sub_messages: SubMessage[],
  origin: project | human | agent | tool | scheduler | system,
  run_id?, run_seq?,
  created_at, metadata
}

SubMessage =
  Text { text }
  | FileRef { attachment_id, media_type, name? }
  | ToolUse { call_id, tool_name, arguments, provider_metadata? }
  | StructuredData { media_type, value }
```

角色与内容约束：

- `system` Message 保存 System prompt，只允许系统支持的普通 sub-message，不允许 ToolUse。
- `assistant` Message 可包含 Text、StructuredData、FileRef 和零个或多个 ToolUse。ToolUse 是 assistant Message 内部的 sub-message 节点，不能单独成为 Session 树节点。
- 普通 `user` Message 承载人类、调度器或系统注入的输入。
- ToolResult 是 `role=user, message_kind=tool_result, origin=tool` 的特殊 Message：

```text
ToolResultMessage extends Message {
  role: user,
  message_kind: tool_result,
  origin: tool,
  tool_result: {
    call_id,
    status: succeeded | failed | denied | cancelled,
    output?, error?
  }
}
```

每个 ToolUse 对应一个 ToolResult user Message。若一个 assistant Message 含多个 ToolUse，工具可以并行执行，但 ToolResult Message 必须按 ToolUse 在 `sub_messages` 中的稳定顺序串行追加到 Session 路径。

### Run

Run 是从一个确定的 `base_message_id` 调用一个确定 Agent revision 的一次执行。

```text
Run {
  id, session_id, base_message_id, last_message_id?,
  agent_id, agent_revision,
  status, stop_reason?, error?,
  step_count, max_steps,
  token_budget?, cost_budget?, usage,
  dedupe_key?,
  started_at?, ended_at?, created_at
}
```

一个 Session 可以在不同 Run 中选择不同 Agent；不提供“修改 Session 的历史 Agent”这一操作。每个 Run 固定 `agent_revision`，其生成的 Message 通过 `run_id` 获得完整来源。

### ToolExecution

ToolExecution 是工具执行控制与审计记录，不属于给模型看的 Session 树。

```text
ToolExecution {
  id, run_id, call_id,
  assistant_message_id, tool_use_index,
  tool_result_message_id?,
  tool_name, arguments,
  attempt,
  approval_status,
  status,
  result_summary?, error?,
  started_at?, ended_at?, created_at
}
```

`assistant_message_id + tool_use_index` 唯一定位 ToolUse sub-message。重试产生新的 ToolExecution attempt，但一个 ToolUse 最多产生一个最终 ToolResult Message。完整大输出可放附件存储，Message 只保存受限内容与引用。

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
  | ContinueFromMessage { session_id, base_message_id }
```

每次触发先创建一个 `role=user, message_kind=standard, origin=scheduler` 的 Message，再从该 Message 启动 Run。`ContinueFromMessage` 必须固定 Message ID；禁止使用隐式 `active_head`，从而避免调度时刻的竞态。默认 target 为 NewSession。触发幂等键为 `cron_id + scheduled_at`。

## 3. 关系图

```text
Project 1 ───── * Session 1 ───── * Message
   │                 │                │
   │                 │                ├── parent_message_id -> Message
   │                 │                └── * SubMessage
   │                 │                         └── ToolUse 仅属于 assistant Message
   │                 │
   │                 └──── * Run 1 ───── * ToolExecution
   │                              │              └── links ToolUse -> ToolResult Message
   │                              └── appends an ordered Message chain
   ├──── default/binding ──── * Agent(revisioned)
   └──── * Cron ── trigger ──> input user Message + Run

ToolResult = role=user, message_kind=tool_result 的 Message
Message.role = user | system | assistant
```

## 4. 核心不变量

1. Project 创建结束后，`workdir` 等于 Git top-level；若创建前不是 Git root，必须先在该目录成功执行 `git init`。规范化路径在系统内唯一。
2. 一个 Session 只属于一个 Project，且恰有一个根 Message；根必须是 `role=system` 的 Message。
3. 一个 Message 只属于一个 Session；其 parent 必须属于同一 Session；Message 图必须无环。
4. Message 创建后不可变。编辑旧 Message 会以旧 Message 的 parent 为基点创建替代分支；重新生成会以旧 assistant Message 的 parent 为 `base_message_id` 启动新 Run。
5. Message role 严格限制为 `user | system | assistant`。ToolUse 只能是 assistant Message 内的 sub-message；ToolResult 必须是 `role=user, message_kind=tool_result` 的 Message。
6. Run 的首个输出 Message 以 `base_message_id` 为 parent；后续输出以前一个 Run 输出 Message 为 parent；`run_seq` 从 1 连续递增。并发 Run 因而自然形成分支。
7. ToolResult Message 必须引用同一 Run 当前路径上尚未完成的 ToolUse；每个 `call_id` 在 Run 内唯一，且最多有一个最终 ToolResult Message。
8. 每个 Agent 输出 Message 必须先持久化，才能开始下一模型或工具步骤；崩溃恢复只能从最后一个已提交 Message 继续。
9. Run 固定 Agent revision 和预算。凭证不得进入 Message、日志、导出或 metadata。
10. 更新 `active_head_message_id` 必须以 Session `version` 做 compare-and-swap；冲突时保留新分支并返回冲突，不丢历史。
11. Cron 触发按 dedupe key 幂等；固定 Message 上的并发触发形成分支，不能静默覆盖 active head。

## 5. Run 状态机

```text
queued
  -> assembling_context
  -> calling_agent
       -> persisting_message -> calling_agent
       -> waiting_approval -> executing_tool -> persisting_tool_result -> calling_agent
       -> executing_tool   -> persisting_tool_result -> calling_agent
  -> completed

queued / assembling_context / calling_agent / persisting_message /
waiting_approval / executing_tool / persisting_tool_result
  -> failed | cancelled | limit_exceeded
```

Agent 产生最终 assistant Message 且其中没有待处理 ToolUse 时为 `completed`；用户取消为 `cancelled`；预算或步数达到上限为 `limit_exceeded`；不可恢复错误为 `failed`。`waiting_approval` 可恢复到 `executing_tool`，也可因拒绝先持久化 denied ToolResult user Message 再回到 `calling_agent`。

## 6. 最小 API 契约

```text
createProject(workdir, metadata) // 必要时在 workdir 执行 git init
updateProjectMetadata(project_id, patch)
createSession(project_id, agent_id?)
appendMessage(session_id, parent_message_id, role, sub_messages)
getSessionPath(session_id, head_message_id)
startRun(session_id, base_message_id, agent_id, limits, dedupe_key?)
cancelRun(run_id)
resumeRun(run_id)
setActiveHead(session_id, head_message_id, expected_version)
createCron(project_id, schedule, timezone, target, input_template)
```

`resumeRun` 不重复已提交 Message 或已完成工具调用。所有创建型接口接受可选 idempotency key。

## 7. 稳定错误语义

统一错误信封：

```text
DomainError { code, message, retryable, details?, cause_id? }
```

稳定错误码：

- Project：`PROJECT_PATH_NOT_FOUND`、`PROJECT_PATH_NOT_DIRECTORY`、`PROJECT_PATH_ALREADY_REGISTERED`、`PROJECT_GIT_INIT_FAILED`、`PROJECT_PATH_OUT_OF_SCOPE`
- Session/Message：`SESSION_NOT_FOUND`、`MESSAGE_NOT_FOUND`、`MESSAGE_SESSION_MISMATCH`、`INVALID_ROOT_MESSAGE`、`INVALID_MESSAGE_ROLE`、`INVALID_SUBMESSAGE_KIND`、`MESSAGE_IMMUTABLE`、`ACTIVE_HEAD_CONFLICT`
- 消息协议：`TOOL_USE_REQUIRES_ASSISTANT`、`TOOL_RESULT_REQUIRES_USER`、`TOOL_RESULT_MESSAGE_INVALID`
- Agent/Run：`AGENT_NOT_FOUND`、`AGENT_DISABLED`、`AGENT_REVISION_NOT_FOUND`、`AGENT_CAPABILITY_UNSUPPORTED`、`RUN_NOT_RESUMABLE`、`RUN_ALREADY_TERMINAL`、`RUN_CANCELLED`、`RUN_LIMIT_EXCEEDED`
- Tool：`TOOL_USE_NOT_FOUND`、`TOOL_CALL_DUPLICATE`、`TOOL_RESULT_DUPLICATE`、`TOOL_RUN_MISMATCH`、`TOOL_APPROVAL_REQUIRED`、`TOOL_EXECUTION_FAILED`
- Cron：`CRON_TARGET_INVALID`、`CRON_DUPLICATE_FIRE`、`CRON_CONCURRENCY_BLOCKED`

只有暂时性 I/O、限流、锁冲突和可恢复 provider 错误设置 `retryable=true`。参数/不变量错误始终不可重试；重试必须沿用幂等键。

## 8. 五项开放决策的结论

1. **Agent 语义**：采用“统一运行 API + 可运行的版本化配置”；Provider 是适配层，不是一级领域对象；人格和工具策略属于 Agent 配置。
2. **Session 中切换 Agent**：允许按 Run 选择 Agent；Run 固定 revision，历史不被改写。
3. **跨 Provider 工具协议**：Session 树只包含 Message。ToolUse 是 assistant Message 内的 sub-message；ToolResult 是 user Message；Adapter 双向转换供应商协议。
4. **Cron target**：默认从模板新建 Session；续写必须显式固定 `session_id + base_message_id`，不隐式追随 active head。
5. **本地持久化**：SQLite + append-only Message + FTS5；sub-message 随 Message 原子持久化；大附件采用内容寻址文件存储；schema 使用单调递增 migration，并在迁移前备份数据库。

## 9. 实现备注

此 ADR 固化领域边界，不固定语言、ORM 或 provider SDK。实现阶段若发现 Provider 无法无损映射到上述 Message/sub-message 协议，应新增兼容性 ADR，而不是增加 Message role、把 ToolUse 升格为 Session 节点，或原地修改历史 Message。
