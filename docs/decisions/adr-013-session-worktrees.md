# ADR-013：Session 固定 linked worktree 与工作目录

- 状态：Accepted
- 日期：2026-09-12
- 依赖：ADR-001 v4、NEC-209、NEC-212
- 修订：以 Session worktree 取代 Session-bound Run 直接以 Project 主检出作为工作目录的约束

## 背景

Session 是可长期继续的交互分支，但此前文件执行的入口仍以 Project 主工作区为准。即使 Codex Run 内部另建临时隔离 worktree，Session 自身没有稳定的文件分支；API Provider 的宿主工具也直接作用于 Project 主检出。这样两个 Session 的 Message 分支与文件分支并不一致，成员主检出也容易被 Session 工具改脏。

## 决策

1. 每个 Session 固定拥有一个 manager-owned detached linked worktree。目录必须是规范 Project Git root 下的 `.ait/<session-id>`；`.ait` 是唯一父目录，Session id 必须是非空、无路径分隔符、非 `.`/`..` 且不含控制字符的单一路径组件。
2. 创建、Fork 或需要从旧记录补建 Session 时，从目标 Message 可证明的最近 Codex commit、human Message Git commit 或 `Project.base_commit` 创建 worktree。创建使用 `git worktree add --detach --no-checkout` 后受控 `reset --hard`，不执行 `post-checkout` hook。Project 的本地 `.git/info/exclude` 加入 `/.ait/`，因此主检出的 status 不包含 manager-owned worktree。
3. `Session.workdir`/`SessionView.workdir` 持久化规范绝对路径，并且必须严格等于 `<Project workdir>/.ait/<session-id>`。旧记录缺少该字段时可确定性补齐；已保存但不匹配的路径失败关闭，不能成为文件系统授权。
4. 所有带 `follow_session_id` 的 Run 从 Session worktree 捕获 HEAD/index 基线。Codex 的 Session-bound invocation、API Provider 宿主工具、原生审批路径和 Session 标题生成均使用 Session worktree 作为 cwd 与 workspace 边界。没有 Session 的 Cron Run 继续以 Project 工作区为入口。
5. NEC-209 的 Project 级进程内租约和 advisory lock 保留，同 Project 的写入结算继续串行。Codex adapter 仍可在其内部为单个 Run 创建临时隔离 worktree，但候选提交发布到 Session worktree，而不是 Project 主检出；Run ref、checkpoint、取消 gate、补偿日志和重启对账语义不变。
6. 不同 Session 从同一 Message 打开时，其 linked worktree 可以从同一 commit 分叉；一个 Session 的成功提交不会推进另一个 Session 或 Project 主检出的 HEAD。后续 human Message 的 `git_commit` 记录本 Session worktree 的 HEAD。
7. portable Project archive 不携带源主机的 Session workdir。导入时在目标 Project 的 `.ait` 下重建所有 Session worktree；归档 Message/Session 身份保持不变，本地路径重新派生。
8. 文件系统与控制存储是两个提交域。worktree 创建成功但 Session 记录提交失败时，不猜测或删除可能被并发写入的目录；错误明确返回保留路径，要求成员检查后重试或处理。

## 后果

- Project 主检出保持成员所有；Session 的文件修改、Git HEAD 和未跟踪文件不会串到其他 Session。
- Desktop、HTTP、CLI 与恢复查询可直接从 Session DTO 得到真实工作目录。
- API 工具若留下未提交修改，下一次输入会因该 Session worktree 不干净而拒绝；系统不会静默把修改归入另一个 Session。
- Session 删除/归档目前不自动删除 worktree；显式安全回收需要单独设计，不能在本 ADR 中隐式执行破坏性清理。

## 验证

- 创建 Session 后存在 `<Project>/.ait/<session-id>/.git`，Session DTO 的 workdir 与其 Git top-level 一致，Project 主检出 status 仍为空。
- 两个 Session 从同一基线运行时分别获得独立 commit 和文件集合，Project HEAD 不移动。
- Session worktree 中的 dirty 文件阻止输入且不产生 Message/Run；Project 主检出中的 Session 管理目录不触发 dirty 拒绝。
- Codex invocation、API 工具和标题生成收到 Session workdir；startup recovery 在同一目录验证 baseline/ref/index/worktree。
- 非法 Session id、符号链接 `.ait`、非 linked-worktree 占位目录及不匹配的持久 workdir 全部失败关闭。
- 导出清空本地 Session workdir，导入在目标 Project 下重建对应 worktree。
