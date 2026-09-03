## 核心领域模型（草案 v0.1）

> 归档说明：NEC-144 初稿，已由 ADR-001 v4 替代。

### 1. 建模原则

1. **历史不可变**：Message 创建后不原地修改；重试、编辑和重新生成都产生新分支。
2. **Session 是树，展示是路径**：Session 持有整棵 Message 树；给定任意 `head_message_id`，从根到该节点的唯一路径就是一次可展示、可继续的对话视图。
3. **运行与历史分离**：Run 表示一次 Agent 自动循环。Run 读取一条消息路径并向树上追加节点，但不是 Session 本身。
4. **配置可复现**：每条模型输出记录实际 Agent/模型配置版本；每个 Session 的根 System Message 保存 Project 提示词快照。
5. **自动化复用同一入口**：人工输入、Cron、Webhook 最终都转成“向 Session 追加输入并创建 Run”，避免多套执行语义。

### 2. 核心概念

#### Project

Project 代表一个受管理目录，是上下文、安全边界和默认配置的容器。

建议字段：

- `id`, `name`, `root_path`
- `default_agent_id`
- `instruction_sources[]`：例如内置模板、`AGENTS.md`、用户配置；带优先级
- `prompt_revision`
- `created_at`, `updated_at`

行为：

- 校验并规范化真实路径，禁止任务越出 Project 根目录，除非显式授权。
- 从目录信息与指令源生成基础 System prompt。
- 创建 Session 时，把“渲染后的 prompt + 来源摘要 + revision”写成根 System Message。Project 后续变化不回写旧 Session。

#### Agent

用户当前设想中，Agent 代表模型提供商。为避免“厂商”和“可运行配置”混在一起，MVP 可把 Agent 定义为**一个可运行的 provider/model 配置**，内部再由 Provider Adapter 对接 OpenAI、Anthropic、本地模型等。

建议字段：

- `id`, `name`, `provider_type`, `model`
- `endpoint`, `credential_ref`（只引用凭证，不保存明文）
- `capabilities`：streaming、tool calling、多模态、上下文窗口等
- `default_parameters`, `tool_policy`, `prompt_fragment`
- `config_revision`, `enabled`

Agent 负责“如何调用模型”；Project/Session 负责“带什么上下文调用”。一次 Run 固定 Agent revision，防止执行中配置漂移。

#### Message

Message 是 Session 树中的不可变节点。

建议字段：

- `id`, `session_id`, `parent_message_id`（根节点为空）
- `role`: `system | user | assistant | tool`
- `content_parts[]`: text、image/file 引用、tool_call、tool_result 等
- `origin`: `project | human | agent | tool | scheduler | system`
- `agent_id`, `agent_revision`, `run_id`（不适用时为空）
- `status`: `complete | partial | failed | redacted`
- `created_at`, `metadata`

System prompt 是 `role=system` 的特殊 Message，而不是 Session 外的隐式字符串。根节点通常是创建 Session 时冻结的 Project prompt；若未来允许中途追加系统指令，它仍然是普通树节点，因此可被审计。

Tool calling 的规范化表示：Assistant Message 的 content part 保存一个或多个 `tool_call`；每个执行结果按确定顺序追加为后代 Tool Message，并通过 `call_id` 回指。这样根到 head 的路径包含模型下一轮所需的全部工具结果，不会因把并行结果写成兄弟节点而丢上下文。

#### Session

Session 是 Message 树的聚合根，不等同于单条聊天记录。

建议字段：

- `id`, `project_id`, `title`
- `root_message_id`, `active_head_message_id`
- `default_agent_id`
- `status`: `active | archived`
- `version`（乐观并发控制）
- `created_at`, `updated_at`

投影规则：`SessionView(session_id, head_message_id)` = 从 `head_message_id` 沿 parent 回溯到 root，再逆序排列。`active_head_message_id` 只是 UI 默认指针，不改变历史；切换分支只移动该指针。

“编辑旧消息”语义为：以它的 parent 为基点创建一个替代 Message，从那里形成新分支；“重新生成”语义为：以旧 Assistant Message 的 parent 为基点启动新 Run。

#### Run

Run 是一次从指定分支头开始的 Agent 执行，用于承载模型调用、流式输出、工具执行、取消、限额和错误。

建议字段：

- `id`, `session_id`, `base_message_id`
- `agent_id`, `agent_revision`
- `status`, `stop_reason`, `error`
- `step_count`, `max_steps`, token/费用预算
- `started_at`, `ended_at`

状态机：

`queued -> assembling_context -> calling_model -> executing_tools -> calling_model ... -> completed`

任一执行态可进入 `waiting_approval`、`failed`、`cancelled` 或 `limit_exceeded`。模型产生最终 Assistant Message 且没有待执行 tool call 时结束。每一步都先持久化再继续，以支持崩溃恢复。

#### ToolExecution

虽然 tool call/result 会进入 Message 历史，仍建议单独保存 ToolExecution 作为运行记录：`call_id`, `run_id`, `tool_name`, 参数、审批状态、结果/错误、开始结束时间。Message 面向模型上下文，ToolExecution 面向执行控制与审计；两者通过 `call_id` 关联。

#### Cron

Cron 是自动化触发器，不直接调用模型；它在到点后创建一次可追踪的 Run。

建议字段：

- `id`, `project_id`, `name`, `schedule`, `timezone`, `enabled`
- `target`: 新建 Session、向固定 Session 分支追加输入，或从 Session 模板创建实例
- `agent_id`, `input_template`
- `concurrency_policy`: `allow | forbid | replace`
- `misfire_policy`: `skip | run_once | catch_up`
- `max_runtime`, `next_run_at`, `last_run_at`

每次触发生成稳定的 `dedupe_key = cron_id + scheduled_at`，确保重启或重复扫描不会启动两次。固定 Session 的并发写入默认创建兄弟分支，不能静默覆盖 active head。

### 3. 关系与不变量

```text
Project 1 ── * Session 1 ── * Message
   │             │             └── parent_message_id -> Message
   │             └── * Run 1 ── * ToolExecution
   ├── * Agent (default/bindings)
   └── * Cron ──trigger──> Run
```

必须满足：

- 一个 Message 只属于一个 Session，parent 必须同 Session，且图必须无环。
- 一个 Session 只有一个根；根通常是 System Message。
- Message 不可变；删除采用隐藏/脱敏标记，不破坏引用链。
- Run 的新消息必须是 `base_message_id` 的后代；并发 Run 自然形成分支。
- Tool result 必须引用同一 Run 路径上尚未完成的 tool call。
- `active_head_message_id` 的更新使用 Session version 做 compare-and-swap；失败时保留新分支并提示冲突。
- Project/Agent 配置都按 revision 快照，凭证永不进入 Message、日志或导出文件。

### 4. 最小操作契约

- `createProject(path)`：注册目录、发现指令源并生成 prompt revision。
- `createSession(project, agent?)`：创建 Session 和根 System Message 快照。
- `appendMessage(session, parent, content)`：验证树不变量并追加节点。
- `getSessionPath(session, head)`：返回根到 head 的有序路径。
- `startRun(session, base, agent, limits)`：锁定配置版本并启动自动循环。
- `cancelRun(run)` / `resumeRun(run)`：可控中止与崩溃恢复。
- `forkSession(session, fromMessage)`：逻辑分支；无需复制历史。
- `createCron(project, schedule, target)`：创建自动触发器，触发记录可追踪到 Run。

### 5. 仍需在实现前拍板的决策

1. Agent 的产品语义最终是“模型提供商”、 “可运行模型配置”，还是还包含人格/工具集；建议采用第二种，Provider 独立为适配层。
2. 是否允许一个 Session 中途切换 Agent；建议允许，但每条 Assistant Message 和 Run 必须记录实际配置版本。
3. Tool result 在跨 provider 协议中的规范化格式；建议领域层保持 content parts，适配器负责转换。
4. 固定 Session 的 Cron 到底续写 active head 还是每次由模板新建 Session；建议默认模板新建，显式选择才续写。
5. 本地持久化建议 SQLite + append-only Message；需单独 ADR 确认搜索、附件存储和迁移策略。
