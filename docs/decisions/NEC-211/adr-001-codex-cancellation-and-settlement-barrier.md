# ADR-001：Codex 取消、进程退出与 Git 结算屏障

- 状态：Proposed（NEC-211 实现，待评审）
- 日期：2026-09-07
- 依赖：ADR-001 v4、ADR-009、NEC-154 ADR-002、NEC-169 ADR-001、NEC-205 ADR-001、NEC-209 ADR-001、NEC-210 ADR-001
- 修订：NEC-174 ADR-001 中取消时立即释放 Session、Codex 调用与 Git 提交作为一个可丢弃 future 的实现方式

## 决策

1. `CancelRun` 对 queued Run 直接保存 `cancelled`；对已经执行的 Run 先保存非终态
   `cancelling` 和 `RUN_CANCELLED` 意图，再通知调用取消。重复取消返回同一快照，不重复产生事件。
2. `cancelling` 期间保留 Session 的 `active_run_id`。只有调用 future 返回、Run 拥有的
   app-server 进程树已经退出、进度通道已经排空且 Git 结算得到确定结果后，application 才保存
   `cancelled` 并释放 Session。新输入继续按 ADR-009 立即返回 `SESSION_BUSY`，不会与旧执行重叠。
3. `WorkspaceAgent::invoke` 的返回是可等待的退出/结算屏障。普通取消不能由 application
   `select!` 丢弃该 future；实现必须在返回前回收本次 invocation 拥有的进程并完成已经开始的
   外部副作用。OpenAI/DeepSeek API 请求不拥有本地工作区结算，仍通过 drop HTTP future 快速取消，
   避免等待远端请求超时。
4. Codex turn 建立前收到取消时直接回收进程树；建立后发送一次带目标 thread/turn 的
   `turn/interrupt`，等待目标 `turn/completed`，宽限期为 2 秒。同一绝对期限覆盖协议读写、审批
   handler 和事件通道背压。每个 app-server 在 spawn 时取得只属于该 Run 的环境所有权标记，正常
   继承到工具后代；ACK 或终态缺失超过期限后，Unix 按该标记反复扫描、冻结、强杀并等待到空集合
   稳定，进程组只作补充安全网。身份不依赖 PPID，因此 app-server 先退出、`setsid()` 后代被
   reparent，或扫描期间发生 fork+exit 都不会丢失归属。Windows 使用同一 PID 的 `/T` tree。
5. Git 提交开始前再次检查取消。若取消已经记录，不启动 Git；一旦在 NEC-209 隔离 worktree 中进入
   Git 结算，`git add`、hook、commit 和 HEAD 读取不可因普通取消中途丢弃。Application 的 cancel
   与 Adapter 的主工作区发布共享同一 finalization gate：cancel 先赢时隔离 commit/ref 保留但不发布，
   integration 先赢时 cancel 返回 `RUN_ALREADY_TERMINAL` 并可靠完成结果落库。取消竞态不追加成功
   assistant Message，但将确定的 commit SHA 保存到 Run 的 `workspace_commit_id` 供审计与后续对账。
   若 add、hook、commit 或 HEAD 读取失败，则在 Run error 的结构化 details 中同时保留操作阶段、
   失败代码/消息、前后 HEAD、index/worktree dirty 状态、可能的 commit 与 NEC-210 恢复句柄。
6. NEC-205 的状态通道继续区分 `running`、`settling`、`cancelling` 与终态。桌面在 active Run 上
   提供停止入口，并在屏障完成前显示正在停止/结算；只有持久化 `cancelled` 后才显示取消完成。

## 一致性结果

- 持久化取消意图、停止请求与取消终态不再是同一个瞬间。
- 迟到的成功输出不能覆盖取消状态或追加不可变 Message；已经完成的 Git commit 不会成为无归属副作用。
- Session 与 Project workspace lease 覆盖完整外部执行、隔离 Git 结算、finalization gate 和可靠终态
  落库。Unix 进程回收按 spawn 时的 Run 唯一继承标记识别，不按进程名、工作目录或清理时 PPID
  猜测；脱组/reparent 后代仍可识别，预先存在或无关进程没有该标记而不会被命中。
- daemon 崩溃后的持久恢复和无法确认的 Git/SQLite 对账仍由 NEC-212 负责；失败/取消部分输出保留由
  NEC-210 负责；同 Project 并发工作区隔离由 NEC-209 负责。

## 验证

- 协议 fake 覆盖 interrupt ACK/终态延迟、ACK 缺失、阻塞 approval handler 和已满事件通道。
- 进程测试覆盖握手取消、`setsid()` 脱组工具、脱组工具孙进程、app-server 先退出、扫描期间
  fork+exit，以及无关预先存在进程。
- application fake 覆盖重复取消、取消后立即发送、迟到成功、Session 延迟释放、Message 不追加与
  commit 审计，并覆盖 OpenAI/DeepSeek pending 请求的快速取消。
- 临时 Git 仓库和延迟 pre-commit hook 覆盖 commit 开始后的 shielded settlement，以及取消后
  hook 失败的结构化审计。
- desktop 单元测试覆盖 stopping 与 cancelled-after-commit 展示。
