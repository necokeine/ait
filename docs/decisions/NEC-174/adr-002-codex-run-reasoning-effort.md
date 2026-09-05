# ADR-002：Codex Run 推理强度覆盖

- 状态：Accepted
- 日期：2026-09-05
- 依赖：ADR-001 v4、NEC-161 ADR-003、NEC-174 ADR-001

## 背景

Codex app-server 允许在 `turn/start.params.effort` 覆盖模型推理强度，并由 `model/list` 为每个模型广告可选值和默认值。AIT 需要让桌面用户选择该值，同时保持 Session 作为 Message 指针和 Agent 绑定的既有语义。

## 决策

1. 推理强度是一次 Run 的可选、固定覆盖值，不加入 Session 聚合，也不修改 Agent revision。
2. 桌面仅在 Agent 投影明确提供 `supportedReasoningEfforts` 时显示选择器；默认值来自同一投影。当前内置 `gpt-5.6-sol` 的投影与 Codex 0.151.0 `model/list` 保持一致。
3. `SendMessage` 和 `ForkSession` 接受可选的 `reasoning_effort`。application 在持久化 user Message 前验证 Agent mode 和已知模型能力，并将有效值持久化到 `RunView`。
4. `WorkspaceAgentInvocation` 将固定值传给 Codex adapter，adapter 映射到 `turn/start.params.effort`。省略值时由 Codex 使用模型默认值。
5. 非 Codex Agent 的既有请求继续省略该字段；若外部调用方强行为非 Codex Agent 提供值，application 在产生 Message 或 Run 前返回配置错误。

## 后果

- 重试或后续恢复可以从持久化 Run 读取相同覆盖值，不依赖 renderer 临时状态。
- 新 Codex 模型必须先提供经过验证的能力投影，桌面才会展示其选项；不能把内置模型的枚举臆测应用到其他模型。
- Session 的指针、Agent 绑定、CAS 和活动 Run 约束保持不变。
