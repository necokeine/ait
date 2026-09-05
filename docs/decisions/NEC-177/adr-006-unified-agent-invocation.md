# ADR-006：统一 Agent 调用接口

- 状态：Accepted，待分阶段实现
- 日期：2026-09-05
- 依赖：ADR-001 v4、NEC-151 ADR-002/003、NEC-169 ADR-001
- 修订：澄清 ADR-001 v4 的 Agent API，不改变 Message、Session 或 Run 聚合
- 来源：NEC-177

## 1. 背景

Ait 至少要支持两类执行后端：

1. 完整 Coder harness，例如直接调用 Codex app-server。它可以读取或修改工作区、运行命令、请求审批并返回 checkpoint。
2. 普通 LLM API，例如通过 OpenAI-compatible 协议调用 DeepSeek。它只负责模型推理，工具执行、持久化和恢复均由宿主管理。

当前原型已经分别验证了 `agent-adapters::AgentAdapter`、
`providers::ProviderAdapter`、worker 使用的 `ports::RunAgent`、Codex 纵向切片使用的
`ports::WorkspaceAgent`，以及 Session 命名使用的 `ports::SessionTitleGenerator`。这些接口各自合理，
但 application 必须知道具体 driver，并且同一个 Codex adapter 被包装成多种业务专用 trait；继续扩展会让每个
Agent 类型与每个直接调用用途形成笛卡尔积。

本 ADR 固定统一边界、两种上层调用方式及迁移方向。它不把 Codex Thread、Provider、SDK request 或
DeepSeek endpoint 提升为领域实体。

## 2. 决策摘要

1. application、runtime 和 worker 只依赖一个 `AgentInvoker` port。Codex 与
   OpenAI-compatible/DeepSeek 均在 composition root 之后表现为同一个接口；上层不得按 driver 分支。
2. `AgentInvoker` 表示一次 Agent 调用，不表示完整 Run。`RunCoordinator` 仍拥有 Message 持久化、工具循环、
   retry、queue drain、恢复和终止屏障。
3. 上层保留两种使用方式：
   - durable Run：worker 读取已固定的 Agent revision，循环调用 `AgentInvoker` 并提交领域状态；
   - direct invocation：application 对纯、短、无工具的生成任务调用同一个 `AgentInvoker`，默认不创建 Run。
4. 两种调用都必须先解析并固定一个 Agent revision。direct invocation 可以由用途绑定选择默认 Agent，
   但不能在失败时静默换模型或 driver。
5. Coder harness 与模型 Provider 的能力并不相同。统一接口通过 capability negotiation 和显式 execution
   profile 表达差异，而不是只保留最低公共能力，也不伪装 Codex 内部工具由宿主执行。
6. Session 标题、检索摘要和 Git commit 文案属于 direct invocation 的典型用途；真正的 Git 写入、commit、
   Message 更新和 Session 更新始终由宿主完成。

## 3. 统一 port

以下为目标形状；具体字段可以随实现细化，但所有权和语义固定：

```rust
#[async_trait::async_trait]
pub trait AgentInvoker: Send + Sync + 'static {
    fn capabilities(&self) -> AgentCapabilities;

    async fn invoke(
        &self,
        request: AgentRequest,
    ) -> Result<AgentEventStream, AgentError>;
}

pub struct AgentRequest {
    pub call_id: AgentCallId,
    pub purpose: AgentPurpose,
    pub input: AgentInput,
    pub workspace: WorkspaceAccess,
    pub tools: Vec<ToolDescriptor>,
    pub output: OutputContract,
    pub limits: AgentCallLimits,
    pub cancellation: CancellationToken,
}

pub enum AgentPurpose {
    RunAttempt { run_id: RunId, attempt_id: RunAttemptId },
    Direct { operation_id: OperationId, kind: DirectTaskKind },
}

pub enum AgentInput {
    MessagePath(Vec<ProjectedMessage>),
    Prompt(String),
}

pub enum WorkspaceAccess {
    None,
    ReadOnly { root: PathBuf },
    WorkspaceWrite { root: PathBuf },
}

pub enum OutputContract {
    Message,
    Text,
    JsonObject,
    JsonSchema(Value),
}

pub enum AgentEvent {
    TextDelta { text: String },
    ProposedMessage { sub_messages: Vec<SubMessage> },
    Usage(RunUsage),
    Checkpoint { opaque_id: String },
    Activity(AgentActivity),
    Completed { stop_reason: AgentStopReason },
}
```

`AgentResolver` 以已固定、无明文凭证的 `AgentRevisionSnapshot` 解析
`Arc<dyn AgentInvoker>`。resolver 和 adapter 在调用时按最小 scope 临时解析 `CredentialRef`；secret 不进入
request、checkpoint、日志、Message 或导出。

每个 stream 必须满足：

- `Completed` 恰好一次且为最后一个 event；stream error 结束当前 attempt，不等同于 Run 失败。
- `ProposedMessage` 必须完整且可由领域规则校验；token delta 和 SDK item 不是领域事实。
- `Checkpoint` 对 driver、Agent revision 和调用上下文不透明绑定，只能由同一兼容 adapter 恢复。
- capability 缺失必须在网络请求、子进程启动或工作区副作用前返回
  `AGENT_CAPABILITY_UNSUPPORTED`。
- `Activity` 只用于进度和审计投影，不能绕过 `RunStore` 修改 Message、Session 或 Run。

## 4. 两类后端如何适配

### 4.1 Coder harness：Codex

`CodexAgentInvoker` 包装 `agent-adapters::AgentAdapter`：

- `MessagePath` 由共享 context assembler 转换为明确分隔、不可修改的 prompt；`Prompt` 可用于直接调用。
- workspace profile 映射到 app-server sandbox；审批策略由宿主显式注入并默认 fail closed。
- Codex 的 thread id 映射为 opaque checkpoint，不替代 Ait 的 Session、Message 或 Run ID。
- command/file/MCP 等 Codex 内部行为投影为 `Activity`。只有能无损组成 Ait `ToolUse` / `ToolResult`
  协议的动态工具才进入 Message；不能把 harness 内部工具伪装成宿主 `RunTool`。
- workspace write 仅可用于 durable Run。direct invocation 必须是 `None` 或 `ReadOnly`，且禁用工具和审批。

### 4.2 模型 Provider：DeepSeek

`ProviderAgentInvoker` 包装 `providers::ProviderAdapter`。DeepSeek 首版复用现有
`OpenAiCompatibleProvider`，endpoint、model、能力声明和 `credential_ref` 均来自固定的 Agent revision：

- `MessagePath` 无损映射为 provider-neutral messages；`Prompt` 映射为一个 user message。
- Provider 的 tool-call stream 组装为一个完整 `ProposedMessage`。工具由 `RunCoordinator` 按
  ToolExecution 规则持久化、审批和执行。
- Provider 不获得 workspace capability。文件与命令能力只能来自宿主注册的工具。
- `JsonSchema` 只有在该 endpoint/model revision 明确声明支持时才能使用；否则调用前失败。
- HTTP 429、超时、连接失败和 5xx 的 retry directive 继续由 Provider adapter 给出，是否重试由调用方策略决定。

能力属于具体 Agent revision，不属于笼统的 driver 名称。例如同为 OpenAI-compatible 的两个模型可以分别
声明是否支持 tool calling、parallel tools、usage 或 structured output。

## 5. 能力模型

统一 capability 至少覆盖：

| 能力 | Codex harness | DeepSeek Provider | 所有者 |
| --- | --- | --- | --- |
| text / streaming | 是 | 是 | adapter |
| Message path | 经 context assembler | provider messages | adapter |
| structured output | app-server schema | 按模型声明 | adapter revision |
| host-managed tools | 仅显式 dynamic tool | 是 | RunCoordinator |
| harness-managed activity | 是 | 否 | harness adapter |
| workspace read/write | 按 sandbox profile | 否 | host capability grant |
| approval | 是 | 否；工具审批在 host | host policy |
| checkpoint/resume | Codex thread | 可选，首版否 | adapter |
| usage | 按 Codex event | 按 Provider event | adapter |

请求的 profile 是 capability 的子集。声明能力和实际 adapter 能力都必须满足请求；revision 声明不能扩大
adapter 或宿主授权。

## 6. 两种上层调用

### 6.1 durable Run

用户交互、Cron 和可恢复后台任务都创建领域 Run。worker 的执行顺序保持不变：

1. 从 bootstrap 读取已固定的 `agent_id + revision`，通过 `AgentResolver` 得到 invoker。
2. 加载 root-to-head Message path，构造 `AgentPurpose::RunAttempt`。
3. 消费 event；只把完整 ProposedMessage、usage 和 checkpoint 提交给 daemon。
4. 对 host-managed ToolUse 持久化 intent、审批、执行并追加 ToolResult，再调用同一 invoker。
5. 处理 retry、恢复和 queue，最后请求 Run 终止屏障。

Coder harness 返回 `Completed` 只表示一次 turn 完成，Provider 返回 stop 也只表示一次模型调用完成；两者都不能
直接把 Run 写成 `completed`。

### 6.2 direct invocation

direct invocation 用于结果很小、纯生成且由调用方立即消费的内部能力，例如：

- Session title 与检索 description；
- 基于已确定 diff/任务摘要生成 Git commit subject/body；
- UI 中的短摘要、分类或候选标签。

它默认不创建 Message、Session 或 Run，但必须具有 request/operation id、超时、输入上限、输出 contract 和
取消信号。调用方验证结果后，在自己拥有的事务中持久化最终字段；失败只保留原值或返回稳定错误，不能留下
半完成领域状态。

以下任一条件成立时必须创建 durable Run，不能使用 direct invocation：

- 需要修改工作区、执行工具或请求审批；
- 需要用户可见的过程、Message lineage 或可恢复 checkpoint；
- 需要跨进程恢复、queue/steer、长期 retry、token/cost budget 的 durable 结算；
- 结果本身就是成员委托的工作，而不是另一个领域操作的派生字段。

因此“用一个简单 Run 生成标题”在语义上可行，但不是默认方案：它会制造无意义的 Message 树和 Run 历史。
若未来需要审计每次生成尝试或可靠地跨重启恢复，再由调用方提升为普通 Run，而不改变 Agent adapter。

## 7. Agent 选择与 revision

durable Run 沿用 ADR-001：创建事务内固定 `agent_id + revision`。direct invocation 也必须在调用开始前固定
revision，但选择策略由 application 拥有：

- 调用方可显式指定 Agent；或
- 配置一个 `DirectTaskKind -> AgentId` 的用途绑定，例如 `session_metadata`、`commit_message`。

用途绑定只能选择 Agent，不能覆盖该 revision 的 endpoint、credential 或权限。调用开始后，配置更新不影响
本次请求。失败时不得从 DeepSeek 静默切到 Codex，反之亦然；显式 fallback 必须是上层可观察的新 attempt，
并遵守同一成本与权限策略。

## 8. 错误、重试与审计

`AgentError` 统一提供稳定 kind、安全 message 与 retry directive，并由 adapter 分别映射
`ProviderError` 和 `AdapterError`。认证、权限、无效请求、协议不兼容和能力缺失不可重试；限流遵守
Retry-After；连接、超时和服务端暂时错误使用有界退避；取消不重试。

durable Run 将每次调用记入 RunAttempt，并由 Run policy 决定重试。direct invocation 默认最多执行一次；只有
调用方确认操作纯且幂等时才能做短、有界重试。日志记录 operation/run/attempt、Agent revision、driver kind、
延迟、usage 和错误 kind，不记录 prompt、返回正文、credential 或 Codex 登录态。

## 9. crate 边界与迁移

目标依赖方向：

```text
application / runtime / worker
             │
             ▼
      ait-ports::AgentInvoker
          ▲              ▲
          │              │
ait-agent-adapters   ait-providers
    (Codex)          (DeepSeek/OpenAI-compatible)
```

分阶段迁移：

1. 在 `ait-ports` 新增统一 request/event/capability/error 与 `AgentInvoker` / `AgentResolver`，建立共享
   conformance tests。
2. 新增 `CodexAgentInvoker` 和 `ProviderAgentInvoker`；保留现有底层 app-server/HTTP protocol trait 作为 crate
   内 SPI。
3. 让 `RunCoordinator` 和 worker 只使用 `AgentInvoker`，把当前 `RunAgent` 收敛为协调器内部 collector 或删除。
4. 让 `LocalControlService` 通过 direct invocation 完成 Session metadata，删除业务专用
   `SessionTitleGenerator`；Git commit 文案复用同一路径。
5. 用 resolver registry 替代 application 中的 `AgentMode::Codex` 分支，随后删除
   `WorkspaceAgent` 和仅用于测试的 mode 路由。

迁移完成前允许兼容 wrapper 存在，但新业务不得新增 `XxxGenerator` 或按 driver 命名的 application port。

## 10. 验收条件

- 同一个无工具 Message fixture 分别经 fake Codex 和 fake OpenAI-compatible/DeepSeek adapter 产出相同的
  ProposedMessage 语义，并通过统一 conformance suite。
- worker/RunCoordinator 测试只注入 `dyn AgentInvoker`，测试代码不匹配 driver。
- Session metadata 和 Git commit 文案 direct invocation 可显式选择任一兼容 Agent；不会创建 Message/Run，
  不会取得 workspace write 或工具权限。
- capability mismatch 在网络、spawn 和文件副作用前失败；取消、终止 event、usage、tool ordering 和错误分类均有
  contract tests。
- application、runtime 与 worker 中不存在 `if driver == codex/deepseek` 一类执行分支；具体协议只存在于 adapter
  crate 与 composition root。

## 11. 不做的事

- 不把 Provider、Codex Thread 或 SDK item 变成一级领域对象。
- 不要求 Codex 与 DeepSeek 暴露完全相同的能力，也不模拟不存在的 workspace、approval 或 checkpoint 能力。
- 不让 direct invocation 绕过权限、凭证解析、输入输出限制或结果校验。
- 不由 Agent adapter 写数据库、推进 Session、创建 Git commit 或决定 Run terminal。
- 不在本 ADR 中定义 Agent catalog 的 SQLite schema、用途绑定 UI 或远程 worker wire DTO；这些按迁移阶段另行实现。
