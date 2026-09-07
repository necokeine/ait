# ADR-008：Control 命令内部完成 Run 执行

- 状态：Accepted
- 日期：2026-09-07
- 依赖：ADR-001 v4、NEC-174 ADR-001
- 后续修订：NEC-203 移除了测试型本地 Provider 模式及 `apply_run_mode`。

## 背景

`LocalControlService::execute` 原先根据 `CommandResult::Run` 中的 `queued` 状态启动 workspace Agent。这使返回给调用方的 DTO 同时承担执行信号，读取 queued Run 或重复触发已有 Cron Run 也可能启动外部执行。

## 决策

1. `execute` 只将 `try_execute` 的成功结果或错误包装成 API `Response`，不根据返回 DTO 产生副作用。
2. `try_execute` 负责完整命令编排：只读命令直接读取快照；修改命令先提交状态，再完成命令明确要求的外部执行，最后返回结果。
3. 内部 `CommandOutcome` 区分已就绪结果与待执行 workspace Run 的 ID。新建可执行 Provider Run 的 `SendMessage`、`ForkSession` 和 `TriggerCron` 分支产生执行指令；`RunView.status` 不用于调度。
4. 提交重试只重新计算和提交状态。Agent 调用位于 `commit_command` 成功返回之后，不进入 CAS 重试循环；完成状态的提交冲突也不重复调用 Agent。
5. 实际执行的命令等待 Run 成功、失败或取消后返回持久化结果。执行失败记录在 `RunView.status/error`；API `Response.ok` 表达命令是否被正确处理。基础设施错误仍可返回 API 错误。
6. 查询返回当前快照；Cron 幂等重放返回已有 Run，即使它仍处于 queued，也不隐式恢复执行。后续恢复应由显式恢复操作或 worker 协议承担。
7. 所有 Provider kind 都代表真实 adapter：Codex 通过 `WorkspaceAgent` port，OpenAI/DeepSeek 通过 Provider gateway。queued/running/terminal 三个持久化检查点保持原有边界；测试用中间状态通过 store/executor seam 注入，不进入生产目录。领域 Run 的终止屏障、Message 不可变性和 Session 指针语义不变。

## 验证

- SendMessage、ForkSession 和 TriggerCron 在 Agent 执行结束并持久化结果后才返回。
- queued、running、terminal 各检查点注入提交冲突时，外部调用仍只执行一次；创建提交失败时不执行 Agent。
- Provider 失败返回持久化的 failed Run 并释放 Session。
- 查询 queued Codex Run 不改变存储，也不执行 Agent；重复 Cron 触发不重复执行。
- 取消 queued/running Run 仍释放 Session；工具审批恢复由 runtime 状态机独立验证。
