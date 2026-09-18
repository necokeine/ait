# ADR-001：全局 Default Agent、Small Agent 与保留 System Prompt

- 状态：Accepted
- 日期：2026-09-17
- 来源：NEC-301
- 修订：ADR-001 v4 的 Project/Session Agent 选择，以及 NEC-176 的 Session 元数据生成模型

## 决策

1. 全局 Settings 增加 `agents.default_agent`。当创建 Session、派生分支或 Cron 没有显式 Agent 时，按 `Project.default_agent_id → agents.default_agent` 解析；显式选择始终优先。Project 可以清除自身覆盖并恢复全局默认。
2. Settings 增加 `agents.small_agent`，用于 Session 标题、检索摘要等短调用。未配置时回退到全局 Default Agent；兼容旧数据时，若两项均未配置，标题生成沿用当前 Session Agent。
3. 两个全局引用只能指向启用的命名 Agent，不能指向 Session 私有 Agent。Settings schema 使用 `agent_reference` 让客户端从实时 Agent catalog 渲染选择器。
4. `AgentConfiguration.system_prompt` 是可选、非敏感、可持久化字段，并随 Agent/Run 配置快照保留。当前所有 Codex/API prompt 组装都必须忽略它；后续启用需要单独设计其指令优先级和安全边界。
5. Session 元数据生成仍是不创建 Message、Run 或 Git 提交的只读调用，但不再固定模型。应用层解析 Small Agent 的 provider/config/credential reference；Codex 使用该 Agent 的 model/reasoning，OpenAI/DeepSeek 使用相同的一次性文本完成边界。System prompt 不参与该调用。

## 兼容与失败语义

- 旧 Settings 记录在读取时补齐新增键，值默认为空，不覆盖已有配置。
- HTTP/Command 中创建 Session、Fork、Derive、Cron 的空或省略 `agent_id` 表示请求默认解析；已有显式字符串保持原语义。
- 没有任何可解析的默认 Agent、引用不存在或 Agent/Provider 不可用时，操作失败，不静默选择任意 Agent。
- API Small Agent 缺少凭据或返回不合法元数据时保留临时 Session 标题，并沿用既有一次性生成失败语义。

## 验证

- application 测试覆盖全局默认解析、Project 覆盖清除、Cron 回退、引用校验、Small Agent 标题配置选择及重启持久化。
- adapter 测试覆盖配置的 Codex model/reasoning，并证明已保存的 system prompt 不进入请求。
- desktop 测试覆盖无 Project 覆盖的创建路径、Agent 引用设置和 system prompt 编辑持久化。
