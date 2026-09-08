# ADR-001：DeepSeek adapter-owned reasoning effort 目录

- 状态：Accepted
- 日期：2026-09-09
- 来源：NEC-218
- 修订：ADR-009 中远程 Provider 发现一律不提供 reasoning effort 的约定。

## 决策

1. DeepSeek adapter 为 `/models` 返回的每个模型补充有序的
   `off`、`low`、`high`、`max` reasoning effort。这是 DeepSeek API 方言的
   adapter-owned 能力，不是根据模型名称推断的能力。
2. `off` 继续映射为 `thinking.type=disabled` 且不发送
   `reasoning_effort`；其余三个等级发送 `thinking.type=enabled` 与对应的
   `reasoning_effort`。adapter 在网络 I/O 前拒绝目录以外的 DeepSeek 等级。
3. Provider 重新发现模型时，adapter 返回的非空能力目录优先于旧的保存值；
   若 adapter 未提供能力，继续保留旧的人工声明。这让已有空目录的 DeepSeek
   Provider 在下一次发现或刷新后得到能力，同时不为 OpenAI `/models` 猜测等级。
4. 桌面端继续使用通用 `ProviderModel.reasoning_efforts` 投影。用户选择并保存
   DeepSeek 模型后，Agent 和 Session 对话框沿用现有 Codex 控件显示 reasoning
   effort 选择器，无需按 Provider 类型增加第二套 UI 状态。

## 依据

DeepSeek Harness 的官方 adapter 将 `off`、`low`、`high`、`max` 作为所有已解析
模型的有序 reasoning effort 目录；部署未禁用 thinking 时该目录同时适用于目录内
和未登记模型。Ait 采用相同的 adapter-owned 能力边界，但仍由实际 `/models`
结果决定哪些模型可供用户选择。

参考：

- [DeepSeek Harness adapter source](https://github.com/deepseek-ai/deepseek-harness/blob/c389f96bf3a9b6807cb71ed6bdad5849be0df6d8/packages/llm/llm-deepseek/src/adapter.ts)
- [DeepSeek Harness adapter contract](https://github.com/deepseek-ai/deepseek-harness/blob/c389f96bf3a9b6807cb71ed6bdad5849be0df6d8/packages/llm/llm-deepseek/README.md)

## 后果

- 新配置的 DeepSeek API 在模型选择后即可在对话框中选择四个等级。
- 已保存但没有等级的 DeepSeek Provider 需要执行一次模型发现或刷新；刷新会把
  adapter 目录持久化，之后配置校验与 Run 快照使用同一组值。
- OpenAI 以及其他不公布能力的标准模型列表仍可保留人工声明，不会被空发现结果清除。

## 验证

- adapter 测试固定四个等级的顺序，并验证未知 DeepSeek 等级在请求前失败。
- application 测试覆盖非空 adapter 能力覆盖旧值、空能力保留人工声明，以及刷新持久化。
- desktop 测试覆盖发现合并与 Agent 投影，使非空 DeepSeek 能力进入现有对话框选择器。
