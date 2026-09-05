# ADR-001：Session Message 推进与来源元数据

- 状态：Accepted，已实现
- 日期：2026-09-05
- 来源：NEC-173
- 依赖：ADR-001 v4、NEC-161 ADR-003、NEC-174 ADR-001

## 决策

1. 常规 Session 输入和后续 Assistant 回复都推进原 Session 的 `current_message_id`；Assistant 完成不会创建新 Session。只有用户显式选择历史 Message 并执行 “Branch from here” 时才创建新 Session。
2. 每条新建 Message 都在不可变 `metadata.git.commit_id` 中记录追加时 Project 仓库的 Git HEAD。尚无首个提交时该值为 `null`；工作树是否干净不改变 HEAD 的含义。
3. 每条 Assistant Message 都在 `metadata.agent` 中记录实际生成者：`id` 为固定到 Run 的 Agent identity，`revision` 为该 Run 固定的配置 revision。
4. Codex 本轮新建提交继续额外记录在 `metadata.codex.commit_id`。它与通用 Git 字段语义不同：前者表示本轮是否创建提交，后者表示 Message 落盘时观察到的 HEAD。
5. 桌面端保留 `assistant` 作为协议 role，但作者界面使用 `metadata.agent.id` 解析具体 Agent 名称；旧数据缺少 provenance 时显示 `Agent`，不伪装成某个配置。

## 元数据形状

```json
{
  "git": { "commit_id": "<sha-or-null>" },
  "agent": { "id": "<agent-id>", "revision": 3 },
  "codex": { "commit_id": "<new-sha>" }
}
```

`agent` 只出现在 Assistant Message；`codex` 只在 Codex 确实创建新提交时出现；`git` 出现在所有本地新建 Message。元数据均为非敏感审计信息，并随 Message 一起不可变持久化和导出。

## 一致性

User Message 与 Run 创建仍在同一状态提交中完成。同步 Agent 输出与 Session head 推进同事务持久化；Codex 的外部执行与 Git commit 完成后，Assistant Message、Session head、Run terminal 状态和 provenance 再原子写入控制存储。失败不会创建 Assistant Message，也不会新建替代 Session。
