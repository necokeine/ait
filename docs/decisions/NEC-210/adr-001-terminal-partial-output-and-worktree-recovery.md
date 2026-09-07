# ADR-001：终止 Run 的部分输出与工作区恢复

- 状态：Accepted
- 日期：2026-09-07
- 关联：NEC-210、NEC-205、NEC-204、NEC-174

## 背景

NEC-205 已把 Codex 实时事件投影为 daemon 持有的有界进度 checkpoint，但失败、app-server EOF、用户取消或 Git 提交失败时，Run 终态此前只保留错误。进程仍在时可见的解释、操作与未完成回答会在刷新后消失；Codex 已写入 Project 的文件则继续留在工作区，却没有持久化的检查入口或安全续跑协议。

核心领域基线要求 Message 是追加且不可变的，因此失败过程不能伪装成一条 assistant Message。恢复也不能自动丢弃、提交或盲目重试工作区修改。

## 决策

### 终态部分输出属于 Run

Run 新增可选的 `partial_output` 投影。执行以失败或取消结束时，Application 在写入终态的同一个 CAS 更新中，把 NEC-205 的进度 checkpoint 复制到 `Run.partial_output.progress`，再清理临时 checkpoint。该投影随 Run 持久化，daemon 重启或桌面刷新后仍可读取。

进度消息项携带 `completed` 标记：完整收到的 commentary/message 与 operation 完成事件为已确认；仅收到 delta 而尚未完成的 item 为未完成。存储继续沿用 NEC-205 的有界 checkpoint，不引入无界事件日志。

失败 Run 不追加 assistant Message。只有成功执行的最终答复继续进入不可变 Message 历史。

### 工作区状态可检查且不被自动处置

终止时检查 Project 工作树，将 HEAD、dirty 标志、截断后的变更路径列表与精确指纹写入 `Run.partial_output.worktree`。展示最多 256 个路径；指纹覆盖完整 porcelain 状态、暂存和未暂存 diff，以及未跟踪文件或符号链接的内容，因此不以可见列表的截断结果做一致性判断。

系统不会因失败、EOF、取消或提交失败自动 reset、discard 或 commit。桌面端独立展示错误、已确认输出、未完成输出与保留的工作区变化，并提供打开 Project 目录的检查入口。

### 继续执行是显式且受保护的新 Run

用户只能从尚未产生 assistant Message 的 Codex 终止 Run 发起 `ContinueRun`，并必须提交界面所见的工作区指纹。Application 在创建新 Run 前再次计算指纹；Adapter 在开始调用 Codex 前再校验一次。工作区已变、已变干净、原 Run 不可恢复或已经存在恢复子 Run时拒绝执行。

恢复不会修改原 Run，而是创建 `trigger=recovery` 的新 Run，记录 `recovery_of_run_id`，复用原 Run 固定的 Session、Agent revision、Provider 配置与 base Message。恢复指令明确要求先检查并保留现有修改，避免重复外部副作用。成功后仍走既有的单次 Git commit 与 assistant Message 收尾路径。

## 结果

- 刷新或 daemon 重启不再丢失终止前的有界输出与工作区检查信息。
- Message 聚合保持只追加、不伪造失败回答。
- 用户可以明确选择继续，但工作区内容变化会使旧确认失效。
- 恢复 Run 形成可追溯链；同一个失败 Run 不会被重复续跑。
- 精确指纹需要读取 diff 与未跟踪文件内容，成本高于只读 Git status，但只发生在终止归档与显式恢复边界。

## 验证

- fake app-server/adapter 覆盖 confirmed commentary、completed operation、unfinished delta 后的失败、EOF 与流错误。
- Application 集成测试覆盖归档后重载、内容变化导致的指纹拒绝、还原后继续成功，以及取消后的状态保留。
- Git pre-commit hook 失败测试覆盖提交错误后部分输出与已暂存工作区仍可检查。
- Desktop 测试覆盖 error、partial confirmed/unfinished、retained worktree 与恢复操作的区分展示。
