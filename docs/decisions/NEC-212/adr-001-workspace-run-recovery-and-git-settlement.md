## ADR-001：Workspace Run 恢复与 Git/结果结算

- 状态：Accepted
- 日期：2026-09-07
- 来源：NEC-212
- 协调：NEC-169 worker lease；NEC-206 Codex thread 恢复；NEC-180/182 invoker 迁移

## 背景

当前 control-plane 先让 Codex 修改工作区并创建 Git commit，随后才把 assistant Message 和 Run 终态写入 SQLite。daemon 在两个动作之间退出时，代码可能已经提交，但 Run 仍为 `running` 且 Session 一直被 `active_run_id` 占用；若直接重放 Codex，又可能重复工具副作用或 Git commit。

控制面已有 `runtime.recovery = resume_safe | ask | fail` 设置，但启动入口没有扫描遗留 Run，也没有把该设置落实为行为。查询快照和 `GetRun` 必须继续保持只读，不能成为隐式执行入口。

## 决策

### 持久执行身份与结果日志

每个新 Run 在 `queued` 时获得稳定的 `operation_id = workspace-<run_id>`。开始执行时以 SQLite snapshot CAS 把 Run 切到 `running/calling_agent`、递增 `lease_epoch`，并原子创建 `workspace_run_journals[run_id]`。日志保存：

- `operation_id` 与当前 `lease_epoch`；
- Agent 开始前的 `expected_head`；
- Agent 完成时观察到的 `observed_head`；
- 完整 `WorkspaceAgentResponse`；
- `commit_id` 与是否已经结算。

`WorkspaceAgent::invoke_and_checkpoint` 在成功返回前调用 application 提供的 `WorkspaceResultSink`。结果 checkpoint 与 `run.result_persisted` outbox 事件同一事务提交，并把 Run 切到 `finalizing/result_persisted`。生产 Codex adapter 不再创建 Git commit；因此可见工作区副作用跨过 Git 边界前，最终文本和操作时间线已经进入持久日志。

每次 checkpoint、Git 后终态写入和恢复 claim 都同时校验 `operation_id + lease_epoch`。恢复或取消递增 epoch；旧执行者最多返回迟到结果，不能覆盖新 lease 下的 Run、Message 或 Session 指针。

### 幂等 Git 结算

application 只对已 checkpoint 的结果结算 Git：

1. HEAD 必须仍等于结果 checkpoint 时的 `observed_head`；否则保留现状并把 Run 呈现为 `interrupted`，等待用户处理。
2. 工作区无变化时，HEAD 相对 `expected_head` 未变化表示无需 commit；已变化表示 Codex 自己产生了一个可关联的 commit。
3. 工作区有变化时，application 统一 `git add --all` 后创建 commit，并写入唯一 trailer `Ait-Operation-Id: <operation_id>`。
4. commit 成功但 SQLite 最终写失败时，下一次恢复先检查当前 HEAD 的 trailer；命中即复用同一 commit，不重复提交。
5. 最终事务一次性追加不可变 assistant Message、保存 commit 关联、把 Run 切到 `completed/terminal`、条件释放匹配 Session，并写 outbox 事件。CAS 冲突可重复尝试而不会重复 Message 或 commit。

### 启动恢复策略

daemon 在开始监听前显式调用 `recover_interrupted_runs`。只有这个启动入口可以触发恢复；`Snapshot`、`GetRun` 和其他查询不执行 Run。

| 遗留状态 | `resume_safe` | `ask` | `fail` |
| --- | --- | --- | --- |
| `queued` | 取得新 lease 后执行 | `interrupted`，释放 Session | `failed`，释放 Session |
| `finalizing` 且结果日志完整 | 取得新 lease，幂等补 Git/Message/终态 | `interrupted`，释放 Session | `failed`，释放 Session |
| `running` 或结果未知 | `interrupted`，不重放，保留工作区 | `interrupted`，保留工作区 | `failed`，保留工作区 |
| `cancelling` | 完成 `cancelled` 终态 | `interrupted` | `failed` |
| 已有终态 | 不修改、不执行 | 不修改、不执行 | 不修改、不执行 |

`interrupted` 是需要用户查看工作区和 Run 错误的终态，桌面端按终态失败展示；它不会永久占用 Session。`resume_safe` 目前不恢复未知的 Codex 原生 thread：只有 NEC-206 提供已持久化且证明安全的 thread/checkpoint 后，`running` 才能新增自动续接分支。也不因本 ADR 自动重放原生工具。

## 边界与后果

- Message 历史仍只追加；恢复日志不是 Message，也不把 Codex 原生 operation 伪装成 Ait ToolUse/ToolResult。
- 结果日志暂随 control snapshot 保留，便于审计 commit 与最终 Message 的关联；后续拆表或迁入统一 invoker 时必须保留 operation/lease/结果的幂等语义。
- daemon 启动会等待安全 queued/finalizing 工作完成后再监听。后续若改为 supervisor 并发恢复，必须先完成同样的扫描和 lease claim，且不能削弱 fencing。
- 若结果 checkpoint 本身不可写，Git 不会由 application 提交；Run 保留非终态供下一次启动明确标记或恢复，工作区变化不会被清理。

## 验证

离线 fake 注入并覆盖：queued 启动恢复、未知 running 中断、Agent 完成且 Git 前退出、Git 已提交但最终 store 失败、最终 CAS 冲突耗尽、重复恢复、`ask/fail` 策略、Session 释放、单一 Message/commit、以及只读查询无执行副作用。真实 Codex thread 续接仍归 NEC-206，不作为本 ADR 的自动测试前置。
