# ADR-001：Codex Session 执行与 Git 提交闭环

- 状态：Accepted，已实现首个本地纵向切片
- 依赖：ADR-001 v4、NEC-151 ADR-003、NEC-169 ADR-001
- 来源：NEC-174

## 决策

1. 新增显式 `codex` Agent mode；桌面内置 Codex Agent 使用该 mode，不再以固定 echo 文本模拟执行。
2. application 只依赖 `WorkspaceAgent` port。daemon 在组合根注入 `CodexWorkspaceAgent`，具体 app-server 协议仍封装在 `agent-adapters`。
3. Session 输入先原子持久化 user Message、推进 Session 并创建 queued Run；提交成功后才调用外部 Codex。调用前另行持久化 running 状态。
4. Codex invocation 使用 Project 的规范化 Git root 作为 cwd 和 workspace-write 沙箱边界，并把当前 Message 根到 user Message 的不可变路径组装为 prompt。
5. Codex app-server 的 message delta（无 delta 时使用最终 agentMessage item）组装为 assistant result。成功后 application 原子追加 assistant Message、推进 Session、完成 Run 并释放 `active_run_id`。
6. Codex 不自行提交。宿主仅在调用前工作树完全干净时允许运行，并在 turn 成功后执行 `git add --all` 与单个 Git commit；commit SHA 保存于 assistant Message 的结构化 `codex.commit_id` 元数据。
7. 若 Codex 或 Git 失败，Run 进入 failed 并释放 Session；不会伪造 assistant result 或完成状态。调用前已有未提交改动时直接拒绝，避免把用户工作混入 Agent commit。
8. UI 将 legacy echo mode 明确显示为 `Echo · echo`。空闲 Session 的 Agent 下拉选择会以 version CAS 原地重绑；活动 Run 期间保持禁用，每个已创建 Run 的 Agent revision 不受后续重绑影响。

## 一致性边界

外部 Agent 执行和 Git commit 无法与 SQLite 处于同一事务。当前纵向切片采用三个 durable checkpoint：queued、running、terminal。SQLite 的 user Message/Run 创建先于外部副作用，assistant Message/Session 推进后于成功 commit。并发终态写入不会被 Codex 完成覆盖。

完整的崩溃恢复、取消传播与 operation id 去重继续由 NEC-169 定义的 worker/daemon 协议承接；本实现不把 app-server 或 Git 副作用放入 SQLite CAS 重试循环。

## 验证

- 内存 Agent 验证 Session 输入生成 assistant Message、Run completed、Session 释放及 commit SHA 持久化。
- fake Codex adapter 在临时 Git 仓库生成文件，验证 assistant result、真实 commit 与 clean worktree。
- dirty worktree 测试验证 Codex 不会启动，也不会提交既有用户改动。
