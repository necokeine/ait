# 核心术语速查

- **Project**：拥有一个 Git 工作目录、注册时冻结的 `base_commit`、可选 `repo_url` 和元数据；无 HEAD 的新仓库会获得一个空初始提交。
- **Message**：不可变的对话树节点，角色为 `system`、`user` 或 `assistant`。普通 human user Message 只在仓库干净时创建，并记录当时的 `git_commit`。
- **SubMessage**：Message 内部的有序内容节点；ToolUse 是 assistant Message 中的一种 SubMessage。
- **ToolResult**：承载工具结果的一种 user Message，并关联对应 ToolUse。
- **Session**：指向 Message 树上当前节点的可移动引用，类似 Git branch。用户可从任意节点打开 Session；新增 Message 后，Session 指针向新叶子移动。
- **AgentProvider**：共享的连接、认证引用与模型能力目录。
- **Agent**：引用 Provider 的版本化运行配置，包含 model 与 reasoning effort；可以是可复用命名预设，或单个 Session 的匿名配置。
- **Run**：某个 Agent 基于 Session 当前 Message 发起的一次完整运行。只有重试、压缩恢复与新队列均无需继续处理时才结束。
- **RunAttempt**：Run 中的一次可重试执行尝试；多个 Attempt 仍属于同一个 Run。
- **Cron**：绑定 Agent 与 Session 起点，按照时间安排定期启动新 Run 的计划。
- **Provider Adapter**：把稳定的 Agent/Run 契约映射到具体模型或外部 Agent 协议的适配层。

完整定义、不变量和关系以 `decisions/NEC-150/adr-001-core-domain-model-v4.md` 为准。

当前 daemon 在 Session 有 active Run 时拒绝新的用户消息；发送与改配置均不要求客户端传 version。见 [ADR-009](decisions/adr-009-session-exclusion-and-agent-providers.md)。
