# ADR-001：Codex 工作区写入租约与 Run 隔离

- 状态：Accepted
- 日期：2026-09-07
- 依赖：ADR-001 v4、NEC-149 ADR-001、NEC-174 ADR-001、ADR-009
- 修订：替代 NEC-174 中 Codex 直接在 Project workdir 执行并 `git add --all` 的实现约束

## 背景

Session 独占只能阻止同一 Session 重入。多个 Session 仍可共享一个 Project workdir，路径别名或多个 daemon 实例也可能指向同一实际 Git 工作区。若 Codex 直接修改该目录并在结束时提交整个 worktree，另一个 Run 或成员在运行期间写入的内容会被错误归属，启动前的 clean 检查无法关闭这个竞态窗口。

## 决策

1. 会写入工作区的 Codex 请求必须在捕获 human Message 的 Git HEAD 前取得 Project 工作区写入租约，并持有到 Run 外部调用退出、终态持久化完成。租约以 canonical Project Git root 为进程内键；同一服务的竞争写入异步串行，不同 Git root 独立运行。
2. Git metadata 目录内的 advisory lock 补充进程内租约。另一个进程持锁时立即返回可重试的 `PROJECT_WORKSPACE_BUSY`，不得先追加 Message、创建 Run 或推进 Session。规范化路径及同一 Git worktree 的别名因此共享准入边界。
3. 每个获准的 Codex Run 将租约下捕获的完整 HEAD 固定为 `workspace_base_commit`，并从该基线创建 manager-owned detached linked worktree。Codex 只获得该隔离目录作为 cwd 和 workspace-write 沙箱边界；Project 主工作区、其他 Run 与隔离目录之间不共享 index 或可写文件。
4. Adapter 只在隔离 worktree 中对该 Run 的变化执行 `git add --all` 和提交。提交必须是授权 baseline 的后代，并先固定到 `refs/ait/runs/<request-hash>`，因此自动提交不会包含主工作区或其他 Run 的文件。
5. 整合前再次校验 Project 主工作区的完整 HEAD、symbolic HEAD、index、tracked worktree 和 untracked files。它们必须与启动基线一致且 clean；满足时仅允许 `--ff-only` 更新。HEAD/分支变化返回 `PROJECT_GIT_HEAD_UNAVAILABLE`，工作区或 index 变化返回 `PROJECT_GIT_DIRTY`。不得自动 reset、clean、覆盖或吸收外部变化。
6. 整合冲突、提交失败或 Provider 失败时，有变化的隔离 worktree 与 Run ref 保留，错误中返回恢复位置；没有变化的 manager-owned worktree 可安全移除。成功整合后移除隔离 worktree 和临时 Run ref，commit id 仍由 assistant Message 的 Codex metadata 记录。
7. 取消先传播给 Adapter 并给子进程收尾机会。即使调用方超时退出，Adapter 的受管任务也继续完成子进程回收；取消后不得再整合主工作区。有部分变化时沿用第 6 条的保留语义。重启遇到同一 Run 的既有 worktree/ref 时返回 `RUN_RECOVERY_FAILED`，不猜测旧执行是否结束，也不与它重叠写入。

## 只读与写入准入

- Session title/model discovery 等只读调用不取得写入租约，可与写入型 Run 及其他只读调用并行；它们没有 Git 提交权限。
- 同一 Project 的写入型 Codex Run 串行。等待租约期间仍持有各自 Session 的进程内许可，避免输入或配置越过排队请求。
- 不同 canonical Git root 的写入 Run 可并行。远程文本 Provider 不取得工作区写入租约。
- Cancel 与查询不等待工作区写入租约；Cancel 可立即持久化并通知当前调用退出，租约只在调用已经不能整合主工作区后释放。

## 恢复与可见性

保留目录位于 Project Git metadata 下的 `ait/workspaces/<request-hash>`，对应 ref 为 `refs/ait/runs/<request-hash>`。它们是 manager-owned 恢复材料，不出现在 Project status 中。当前切片选择明确拒绝自动复用既有恢复材料；后续恢复操作必须先证明旧执行已终止，再显式选择整合、导出或删除，不能隐式清理。

## 验证

- 两个同 Project Session 的写入请求以屏障同时发起，后一个在前一个终态后重新捕获 HEAD；两个提交各只包含本 Run 的文件。
- 不同 Project 的写入请求可同时进入 Adapter；独立 `LocalControlService` 无法绕过 advisory lock。
- 运行中注入未跟踪文件、Git index 变化和 HEAD 前移均拒绝整合；成员文件/index 保持原样，隔离 Run commit 由 worktree/ref 保留。
- Codex 的既有成功、dirty-at-start、输出时间线及取消测试继续使用离线 fake，不依赖付费模型调用。
