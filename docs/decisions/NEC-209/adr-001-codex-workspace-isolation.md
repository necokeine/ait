# ADR-001：Codex 工作区写入租约与 Run 隔离

- 状态：Accepted
- 日期：2026-09-07
- 依赖：ADR-001 v4、NEC-149 ADR-001、NEC-174 ADR-001、ADR-009
- 修订：替代 NEC-174 中 Codex 直接在 Project workdir 执行并 `git add --all` 的实现约束

## 背景

Session 独占只能阻止同一 Session 重入。多个 Session 仍可共享一个 Project workdir，路径别名或多个 daemon 实例也可能指向同一实际 Git 工作区。若 Codex 直接修改该目录并在结束时提交整个 worktree，另一个 Run 或成员在运行期间写入的内容会被错误归属，启动前的 clean 检查无法关闭这个竞态窗口。

## 决策

1. 会写入工作区的 Codex 请求必须在捕获 human Message 的 Git 基线前取得 Project 工作区写入租约，并持有到 Run 外部调用退出、终态持久化完成。应用层为每个 Run 启动独立于 HTTP/调用方 future 的 supervisor；它共同持有 Session 许可、工作区租约、取消注册和终态持久化责任，调用方断连或 drop 不得释放这些责任。租约以 canonical Project Git root 为进程内键；同一服务的竞争写入异步串行，不同 Git root 独立运行。
2. Git metadata 目录内的 advisory lock 补充进程内租约。另一个进程持锁时立即返回可重试的 `PROJECT_WORKSPACE_BUSY`，不得先追加 Message、创建 Run 或推进 Session。规范化路径及同一 Git worktree 的别名因此共享准入边界。
3. 每个获准的 Codex Run 将租约下捕获的完整 HEAD 和精确 index tree 分别固定为 `workspace_base_commit`、`workspace_base_index_tree`，并从该基线创建 manager-owned detached linked worktree。准入前后两次读取 HEAD/index，且 index tree 必须等于 HEAD tree。Codex 只获得隔离目录作为 cwd 和 workspace-write 沙箱边界；Project 主工作区、其他 Run 与隔离目录之间不共享 index 或可写文件。
4. 隔离工作区通过 `git worktree add --no-checkout` 后受控 `reset --hard` 创建，避免执行仓库 `post-checkout` hook。任一步失败都必须验证 forced remove、prune、目录删除和 worktree registration 的最终结果；只有清理完整时才删除 Run ref，清理不完整则返回 `RUN_RECOVERY_FAILED` 并保留 ref/路径恢复句柄。Adapter 只在验证后的隔离 worktree 中对该 Run 的变化执行 `git add --all` 和提交。提交必须是授权 baseline 的后代，并先固定到 `refs/ait/runs/<request-hash>`，因此 setup hook 或其他 Run 的文件不能被错归。当前切片明确拒绝主工作区已初始化的 submodule，并给出 deinitialize/拆分 Project 的可操作错误，不能让 Codex 静默看到空 submodule。
5. 最终整合采用带补偿日志的锁定发布协议而非“检查后再 merge”：先完成可失败的隔离 worktree 清理，再 prepare 带旧 OID 的 Git ref transaction；attached HEAD 固定并直接更新准入时的精确 symbolic target ref，detached HEAD 固定为 no-deref `HEAD`，后续 branch switch 不得改变补偿目标。目标 ref 的 direct identity 必须在 prepared transaction 持有精确 ref lock 时确认，补偿 CAS 与 baseline/no-op 对账也先取得同一精确锁，再同时确认 OID 和 identity；外部改成的 symbolic ref 或其他 direct OID 必须原样保留。取得 canonical index lock 并复制准入 index，同时按 baseline/candidate 两棵 tree 的精确 entry/type 构造不重叠的 publication roots，在 manager-owned journal 内物化 candidate tree，并记录 live baseline 与 candidate 的类型、字节、symlink target、权限和目录树摘要。每个 root 的完整祖先链在 mutation 前逐级以 no-follow directory capability 固定并保持到结算：Unix rename 只相对已打开目录 fd 执行；Windows 保留精确祖先 handle，并以不允许 delete-share 的 source entry handle 执行 rename，拒绝 symlink、junction 和其他 reparse point。任何后续 pathname 只用于重新证明可见链仍指向同一目录对象，不能授权写入替换后的链接目标。主 worktree 不使用会覆盖 ignored 文件的 `read-tree -u`：每个 live root 先原子移入 quarantine 并验证就是记录的 baseline 对象，再以 OS no-replace rename 安装 candidate；扫描后出现的 ignored/untracked 路径会被一起隔离或使安装失败，不能被覆盖。补偿同样先把当前 live 目录项原子移入另一 quarantine，再对隔离对象验证 candidate ownership；验证只决定能否恢复 baseline，不能作为自动删除该 candidate inode/tree 的依据。每个 root 显式记录 `untouched`、`quarantined`、`applied`，部分 apply 失败时不得把尚未触碰的 baseline root 误报成外部冲突。若外部 writer 在验证前替换 file/symlink 为文件或目录，原对象会恢复到 live path，baseline 则保留在 journal，并返回 `RUN_RECOVERY_FAILED`，绝不依据旧 bool 删除 live path。POSIX rename 不能撤销已打开 fd 对 inode 的写权限，因此协议在每个 Git 发布边界前重新摘要 original quarantine；已发生的开放句柄写入会拒绝并恢复。摘要后仍可能通过旧句柄写入 baseline original 或 rollback candidate quarantine，所以两类精确 inode/tree 都必须保留为 durable recovery material，不能自动递归删除；后续显式清理只有在证明旧执行和外部句柄均已结算后才能进行。协议在 index lock 前、碰撞扫描后、worktree publication 后、ref commit confirmation 前、ref publish 后、补偿隔离并验证 candidate 后、baseline ref lock 前及 index publish 前设置明确检查点，并反复校验 HEAD、branch、canonical index 原始字节、candidate index tree、tracked worktree、untracked files 和 original quarantine。ref commit 命令一经尝试即视为结果不确定；先完全结算 transaction 进程，再对原始精确 target ref 做上述锁内对账：仍为 candidate 且 identity 未变时以旧 OID CAS 撤回，已为 baseline 且 identity 未变时无需处理，其他 OID/identity 则保留外部更新并返回 `RUN_RECOVERY_FAILED`。canonical index 的原子 rename 是最后一个可失败发布步骤，成功后不再执行会把已落库 Git 结果翻成失败的验证。不得自动 reset、clean、覆盖或吸收外部变化。
6. 提交失败或 Provider 失败发生在清理前时，有变化的隔离 worktree 与 Run ref 保留，错误中返回恢复位置；没有变化的 manager-owned worktree 可安全移除。若隔离 worktree 已安全移除、最终 locked transaction 又因外部变化拒绝整合，则至少保留指向完整 Run commit 的 ref。成功整合后删除临时 Run ref，commit id 仍由 assistant Message 的 Codex metadata 记录。
7. 取消先传播给 Adapter 并给子进程收尾机会。应用层的 cancel 与 Adapter 的最终 integrate 共享单一 finalization gate：Cancel 先取得 gate 时先持久化 `cancelled` 再发 token，Adapter 不得进入整合；Integrate 先取得 gate 时 Cancel 返回 `RUN_ALREADY_TERMINAL`，Run 必须继续持久化 completed 和 commit/output。集成获胜后的 terminal snapshot load/commit 对任意数量的 CAS conflict 和暂时性 store error 做有界退避重试；supervisor 在可靠落盘前持续持有 finalization control、Session/workspace lease 和取消注册，不能把 durable `running` 暴露给 Cancel 或新同项目 Run。不存在“仓库推进但 Run 为 cancelled”的第三种结果。即使调用方超时退出，supervisor 也继续完成子进程回收和终态持久化。有部分变化时沿用第 6 条的保留语义。重启遇到同一 Run 的既有 worktree/ref 时返回 `RUN_RECOVERY_FAILED`，不猜测旧执行是否结束，也不与它重叠写入。
8. 返回应用层前，把 operation 中落在隔离根下的 absolute cwd/file/image path 重写为 Project-relative path；成功后即使隔离目录已删除，桌面仍能相对主 Project 解析和跳转。

## 只读与写入准入

- Session title/model discovery 等只读调用不取得写入租约，可与写入型 Run 及其他只读调用并行；它们没有 Git 提交权限。
- 同一 Project 的写入型 Codex Run 串行。等待租约期间仍持有各自 Session 的进程内许可，避免输入或配置越过排队请求。
- 不同 canonical Git root 的写入 Run 可并行。远程文本 Provider 不取得工作区写入租约。
- Cancel 与查询不等待工作区写入租约；Cancel 可立即持久化并通知当前调用退出，租约只在调用已经不能整合主工作区后释放。

## 恢复与可见性

保留目录位于 Project Git metadata 下的 `ait/workspaces/<request-hash>`；发布补偿材料位于 `ait/integration-rollbacks/<commit-id>`，对应 Run ref 为 `refs/ait/runs/<request-hash>`。它们是 manager-owned 恢复材料，不出现在 Project status 中。失败且从未安装 candidate directory entry 时可删除 journal；一旦 rollback 已隔离 candidate，或成功发布后 baseline original 可能仍由开放句柄引用，就必须保留对应精确 inode/tree。当前切片选择明确拒绝自动复用或回收既有恢复材料；后续恢复操作必须先证明旧执行和开放句柄均已终止，再显式选择整合、导出或删除，不能隐式清理。

## 验证

- 两个同 Project Session 的写入请求以屏障同时发起，后一个在前一个终态后重新捕获 HEAD；两个提交各只包含本 Run 的文件。
- 不同 Project 的写入请求可同时进入 Adapter；独立 `LocalControlService` 无法绕过 advisory lock。
- 运行中注入未跟踪文件、Git index 变化和 HEAD 前移均拒绝整合；成员文件/index 保持原样，隔离 Run commit 由 worktree/ref 保留。
- 在初次 final validation 后分别注入同 baseline branch switch、staged、unstaged 和 untracked 变化；目标 ref 不推进、外部内容完整保留、Run ref 保留隔离提交。
- 在 worktree publication 前后、ref commit confirmation、ref publish 后和 index publish 前注入 staged、untracked、同路径外部写入与 publish failure；确认读取失败也按 attempted/unknown 重新检查精确 ref，拒绝路径撤回 Run 文件/ref、canonical index 保持准入状态，外部内容完整保留。
- 在 ref publish 后把 HEAD 切到同 baseline 的另一分支；原准入 ref 以 candidate OID CAS 回 baseline，新的 symbolic HEAD 与同路径外部内容保留。detached HEAD 后出现 symbolic HEAD 时不得以 no-deref 更新覆盖它。
- baseline ignore 下已有二进制文件时，让 Run 删除 ignore 并跟踪同路径；以及让 Run 以文件替换含 ignored 子项的目录。两种情况都在首次 worktree mutation 前拒绝，原 ref、canonical index 原始字节和 ignored 内容逐字节不变。
- 在碰撞扫描完成后、首次 worktree mutation 前，分别在 candidate-created 路径和即将 D/F 替换的目录内创建二进制 ignored 文件；原子 quarantine 发现 baseline 对象不匹配并逐字节恢复，原 ref 与 canonical index 原始字节不变。
- Run 同时改变 `.gitattributes` 与受 `ident` filter 影响的同层文件，并在 ref publish 后失败；candidate 在 manager-owned tree 中物化，补偿只校验已隔离对象，不依赖删除顺序下变化的 attribute 视图。
- candidate view 固定后、补偿隔离前把 Run file 分别替换成外部文件和含二进制子项的目录；live directory entry 原子移入 quarantine 后判为失配并恢复，baseline 留作 recovery material，递归删除不作用于 live path。
- publication 前保持 baseline file/目录的开放句柄，在 quarantine 验证后通过旧句柄写入；发布边界复核必须拒绝并把新字节恢复到主 worktree。另在成功返回后通过旧句柄写入，字节必须继续存在于按 commit 定位的 durable original quarantine，不得被成功清理删除。
- publication 后先打开 candidate file/目录，触发 ref publish 后补偿；在 candidate 已移入 rollback quarantine 并验证后通过旧句柄写入。ref、canonical index 与主 worktree 恢复 baseline，外部字节必须继续存在于按 commit 定位的 durable `rollback-live` quarantine。
- candidate 只改动 `dir/file` 时，在 worktree mutation 前和补偿开始前分别把 `dir` 换成指向 Project 外目录的 Unix symlink 或 Windows junction/reparse point；所有写入保持绑定原目录对象或失败关闭，Project 外 sentinel 逐字节不变。
- 多个不重叠 publication root 中，较早的新增路径在扫描后发生碰撞、较晚的 baseline-present root 尚未 apply；后者保持 `untouched`，完整补偿返回原始 dirty 冲突并清空 journal，不升级为伪 recovery failure。
- ref publish 后把原 direct target 改为解析到 candidate OID 的 symbolic ref；补偿在精确 ref lock 内发现 identity 变化，保留 symbolic ref 与其 referent，返回 `RUN_RECOVERY_FAILED`，不以 direct baseline 覆盖它。
- ref publish 后先把原 target 恢复到 baseline，再在 baseline/no-op 分支取得精确锁前推进到另一 direct OID；prepared verify 拒绝，外部 OID 保留并返回 `RUN_RECOVERY_FAILED`。
- file→directory 与 directory→file 两个方向都在 ref publish 后注入失败；两棵 tree 的 entry/type journal 完整恢复原 ref、index 和 worktree，且不遗留补偿目录。
- 主动 abort/drop 外层 execute future 后，同 Project Run 仍不能越过 supervisor 持有的租约；首个 Run 与 Session 最终结算后第二个 Run 才进入。
- 在 finalization gate 两侧确定性暂停：Cancel 获胜时不进入整合；Integrate 获胜时 Cancel 被拒绝且 assistant/commit metadata 完整持久化。
- 集成获胜后连续注入超过四次 CAS conflict 和 terminal store error；Run 保持 `running` 且 Cancel 被拒绝，同 Project 新 Run 不能进入，存储恢复后 completed、assistant 与 commit audit 一次性持久化。
- 配置成功但写脏及非零退出的 `post-checkout` hook，均验证受控 setup 不执行 hook、没有残留 worktree/ref；absolute cwd/file/image paths 被重写；已初始化 submodule 得到明确拒绝。
- 强制 partial-worktree cleanup 失败时验证错误包含保留路径/ref，且不会删除唯一恢复 ref。
- Codex 的既有成功、dirty-at-start、输出时间线及取消测试继续使用离线 fake，不依赖付费模型调用。
