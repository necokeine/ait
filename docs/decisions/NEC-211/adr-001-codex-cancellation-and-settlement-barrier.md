# ADR-001：Codex 取消、进程退出与 Git 结算屏障

- 状态：Proposed（NEC-211 实现，待评审）
- 日期：2026-09-08
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
   handler 和事件通道背压。每次 adapter invocation 生成独立随机值作为工具进程归属，并在 initialize
   后调用 `config/read` 读取当前 cwd 的有效 shell environment policy；随后通过
   `thread/start|resume.config` 的 dotted request override 设置
   `shell_environment_policy.set.AIT_CODEX_PROCESS_OWNER`。若有效策略存在非空 legacy `include_only`，
   则只在该列表追加 marker 名；若 canonical `filters` 已包含 include action，则追加 marker 的 include
   action；不存在白名单时不创建白名单。该方式保留用户的 `inherit`、exclude 与白名单语义，也不把
   用户 `set` 中可能敏感的值复制进协议请求。持久化的 `CODEX_THREAD_ID` 只用于 Codex thread 关联，
   不作为 Run ownership。ACK 或终态缺失超过期限后，Unix 按本 invocation 的 marker 反复扫描、冻结、
   强杀并等待到空集合稳定；app-server PID/进程组负责 root、同组进程以及 marker 尚未进入工具环境前
   的握手阶段。身份不依赖 thread id 或清理时 PPID，因此同 thread 的预存进程不会被命中，而
   app-server 先退出、`setsid()` 后代被 reparent，或扫描期间发生 fork+exit 都不会丢失本 Run 归属。
   Windows 使用 app-server PID 的 `/T` tree。
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
  落库。Unix 进程回收按 request-scoped shell policy 中的随机 invocation marker 识别工具进程，不按
  持久 thread id、进程名、工作目录或清理时 PPID 猜测；app-server PID/进程组与 marker 分别覆盖
  root/同组进程和可脱组/reparent 的工具后代。同 thread 的预存或并行进程不携带本次 marker，因而
  不会被命中。
- daemon 崩溃后的持久恢复和无法确认的 Git/SQLite 对账仍由 NEC-212 负责；失败/取消部分输出保留由
  NEC-210 负责；同 Project 并发工作区隔离由 NEC-209 负责。

## 验证

- 协议 fake 覆盖 `config/read`、`thread/start|resume.config`、legacy `include_only` 与 canonical `filters`
  的 marker 接线，以及 interrupt ACK/终态延迟、ACK 缺失、阻塞 approval handler 和已满事件通道。
- Unix 进程测试 4/4 覆盖 initialize 握手取消、resume 握手取消时同 thread 预存进程保持存活、
  `inherit=none/core + include_only` 过滤后的 marker 环境、`setsid()` 直接子进程/孙进程、app-server
  先退出与扫描期间 fork+exit；本 Run 后代全部退出而无关及同 thread 预存进程存活。
- application fake 覆盖重复取消、取消后立即发送、迟到成功、Session 延迟释放、Message 不追加与
  commit 审计，并覆盖 OpenAI/DeepSeek pending 请求的快速取消。
- 临时 Git 仓库和延迟 pre-commit hook 覆盖 commit 开始后的 shielded settlement，以及取消后
  hook 失败的结构化审计。
- desktop 单元测试覆盖 stopping 与 cancelled-after-commit 展示。
- 本地通过 `cargo fmt --all -- --check`、`git diff --check`、workspace clippy（`-D warnings`）和
  workspace tests；关键套件为 `codex_protocol` 8/8、`codex_workspace` 36/36、`run_execution` 13/13、
  `session_agent_config` 28/28。Desktop typecheck、58/58 tests 与 build 通过。
- Codex CLI 0.153.4 的离线探针确认 `experimentalApi=false` 时 `config/read` 返回有效 shell policy。
  Windows `taskkill /T` 真实机和付费模型调用未实测，保持非阻断验证边界。
