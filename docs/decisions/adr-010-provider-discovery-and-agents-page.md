# ADR-010：Provider 两段式配置与独立 Agents 页面

- 状态：Accepted
- 日期：2026-09-07
- 来源：用户要求填写 API URL / Secret 后自动发现并选择模型，以及把命名 Agent 配置从 Settings 移至独立页面。
- 修订：ADR-009 的桌面配置入口；其 Session 独占、配置归属与 Run 快照规则继续有效。

## 模型发现与保存分离

新增 `DiscoverProviderModels { provider, secret? }`，通过 `POST /v1/agent-provider/discover-models` 暴露。请求中的 Provider 与保存接口使用相同结构，包含 `id/name/kind/url/models`；返回 `ProviderModels(Vec<ProviderModel>)`，JSON 结果种类为 `provider_models`。

该操作校验连接并请求实际模型 API，但不提交 ControlStore、不发布配置事件、不保存凭证，也不改变任何 Agent、Session 或已有模型目录。提供 Secret 时，应用层通过 `AgentProviderGateway.list_models_with_secret` 使用仅在当前请求中存在的凭证；未提供时，通过 Provider ID 解析现有凭证引用。SDK、HTTP 和操作系统凭证库仍位于 adapter，领域不引入这些依赖。Secret 沿用只写、Debug 脱敏类型，不出现在响应、事件、快照或日志中。

发现结果保留已有模型的 reasoning effort 声明；远端未提供的能力不从模型名称推断。只有显式 `SaveAgentProvider` 保存用户选择的模型子集及凭证。旧的 `RefreshProviderModels` 保留其立即更新目录的 API 语义，桌面配置向导改用无持久化副作用的发现接口。

## 桌面流程

Settings 的 Providers 页面提供连接列表和两段式向导：

1. 填写名称、API 类型、URL 和 Secret，点击下一步时自动拉取模型。已有 Secret 可留空复用；Codex / 内置 Provider 使用已有目录与主机认证。
2. 展示可搜索的模型列表，由用户勾选并保存。新发现的模型默认不选中；已有选择与能力声明保留。正在被 Agent 使用的模型保留勾选并标明用途；未被 API 返回的已保存模型单独标注。未选择模型时不能保存。

加载或保存错误显示在当前步骤，用户可修改连接并重试。返回列表、关闭 Settings 或切换设置类别会丢弃未保存的连接和 Secret；迟到的发现结果不会恢复已经关闭的向导。失败或取消发现不会留下已注册的空 Provider。

侧栏 Agents 切换主工作区到独立页面，展示所有 Provider、已启用模型与能力，并管理命名 Agent 的创建和编辑。Provider 卡片链接到 Settings 中对应的连接。命名 Agent 编辑器只选择已保存的模型和 reasoning effort；Session 私有匿名 Agent 不出现在命名配置列表中。切回 Sessions 保留当前 Session、消息草稿和分支位置。

Settings 不再提供命名 Agent 编辑器。原 `agents.max_steps` / `agents.parallel_tools` 全局执行偏好仍由 Rust schema 管理，桌面将其类别显示为 Execution，保留原设置 ID 和存储兼容性。

## 验证

应用测试覆盖草稿凭证发现无写入、保存所选子集、复用已保存凭证、保留能力声明，以及失败和非法连接无部分配置。桌面测试覆盖显式模型选择、重新发现不自动启用新模型、已引用模型保留。界面验证覆盖两步流程、错误重试、空列表、取消后迟到响应、命名 Agent 编辑和页面导航。
