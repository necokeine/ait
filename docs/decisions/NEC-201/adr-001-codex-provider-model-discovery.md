## ADR-001：Codex Provider 动态模型发现

- 状态：Accepted
- 日期：2026-09-07
- 来源：NEC-201
- 修订：ADR-010 的主机认证 Provider 发现路径；ADR-009 的目录、配置与 Run 快照规则继续有效。

## 决策

1. Codex Provider 配置界面在展示模型选择器前，通过本机已登录的 `codex app-server` 调用分页 `model/list`，不再把持久化目录中的单个默认模型当作完整可选列表。
2. adapter 将 picker 可见条目的 `model`、`displayName` 和 `supportedReasoningEfforts[].reasoningEffort` 投影为既有 `ProviderModel`；不从名称推断推理等级，也不请求 hidden 模型。
3. application 通过独立的 `HostProviderModelCatalog` port 使用该能力。Codex 发现不接受 Secret；远端 OpenAI/DeepSeek 继续使用 `AgentProviderGateway` 及凭证引用。
4. `DiscoverProviderModels` 仍是无持久化副作用的预览。只有用户保存后，所选子集才进入 Provider catalog；已有 Agent 使用中的模型继续受保存校验保护。
5. 现有内置 Codex 默认模型保留为离线启动和首次 Agent 创建的安全基线。配置界面每次进入模型步骤都重新发现，因此 Codex CLI 后续新增、下架或调整能力时无需发布新的 Ait 静态名单。

## 后果

- 用户可以选择当前账号与本机 Codex CLI 实际提供的全部 picker 可见模型，并得到逐模型的真实 reasoning effort 选项。
- Codex 未安装、未登录或 app-server 协议失败时，配置向导保留当前 Provider 和 Agent 配置并显示可重试错误。
- 模型发现会启动一个短生命周期 app-server 子进程；完成或失败后由 adapter 终止并回收，不创建 Thread、Turn、Message 或 Run。

## 验证

- adapter 协议测试覆盖初始化、`model/list` 分页、展示名和 reasoning effort 投影。
- application 测试覆盖主机目录发现、拒绝 Secret 及预览不写入 ControlStore。
- desktop 配置流程对 Codex 与远端 Provider 一样，在进入模型选择步骤时调用发现接口。
