# ADR-001：Workspace Run 恢复与 Git/结果结算

- 状态：Accepted
- 日期：2026-09-07
- 来源：NEC-212
- 协调：NEC-209 隔离 worktree 与补偿发布；NEC-169 worker lease；NEC-206 Codex thread 恢复；NEC-180/182 invoker 迁移

## 背景

当前 control-plane 由 Codex adapter 在每 Run 隔离 worktree 中创建 commit，并通过 NEC-209 的锁定补偿协议发布到主 Project；application 随后才把 assistant Message 和 Run 终态写入 SQLite。daemon 在发布与最终落库之间退出时，代码可能已经可见，但 Run 仍为 `running`/`settling` 且 Session 一直被 `active_run_id` 占用；若直接重放 Codex，又可能重复模型、工具或 Git 副作用。

控制面已有 `runtime.recovery = resume_safe | ask | fail` 设置，但启动入口没有扫描遗留 Run，也没有把该设置落实为行为。查询快照和 `GetRun` 必须继续保持只读，不能成为隐式执行入口。

## 决策

### 持久执行身份与结果日志

每个新 Run 在 `queued` 时获得稳定的 `operation_id = workspace-<run_id>`。开始执行时以 SQLite snapshot CAS 把 Run 切到 `running/calling_agent`、递增 `lease_epoch`，并原子创建 `workspace_run_journals[run_id]`。日志保存：

- `operation_id` 与当前 `lease_epoch`；
- Agent 准入时的 symbolic ref（HEAD 与精确 index tree 已由 Run 的 NEC-209 baseline 字段保存）；
- 完整 `WorkspaceAgentResponse`，包括隔离 worktree 内创建并固定到稳定 Run ref 的 `commit_id`。

`WorkspaceAgent::invoke_with_progress_and_checkpoint` 在 Codex 完成输出、在隔离 worktree 创建并固定 Run commit 后，调用 application 提供的 `WorkspaceResultSink`；只有 checkpoint 与 `run.result_persisted` outbox 事件同一事务提交并把 Run 切到 `settling/result_persisted` 后，adapter 才能进入 NEC-209 的主 worktree 发布协议。这样最终文本、操作时间线和候选 commit identity 都先于外部可见 Git 发布进入持久日志。

每次 checkpoint、Git 后终态写入和恢复 claim 都同时校验 `operation_id + lease_epoch`。恢复或取消递增 epoch；旧执行者最多返回迟到结果，不能覆盖新 lease 下的 Run、Message 或 Session 指针。
所有 terminal 快捷返回也必须先校验这两个字段；只有持有同一 lease 且提交相同结果的重复
checkpoint/finalize 才作为幂等成功。`commit_id` 只由受信任的 workspace adapter 在隔离 Run ref 下产生；恢复时必须同时验证 authorized baseline、稳定 Run ref、候选 commit、原 symbolic ref 和主 worktree/index，不能把任意 adapter 字符串当作已发布结果。

### 幂等 Git 结算

1. 同一 Project 从 human Message Git 快照、隔离 workspace-write、result checkpoint、Git 发布到 Message/Run 终态持有 NEC-209 的进程内租约和 Git advisory lock；不同 Project 仍可并行。
2. 正常执行仍完全使用 NEC-209 的隔离 worktree、Run ref、精确 index/ref transaction、path quarantine 与补偿日志；本 ADR 不引入第二套主 worktree 发布实现。
3. `settling/result_persisted` 的恢复先判断主 ref 是否已经精确到达 journal 中的候选 commit：若 HEAD/index/worktree/ref identity 一致，直接补最终 SQLite 事务；若主 ref 仍在 authorized baseline，则只在稳定 Run ref 精确指向候选 commit、隔离 worktree 可验证且没有未决 rollback material 时继续同一 Git 发布。
4. HEAD、index、symbolic ref、Run ref、候选 ancestry 或 recovery material 任一不匹配都转为可见 `interrupted`，保留现状供人工处理；不得重新调用模型、自动 reset/clean 或猜测补偿状态。
5. 最终事务一次性追加不可变 assistant Message、保存 commit 关联、把 Run 切到 `completed/terminal`、条件释放匹配 Session，并写 outbox 事件。CAS 冲突和 store 暂时失败可安全重试，不重复 Message 或 Git commit。

### 启动恢复策略

daemon 在监听前只调用 `prepare_startup_recovery`：一次快速 store 扫描原子标记不可恢复 Run、
并 claim 可恢复 queued/settling lease。完成 bind/readiness 后，受 daemon Tokio 生命周期管理的
supervisor 才执行计划中的 Agent/Git 长任务。只有这组显式启动入口可以触发恢复；`Snapshot`、
`GetRun` 和其他查询不执行 Run。

| 遗留状态 | `resume_safe` | `ask` | `fail` |
| --- | --- | --- | --- |
| `queued` | 取得新 lease 后执行 | `interrupted`，释放 Session | `failed`，释放 Session |
| `settling` 且结果日志完整 | 取得新 lease，幂等对账 Git/Message/终态 | `interrupted`，释放 Session | `failed`，释放 Session |
| `running` 或结果未知 | `interrupted`，不重放，保留工作区 | `interrupted`，保留工作区 | `failed`，保留工作区 |
| `cancelling` | 完成 `cancelled` 终态 | `interrupted` | `failed` |
| 已有终态 | 不修改、不执行 | 不修改、不执行 | 不修改、不执行 |

`interrupted` 是需要用户查看工作区和 Run 错误的终态，桌面端按终态失败展示；它不会永久占用 Session。`resume_safe` 目前不恢复未知的 Codex 原生 thread：只有 NEC-206 提供已持久化且证明安全的 thread/checkpoint 后，`running` 才能新增自动续接分支。也不因本 ADR 自动重放原生工具。

单个 Run 的 missing workdir、invalid repository、不可读 HEAD/ref 或 Git identity 不匹配都转为
`interrupted` 并条件释放 Session，supervisor 继续下一项；只有全局 control store 无法读取或提交
claim/终态时才停止本轮恢复。Desktop snapshot 将所有 `interrupted` Run 投影成包含
Project/Session/Run 定位信息的持久处理提示。

## 边界与后果

- Message 历史仍只追加；恢复日志不是 Message，也不把 Codex 原生 operation 伪装成 Ait ToolUse/ToolResult。
- 结果日志暂随 control snapshot 保留，便于审计 commit 与最终 Message 的关联；后续拆表或迁入统一 invoker 时必须保留 operation/lease/结果的幂等语义。
- daemon readiness 不等待安全 queued/settling 长任务；NEC-209 工作区租约使 supervisor 中同一
  Project 的任务串行，不同 Project 可独立恢复。
- 若结果 checkpoint 本身不可写，adapter 不进入主 worktree 发布；隔离 worktree/Run ref 按 NEC-209 保留，Run 供下一次启动明确标记或恢复。

## 验证

离线 fake 注入并覆盖：queued 启动恢复、未知 running 中断、完整 checkpoint 不重跑 Agent 即终态结算、稳定 lease fencing、Session 释放、单一 Message，以及只读查询无执行副作用。真实 Git 测试在 result checkpoint 后阻断首次发布，验证恢复严格复用隔离 Run commit、只发布一次并清理 Run ref；NEC-209 继续覆盖主 worktree/index/ref 的并发、补偿与外部改动矩阵。daemon 集成测试让 fake Agent 阻塞超过 desktop 15 秒 readiness 窗口，验证 daemon 先可用、不会被 launcher 判死且 Run 最终只执行一次。Desktop 测试覆盖带 Project/Session/Run 定位的持久 `interrupted` 提示。真实 Codex thread 续接仍归 NEC-206，不作为本 ADR 的自动测试前置。
