# ADR-001：Codex 取消、进程退出与 Git 结算屏障

- 状态：Proposed（NEC-211 实现，待评审）
- 日期：2026-09-07
- 依赖：ADR-001 v4、ADR-009、NEC-154 ADR-002、NEC-169 ADR-001、NEC-205 ADR-001
- 修订：NEC-174 ADR-001 中取消时立即释放 Session、Codex 调用与 Git 提交作为一个可丢弃 future 的实现方式

## 决策

1. `CancelRun` 对 queued Run 直接保存 `cancelled`；对已经执行的 Run 先保存非终态
   `cancelling` 和 `RUN_CANCELLED` 意图，再通知调用取消。重复取消返回同一快照，不重复产生事件。
2. `cancelling` 期间保留 Session 的 `active_run_id`。只有调用 future 返回、Run 拥有的
   app-server 进程树已经退出、进度通道已经排空且 Git 结算得到确定结果后，application 才保存
   `cancelled` 并释放 Session。新输入继续按 ADR-009 立即返回 `SESSION_BUSY`，不会与旧执行重叠。
3. `WorkspaceAgent::invoke` 的返回是可等待的退出/结算屏障。普通取消不能由 application
   `select!` 丢弃该 future；实现必须在返回前回收本次 invocation 拥有的进程并完成已经开始的
   外部副作用。
4. Codex turn 建立前收到取消时直接回收进程树；建立后发送一次带目标 thread/turn 的
   `turn/interrupt`，等待目标 `turn/completed`，宽限期为 2 秒。ACK 或终态缺失超过期限后，
   只强杀 spawn 时创建的独立进程组（Windows 使用同一 PID 的 `/T` tree），并等待直接子进程退出。
5. Git 提交开始前再次检查取消。若取消已经记录，不启动 Git；一旦进入 Git 结算，`git add`、
   hook、commit 和 HEAD 读取不可因普通取消中途丢弃。取消与 commit 并发时不追加成功 assistant
   Message，但将确定的 commit SHA 保存到 Run 的 `workspace_commit_id` 供审计与后续对账。
6. NEC-205 的状态通道继续区分 `running`、`settling`、`cancelling` 与终态。桌面在 active Run 上
   提供停止入口，并在屏障完成前显示正在停止/结算；只有持久化 `cancelled` 后才显示取消完成。

## 一致性结果

- 持久化取消意图、停止请求与取消终态不再是同一个瞬间。
- 迟到的成功输出不能覆盖取消状态或追加不可变 Message；已经完成的 Git commit 不会成为无归属副作用。
- Session 写入所有权覆盖完整外部执行和 Git 结算期。进程树回收只按本次 spawn 的独立组执行，
  不按进程名或工作目录扫描，因此不会误杀预先存在或无关进程。
- daemon 崩溃后的持久恢复和无法确认的 Git/SQLite 对账仍由 NEC-212 负责；失败/取消部分输出保留由
  NEC-210 负责；同 Project 并发工作区隔离由 NEC-209 负责。

## 验证

- 协议 fake 覆盖 interrupt ACK/终态延迟及 ACK 缺失超时。
- 进程测试覆盖握手取消、直接子进程、继承进程组的孙进程，以及无关预先存在进程。
- application fake 覆盖重复取消、取消后立即发送、迟到成功、Session 延迟释放、Message 不追加与
  commit 审计。
- 临时 Git 仓库和延迟 pre-commit hook 覆盖 commit 开始后的 shielded settlement。
- desktop 单元测试覆盖 stopping 与 cancelled-after-commit 展示。
