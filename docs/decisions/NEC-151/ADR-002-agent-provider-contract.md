## ADR-002：Agent 配置与 Provider Adapter 契约

- 状态：Proposed，原型已纳入 workspace 并验证
- 依赖：ADR-001 v4
- 范围：单次 provider 调用边界；不拥有 Session、Message 持久化或 Run 终止屏障
- 来源：NEC-151

## 决策

1. `AgentDefinition` 是可发布配置，任何变更都追加 revision；Run 创建时只解析一次并持久化 `agent_id + revision`。
2. Provider 接收统一的 `ProviderRequest`，返回有序 `ProviderEvent` stream。事件覆盖文本增量、ToolUse 生命周期、Usage 和唯一终止 Stop。
3. ToolUse 仅能出现在 assistant Message；ToolResult 必须是只含一个结果部件的 user Message。Adapter 只做供应商映射，不改变领域语义。
4. 请求先以“配置声明能力”和“Adapter 实际能力”校验，缺少 streaming/tool calling/parallel tool calls/usage/system message 时以 `CapabilityUnsupported` 在网络调用前失败。
5. 错误统一为稳定 kind 与 RetryDirective：认证/权限/参数/协议不可重试；429 尊重 Retry-After；超时、连接失败和 5xx 退避重试；取消不可重试。
6. 持久化对象只含 `CredentialRef`。每次调用由 `CredentialResolver` 临时解析 `SecretValue`；secret 不实现序列化，Debug 固定脱敏。
7. 首批 Adapter 为本地确定性 `ScriptedProvider` 和远程 `OpenAiCompatibleProvider`。远程 endpoint 是 Agent revision 的一部分。
8. 新 Adapter 必须复用 `verify_stream_contract`，并补充自身的请求映射、错误分类和流解析测试。

## 事件约束

- 每个 tool index 严格遵循 Start → ArgumentDelta* → End。
- Usage 可选，但一旦提供，`total >= input + output`。
- Stop 必须且只能出现一次，并且是最后一个事件。
- Adapter stream 错误终止当前 attempt；是否在同一 Run 重试由上层 supervisor 按 RetryDirective 决定。

## 集成边界

`ait-providers` 不写 Message、不推进 Session、不决定 Run completed。上层 Run supervisor 将流重组成 ProposedMessage，先持久化再进入工具/下一轮，并最终通过 ADR-001 的终止屏障。这样 Provider 更换不会绕过 Message 不可变、Session CAS、工具审计与恢复语义。

## 已知后续项

- 随 domain/ports 实现推进，把当前原型中的 Agent catalog 与 Provider port 进一步下沉到稳定领域/端口边界。
- Agent revision 改由 SQLite repository 持久化，并在 Run 创建事务内 pin。
- CredentialResolver 接入 macOS Keychain / Windows Credential Manager / Linux Secret Service。
- 为目标远程供应商增加录制回放和真实凭证 opt-in smoke test；默认 CI 仍只运行无网络 contract tests。
