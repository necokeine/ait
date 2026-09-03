# 核心术语速查

- **Project**：拥有一个工作目录和元数据；目录根是 Git 仓库。如果指定目录尚非 Git 仓库，初始化为新仓库。
- **Message**：不可变的对话树节点，角色为 `system`、`user` 或 `assistant`。它类似 Git commit。
- **SubMessage**：Message 内部的有序内容节点；ToolUse 是 assistant Message 中的一种 SubMessage。
- **ToolResult**：承载工具结果的一种 user Message，并关联对应 ToolUse。
- **Session**：指向 Message 树上当前节点的可移动引用，类似 Git branch。用户可从任意节点打开 Session；新增 Message 后，Session 指针向新叶子移动。
- **Agent**：提供一套 API；从 Session 当前 Message 出发，持续生成包含普通内容、ToolUse 与相应 ToolResult 的新节点。
- **Run**：某个 Agent 基于 Session 当前 Message 发起的一次完整运行。只有重试、压缩恢复与新队列均无需继续处理时才结束。
- **RunAttempt**：Run 中的一次可重试执行尝试；多个 Attempt 仍属于同一个 Run。
- **Cron**：绑定 Agent 与 Session 起点，按照时间安排定期启动新 Run 的计划。
- **Provider Adapter**：把稳定的 Agent/Run 契约映射到具体模型或外部 Agent 协议的适配层。

完整定义、不变量和关系以 `decisions/NEC-150/adr-001-core-domain-model-v4.md` 为准。
