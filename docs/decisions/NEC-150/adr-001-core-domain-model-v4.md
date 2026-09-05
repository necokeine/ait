## ADR-001：本地多 Agent 管理器核心领域模型

- 状态：Accepted，待实现验证
- 修订：v4；经 NEC-161 ADR-003 修订 Message ID、Project description 与 Session-Agent 绑定
- 日期：2026-09-02
- 替代：NEC-150 v3 附件
- 基线：NEC-144 的 `core-domain-model.md` 与 NEC-150 产品确认
- 来源：NEC-150；这是当前核心领域模型的权威版本

## 1. 决策摘要

1. 一个 Project 对应一个工作目录。创建 Project 时，如果指定目录本身不是 Git 仓库根，就在该目录执行 `git init`；因此注册完成后，工作目录必定同时是 Git root。
2. Project 持有 append-only 的 Message 树/森林；Message 是不可变节点，类似 Git commit。
3. Session 不是 Message 树的容器，而是持有当前 Agent、指向某个 Message 的可移动引用，类似带执行者的 Git branch；它是交互式 Agent 的最小运行单元，空闲时可显式重绑 Agent。
4. Message 的 `role` 只允许 `user | system | assistant`。
5. Message 内部包含有序的 sub-message 节点。ToolUse 是 assistant Message 内的一种 sub-message；ToolResult 是一种特殊的 user Message，不是独立的树节点，也不是新的 role。
6. Agent 是“可运行的 Message 生成 API 及其版本化配置”。给定一个基准 Message，它持续产生一串后代 Message，其中可经历 ToolUse 与 ToolResult，直到本次 Run 终止。
7. Run 是“一个基准 Message + 一个 Agent”的一次完整任务运行。交互式 Run 可绑定一个 Session，并随着新 Message 生成而推进该 Session；重试、压缩恢复和队列处理仍属于同一个 Run。
8. Provider/SDK Adapter 是 Agent 的实现细节，不作为 MVP 的一级领域实体。
9. Cron 固定引用一个 Message 和一个 Agent，按照时间安排反复启动 Run；它不依赖或移动 Session。
10. MVP 使用 SQLite 保存结构化数据和 append-only Message，以 FTS5 建索引；大附件使用工作目录外的内容寻址文件存储。

## 2. 术语表与边界

### Project

Project 是本地工作上下文、安全边界与默认配置的容器。

```text
Project {
  id, name, description,          // 未提供时为空字符串
  workdir,                       // 规范化后的绝对路径
  git_initialized_by_manager,
  repo_url?,                     // 可选、声明型的远端仓库地址
  base_commit,                   // 注册 Project 时冻结的初始 HEAD
  default_agent_id?,              // 创建 Session 时的默认建议，可覆盖
  instruction_revision,
  metadata,                      // 可扩展、非敏感、可序列化元数据
  created_at, updated_at
}
```

`createProject(workdir, metadata)` 的 Git 语义固定为：

1. 校验 `workdir` 存在、是目录且可访问，然后解析为规范化绝对路径。
2. 若 `git -C <workdir> rev-parse --show-toplevel` 的规范化结果正好等于 `workdir`，直接复用现有仓库。
3. 否则在 `workdir` 执行 `git init`。这也适用于位于另一个仓库内部、但自身不是 Git root 的目录；结果是一个独立的嵌套仓库。
4. 再次校验 Git top-level 等于 `workdir`。读取完整 `HEAD` object id；若新初始化或已有仓库仍是 unborn HEAD，创建一个 manager-owned 的空初始提交后再次读取。
5. 将该 object id 冻结为 `Project.base_commit`；只有 Git root 与 HEAD 都验证成功后才持久化 Project。

一个规范化路径最多注册为一个 Project。`repo_url` 是可选的声明型来源信息，不参与 Project 身份；实际 Git remote、当前分支等仍是可刷新派生信息。默认情况下，文件工具不得越出 `workdir`；越界必须经过显式授权。

创建一棵新的 Message 树时，从 Project 指令源生成根 System Message 快照。仅在已有 Message 上打开 Session 时不创建或改写 System Message。Project 后续变化不回写旧 Message。

### Agent

Agent 是一个可运行、可版本化的 Message 生成能力。它封装 provider/model 配置、提示词片段、工具策略和能力声明，并暴露统一 API：

```text
Agent.run(
  agent_revision,
  message_path,
  tool_bridge,
  limits,
  cancellation
) -> ordered stream<ProposedMessage | RunSignal>
```

宿主 Run 协调器校验并持久化每个 ProposedMessage；Agent 不能绕过领域规则直接修改 Message 树。遇到 assistant Message 中的 ToolUse sub-message 时，宿主通过 tool bridge 执行工具，并把结果持久化为一个 ToolResult user Message，再将更新后的路径交还 Agent。

Agent 一轮返回只代表当前生成循环暂时结束，不等于 Run 已完成。Run 协调器还必须处理可重试错误、上下文压缩与检查点恢复，以及运行期间新进入该 Run 队列的工作；只有终止屏障通过后，整次 Run 才结束。

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

Session 是持有当前 Agent、指向 Project 某个 Message 的可移动命名引用，也是交互式 Agent 的最小运行单元。它本身不拥有历史；历史由当前 Message 沿 `parent_message_id` 回溯得到。其引用语义等同于 Git branch ref，而不是 repository 或 commit 集合。Session 空闲时可通过 version CAS 显式重绑 Agent；活动 Run 期间禁止切换。

```text
Session {
  id, project_id, name, title?, description,
  current_message_id,
  active_run_id?,
  agent_id,
  status: active | archived,
  version,
  created_at, updated_at
}
```

经 NEC-176 ADR-005 修订，`name` 对新 Session 默认为空且只由成员显式设置；`title` 与 `description` 分别保存 AI 生成的展示标题与检索摘要。展示时按 `name → title → Session <短 id>` 回退。

`SessionView(session_id)` 是从 `current_message_id` 沿 parent 回溯到根、再反向排列得到的 Message 路径。多个 Session 可以指向同一个 Message，也可以沿不同子节点形成分支。

点击任意 Message 并“打开 Session”时，必须选择 Agent 并创建一个新 Session；其 `current_message_id` 指向该 Message，不复制 Message，也不移动其他 Session。若要让同一节点由不同 Agent 并排推进，应打开另一个 Session；若只需继续同一 Session，可在空闲时显式重绑。常规交互流程为：

1. Session 当前指向 `M0`。
2. 用户提交内容，系统先确认 Project Git index、worktree（含未跟踪文件）均干净，并稳定读取完整 HEAD；然后创建 `U1(parent=M0, role=user, git_commit=HEAD)`，再以 Session `version` 做 compare-and-swap，将指针从 `M0` 推进到 `U1`。检查失败时不得写 Message、Run 或移动 Session。
3. 系统以 `U1 + Session.agent_id` 创建 Run，解析并固定 Agent revision，并把该 Session 绑定为 `follow_session_id`。
4. Run 每持久化一个新 Message，就把 Session 指针从上一个 Message CAS 推进到新 Message；Session 因而随着生成过程逐步向下移动。
5. Run 通过终止屏障后清除 `active_run_id`，Session 留在本次 Run 的最后一个 Message。

一个 Session 同一时刻最多绑定一个非终态 Run。想从同一节点并发构建另一条分支，应在该节点打开另一个 Session。活动 Run 期间到达同一 Session 的新输入进入该 Run 的队列，而不是并发启动第二个 Run；消费该队列项时，先以 Session 当前 Message 为 parent 追加 user Message并推进指针，再继续 Agent 循环。

### Message 与 sub-message

Message 是 Project Message 树中的不可变节点，类似 Git commit。Project 可以有多棵独立 Message 树，合称 Message forest：

```text
Message {
  id: UUID, project_id, parent_message_id?,
  role: user | system | assistant,
  message_kind: standard | tool_result,
  sub_messages: SubMessage[],
  origin: project | human | agent | tool | scheduler | system,
  created_by_session_id?,
  run_id?, run_seq?,
  git_commit?,                   // 仅普通 human user Message 必填
  created_at, metadata
}

SubMessage =
  Text { text }
  | FileRef { attachment_id, media_type, name? }
  | ToolUse { call_id, tool_name, arguments, provider_metadata? }
  | StructuredData { media_type, value }
```

`created_by_session_id` 仅记录创建来源，方便审计；它不表示 Message 被该 Session 所有。

角色与内容约束：

- `system` Message 保存 System prompt，只允许系统支持的普通 sub-message，不允许 ToolUse。
- `assistant` Message 可包含 Text、StructuredData、FileRef 和零个或多个 ToolUse。ToolUse 是 assistant Message 内部的 sub-message 节点，不能单独成为 Message 树节点。
- 普通 `user` Message 承载人类、调度器或系统注入的输入。
- `origin=human` 的普通 `user` Message 必须携带创建时 Project 的完整 `git_commit`；该提交只可在 Git index 与 worktree 均干净时捕获。其他 Message 不携带该字段。
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

每个 ToolUse 对应一个 ToolResult user Message。若一个 assistant Message 含多个 ToolUse，工具可以并行执行，但 ToolResult Message 必须按 ToolUse 在 `sub_messages` 中的稳定顺序串行追加到同一 Message 路径。

### Run

Run 是从一个确定的 `base_message_id` 配合一个确定 Agent revision 所进行的一次完整任务运行。

```text
Run {
  id, project_id, base_message_id, last_message_id?,
  follow_session_id?,
  agent_id, agent_revision,
  trigger: manual | cron,
  cron_id?, scheduled_at?,
  status, phase, stop_reason?, error?,
  step_count, max_steps,
  token_budget?, cost_budget?, usage,
  attempt_count, compaction_count,
  retry_policy, next_retry_at?,
  checkpoint_id?,
  queue_version, queue_cursor,
  dedupe_key?,
  started_at?, ended_at?, created_at
}
```

每个 Run 从创建起固定 `base_message_id`、`agent_id` 和 `agent_revision`，其生成的 Message 通过 `run_id` 获得完整来源。交互式 Run 的 `agent_id` 必须等于创建该 Run 时 `follow_session_id` 的 `Session.agent_id`；Session 可在两个 Run 之间重绑，但每个 Run 始终固定触发时解析到的 Agent 与 revision。Cron 或无 Session Run 直接固定其显式选择的 Agent。

`follow_session_id` 可空：从交互式 Session 启动时设置，并在启动事务中确认 Session 当前指针等于 `base_message_id`、写入 `active_run_id`；Cron 或后台调用可直接基于 Message 启动 Run，不必创建 Session。无 Session 的 Run 仍通过 `last_message_id` 暴露结果，用户之后可在任一产出 Message 上打开 Session。

一次底层 Agent 调用或恢复尝试不是 Run。为了审计，可在一个 Run 下记录多个 attempt：

```text
RunAttempt {
  id, run_id, number,
  reason: initial | retry | recovery,
  checkpoint_id?,
  status, error?,
  started_at, ended_at?
}

RunQueueItem {
  id, run_id, seq,
  kind, payload_ref,
  status: pending | processing | consumed | rejected,
  created_at, consumed_at?
}
```

重试、压缩后恢复、进程崩溃恢复和处理新 RunQueueItem 都沿用原 `run_id`。只有新选定基准 Message 或新触发一次任务，才创建新 Run。

当 Run 带 `follow_session_id` 时，每个新 Message 的持久化与 Session 指针推进必须在同一事务中完成。若指针或 version 不符合预期，返回 `SESSION_POINTER_CONFLICT`，不得覆盖 Session；已产生的 Message 保留为可恢复分支。Run 完成、失败、取消或超限时都必须释放匹配的 `active_run_id`。

Run 进入 `completed` 前必须经过终止屏障：

1. Agent 当前轮已返回，且没有待处理 ToolUse/ToolExecution。
2. 没有正在执行或已安排的 retry。
3. 没有正在执行的上下文压缩、检查点写入或恢复。
4. 所有 Message/ToolResult 和使用量均已持久化。
5. RunQueue 为空，并且从读取空队列到提交完成状态期间 `queue_version` 没有变化。

第 5 条使用 compare-and-swap：若新项目先入队，`queue_version` 增长，完成转换失败并继续处理；若完成转换先成功，后续入队返回 `RUN_ALREADY_TERMINAL`，调用方必须显式创建新 Run。这样不会出现“刚判定完成又漏掉队列”的竞态。

### ToolExecution

ToolExecution 是工具执行控制与审计记录，不属于给模型看的 Message 树。

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
  id, name,
  project_id, base_message_id,
  agent_id,
  schedule, timezone, enabled,
  concurrency_policy: allow | forbid | replace,
  misfire_policy: skip | run_once | catch_up,
  max_runtime?, next_run_at?, last_run_at?,
  created_at, updated_at
}
```

Cron 的 target 就是固定的 `base_message_id + agent_id`；`project_id` 用于边界校验。每次到点直接以该 Message 为基准、以该 Agent 启动一个没有 `follow_session_id` 的新 Run；不自动创建输入 Message，也不新建或移动 Session。若希望定时任务从另一个节点继续，必须显式更新 Cron 的 `base_message_id`。

触发时解析并快照 Agent 当前启用的 revision 到 Run。触发幂等键为 `cron_id + scheduled_at`；`concurrency_policy` 只约束同一 Cron 产生的 Run。Cron 引用的 Message 或 Agent 不可用时，本次触发失败并记录稳定错误，不静默换目标。Run 的 `last_message_id` 是本次结果引用，之后可以在该节点打开 Session。

## 3. 关系图

```text
Project 1 ───── * Message
   │                ├── parent_message_id -> Message
   │                └── * SubMessage
   │                         └── ToolUse 仅属于 assistant Message
   │
   ├──── * Session ── current_message_id ──> Message
   │          ├── agent_id ──> Agent
   │          └── active_run_id? ──> Run
   │
   ├──── * Run ── base_message_id ──> Message
   │          ├── follow_session_id? ──> Session
   │          ├── * ToolExecution ── links ToolUse -> ToolResult Message
   │          ├── * RunAttempt
   │          ├── * RunQueueItem
   │          └── appends an ordered Message chain
   │
   ├──── default/binding ──── * Agent(revisioned)
   └──── * Cron ── fixed Message + Agent + schedule ──> Run

ToolResult = role=user, message_kind=tool_result 的 Message
Message.role = user | system | assistant
```

## 4. 核心不变量

1. Project 创建结束后，`workdir` 等于 Git top-level，`base_commit` 是注册时冻结的有效完整 HEAD；若创建前不是 Git root，必须先在该目录成功执行 `git init`，若 HEAD 尚未出生则创建空初始提交。规范化路径在系统内唯一。
2. 一个 Message 只属于一个 Project；其 parent 必须属于同一 Project；Message 图必须无环。每个连通树恰有一个根，根必须是 `role=system` 的 Message。
3. 一个 Session 只属于一个 Project、持有一个当前 Agent，并且只指向该 Project 的一个 Message。Message 不属于 Session；多个 Session 可以指向同一个 Message。只有 `active_run_id` 为空时才能以 version CAS 重绑 Agent。
4. Message 创建后不可变。编辑旧 Message 会以旧 Message 的 parent 为基点创建替代分支；重新生成会以旧 assistant Message 的 parent 为 `base_message_id` 启动新 Run。查看或继续历史节点时创建新的 Session 引用，不复制历史。
5. Message role 严格限制为 `user | system | assistant`。ToolUse 只能是 assistant Message 内的 sub-message；ToolResult 必须是 `role=user, message_kind=tool_result` 的 Message。
6. `role=user, message_kind=standard, origin=human` 的 Message 必须携带有效完整 `git_commit`，且追加前 Project Git index/worktree（含未跟踪文件）必须干净；Git 检查失败时不得产生任何领域写入。该字段不得出现在其他 Message 上。
7. Run 创建后固定 `base_message_id`、Agent 与 Agent revision。首个输出 Message 以 `base_message_id` 为 parent；后续输出以前一个 Run 输出 Message 为 parent；`run_seq` 从 1 连续递增。并发 Run 因而自然形成分支。
8. ToolResult Message 必须引用同一 Run 当前路径上尚未完成的 ToolUse；每个 `call_id` 在 Run 内唯一，且最多有一个最终 ToolResult Message。
9. 每个 Agent 输出 Message 必须先持久化，才能开始下一模型或工具步骤；重试、压缩恢复和崩溃恢复只能从最后一个已提交 Message/检查点继续，并沿用同一 `run_id`。
10. Run 固定 Agent revision 和预算。凭证不得进入 Message、日志、导出或 metadata。
11. Session 的自动推进只能从当前 Message 移到其新建的直接子 Message，并且必须使用 `version` compare-and-swap。指针冲突时保留已生成分支并返回错误，不覆盖 Session。
12. 一个 Session 同时最多有一个非终态 `active_run_id`。跟随 Session 的 Run 必须使用创建 Run 时的 `Session.agent_id`。从同一 Message 并发运行必须创建另一个 Session，或启动不跟随 Session 的 Run；空闲 Session 可显式切换 Agent。
13. 交互式 Run 每次持久化 Message 时，必须在同一事务内推进其 `follow_session_id`；所有终态都必须条件式释放匹配的 `active_run_id`。
14. `completed` 只能由终止屏障原子写入：无待处理工具、重试、压缩/恢复或队列项，输出已落盘，且 `queue_version` 未变化。
15. Cron 必须固定引用 `project_id + base_message_id + agent_id`；每次触发创建不跟随 Session 的新 Run，按 dedupe key 幂等。多个触发从同一 Message 形成分支。

## 5. Run 状态机

```text
queued
  -> acquiring_session_ref?  // 仅交互式 Run；校验 current_message_id 并占用 active_run_id
  -> assembling_context
  -> calling_agent
       -> persisting_message_and_advancing_session -> calling_agent
       -> waiting_approval -> executing_tool -> persisting_tool_result -> calling_agent
       -> executing_tool   -> persisting_tool_result -> calling_agent
       -> retry_wait -> assembling_context
       -> compacting_context -> checkpointing -> recovering -> assembling_context
       -> draining_queue -> assembling_context
  -> settling
       -> assembling_context  // 有新队列项或 queue_version 发生变化
       -> releasing_session_ref -> completed  // 终止屏障 CAS 成功

任一非终态 -> cancelled | limit_exceeded
不可恢复错误或重试耗尽 -> failed
```

Agent 产生最终 assistant Message 且没有待处理 ToolUse 时，只能进入 `draining_queue/settling`，不能直接进入 `completed`。可重试错误进入 `retry_wait`；压缩或恢复只是同一 Run 的内部 phase。用户取消为 `cancelled`；预算或步数达到上限为 `limit_exceeded`；不可恢复错误或重试耗尽为 `failed`。`waiting_approval` 可恢复到 `executing_tool`，也可因拒绝先持久化 denied ToolResult user Message 再回到 `calling_agent`。所有终态路径都执行 Session ref 的条件释放；只有 `active_run_id` 仍等于本 Run 时才清除。

## 6. 最小 API 契约

```text
createProject(workdir, repo_url?, metadata) // 必要时 git init 并创建空初始提交
updateProjectMetadata(project_id, patch)
createMessageRoot(project_id, system_sub_messages)
openSession(project_id, at_message_id, agent_id, name?)
setSessionAgent(session_id, agent_id, expected_version)
getSessionView(session_id)
getMessagePath(message_id)
submitUserInput(session_id, expected_version, sub_messages)
appendMessage(project_id, parent_message_id, role, sub_messages) // 内部原语
startRun(base_message_id, agent_id, follow_session_id?, limits, dedupe_key?)
enqueueRunItem(run_id, kind, payload_ref, expected_status?)
cancelRun(run_id)
resumeRun(run_id)
advanceSession(session_id, from_message_id, to_child_message_id, expected_version) // 内部 CAS
createCron(base_message_id, agent_id, schedule, timezone, policies)
```

`submitUserInput` 在一个事务里校验 Session 空闲、创建 user Message、推进 Session、创建并绑定 Run；失败时不得留下半完成状态。`resumeRun` 恢复同一个 Run，不重复已提交 Message 或已完成工具调用。`enqueueRunItem` 与终止屏障以 `queue_version` 协调。所有创建型接口接受可选 idempotency key。

## 7. 稳定错误语义

统一错误信封：

```text
DomainError { code, message, retryable, details?, cause_id? }
```

稳定错误码：

- Project：`PROJECT_PATH_NOT_FOUND`、`PROJECT_PATH_NOT_DIRECTORY`、`PROJECT_PATH_ALREADY_REGISTERED`、`PROJECT_GIT_INIT_FAILED`、`PROJECT_PATH_OUT_OF_SCOPE`
- Session/Message：`SESSION_NOT_FOUND`、`SESSION_BUSY`、`SESSION_POINTER_CONFLICT`、`SESSION_MESSAGE_PROJECT_MISMATCH`、`MESSAGE_NOT_FOUND`、`MESSAGE_PROJECT_MISMATCH`、`INVALID_ROOT_MESSAGE`、`INVALID_MESSAGE_ROLE`、`INVALID_SUBMESSAGE_KIND`、`MESSAGE_IMMUTABLE`
- 消息协议：`TOOL_USE_REQUIRES_ASSISTANT`、`TOOL_RESULT_REQUIRES_USER`、`TOOL_RESULT_MESSAGE_INVALID`
- Agent/Run：`AGENT_NOT_FOUND`、`AGENT_DISABLED`、`AGENT_REVISION_NOT_FOUND`、`AGENT_CAPABILITY_UNSUPPORTED`、`RUN_NOT_RESUMABLE`、`RUN_ALREADY_TERMINAL`、`RUN_RETRY_EXHAUSTED`、`RUN_RECOVERY_FAILED`、`RUN_QUEUE_CONFLICT`、`RUN_CANCELLED`、`RUN_LIMIT_EXCEEDED`
- Tool：`TOOL_USE_NOT_FOUND`、`TOOL_CALL_DUPLICATE`、`TOOL_RESULT_DUPLICATE`、`TOOL_RUN_MISMATCH`、`TOOL_APPROVAL_REQUIRED`、`TOOL_EXECUTION_FAILED`
- Cron：`CRON_PROJECT_UNAVAILABLE`、`CRON_BASE_MESSAGE_UNAVAILABLE`、`CRON_AGENT_UNAVAILABLE`、`CRON_DUPLICATE_FIRE`、`CRON_CONCURRENCY_BLOCKED`

只有暂时性 I/O、限流、锁冲突和可恢复 provider 错误设置 `retryable=true`。参数/不变量错误始终不可重试；重试必须沿用幂等键。

## 8. 五项开放决策的结论

1. **Agent 语义**：采用“统一运行 API + 可运行的版本化配置”；Provider 是适配层，不是一级领域对象；人格和工具策略属于 Agent 配置。
2. **Session 与 Agent**：Session 是持有当前 Agent 的可移动 Message ref，也是交互式 Agent 的最小运行单元；空闲时可显式 CAS 重绑，活动 Run 期间锁定。每个 Run 使用创建时的 Agent 并固定 revision，历史不被改写。
3. **跨 Provider 工具协议**：Message 树只包含 Message。ToolUse 是 assistant Message 内的 sub-message；ToolResult 是 user Message；Adapter 双向转换供应商协议。
4. **Cron target**：固定 `project_id + base_message_id + agent_id`。每次到点从同一 Message 配合该 Agent 创建一个不跟随 Session 的 Run；不添加隐式输入，也不移动 Session。
5. **本地持久化**：SQLite + append-only Message + mutable Session ref + FTS5；sub-message 随 Message 原子持久化；大附件采用内容寻址文件存储；schema 使用单调递增 migration，并在迁移前备份数据库。

## 9. 实现备注

此 ADR 固化领域边界，不固定语言、ORM 或 provider SDK。实现阶段若发现 Provider 无法无损映射到上述 Message/sub-message 协议，应新增兼容性 ADR，而不是增加 Message role、把 ToolUse 升格为 Message 树节点，或原地修改历史 Message。实现 Run supervisor 时，Agent 调用结束不得直接写 `completed`；必须统一经过终止屏障。
