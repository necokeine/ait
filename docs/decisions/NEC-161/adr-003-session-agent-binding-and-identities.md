## ADR-003：Session-Agent 绑定与 Message UUID

- 状态：Accepted
- 日期：2026-09-04
- 来源：NEC-161 实现评审
- 修订：ADR-001 v4 中的 Message identity、Project description 与 Session-Agent 语义

## 背景

ADR-001 v4 将 Session 定义为可移动 Message ref，并允许每次 Run 选择不同 Agent。实现评审进一步明确：用户交互和 Agent 运行的最小连续上下文应是 Session，而不是单个 Run；同一 Session 不应在运行之间隐式切换 Agent。另外，Message 需要跨进程、数据库和导入导出保持无协调唯一性。

## 决策

1. `MessageId` 使用 UUID 值类型。领域层持有 `Uuid`，serde 使用标准连字符字符串；SQLite 继续使用 `TEXT` 作为存储表示。
2. `Project.description` 为非空类型 `String`；未提供描述时保存空字符串，不使用 `Option`/`NULL` 表示同一语义。
3. `Session.agent_id` 为必填且在 Session 生命周期内固定，替代可空的 `default_agent_id`。
4. Session 是交互式 Agent 的最小运行单元。跟随 Session 创建的 Run 必须使用 `Session.agent_id`，并在 Run 创建时固定当时的 Agent revision。
5. 从现有 Message 切换 Agent 时创建新的 Session；不修改旧 Session，也不复制 Message 历史。
6. Cron 和显式无 Session Run 仍可直接选择 Agent，不受 Session 绑定约束。

## 不变量与存储影响

- nil UUID 不是有效的 `MessageId`。
- `sessions.agent_id` 为 `NOT NULL`。
- `runs.follow_session_id` 非空时，`runs.agent_id = sessions.agent_id`。
- `projects.description` 为 `NOT NULL DEFAULT ''`。
- 这是当前尚未发布 schema PoC 的修订，不需要兼容已部署数据库；正式 migration 仍须单调递增。

## 后果

Session API 创建时必须显式接收 Agent。发送后续输入不再重复接收 Agent 参数，从而消除“Session 默认 Agent”和“Run 实际 Agent”不一致的状态。需要并排比较不同 Agent 时，可让多个 Session 指向同一 Message。
