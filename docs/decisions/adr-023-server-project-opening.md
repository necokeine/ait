# ADR-023：独立 server 的项目打开与所有权

> 已由 [ADR-029](adr-029-server-metadata.md) 废除：早期 Project 租约服务、
> `project.open/list/get/close` 及其协议和存储实现已删除。
> 下文保留历史决策，现行 Project / Workspace 统一使用 Paseo registry。

- 状态：Superseded by ADR-029。
- 日期：2026-09-22。
- 前置：[ADR-022](adr-022-independent-server.md)、[ADR-001 v4](NEC-150/adr-001-core-domain-model-v4.md)。
- 范围：只适用于新 `server`；不改变旧 daemon、旧数据或旧协议。

## 1. 本批边界

新增有生产消费者的 `server-domain`、`server-ports`、`server-application`、
`server-storage`、`server-workspace`。所有实现和 fixture 独立编写，依赖守卫继续覆盖
production、dev、build 与平台声明。domain 只依赖 UUID 和 thiserror；protocol 的 DTO
不引用 domain。端口是明确的阻塞接口，API 在受监督的 `spawn_blocking` 任务里调用 application。

本批实现 `project.open/list/get/close`，尚未引入 Agent、Session、Run、worker 或事件 outbox。
领域只定义项目创建事实、根 system Message、身份与 owner generation；后续增加 Message
形状时仍必须遵循角色、ToolUse/ToolResult 与不可变历史约束。

## 2. 准入与根快照

- 输入为绝对 UTF-8 路径，规范化后最多 4096 字节且无控制字符。路径别名指向同一项目。
- 只接受已有有效 HEAD 的 Git 根目录；不执行 init、commit、checkout 或 reset。
- `.git` 必须是本目录内的真实目录，common-dir 与其一致，且 `git worktree list` 只有一个
  checkout。首批因此保守拒绝所有 linked worktree 和带额外 worktree 的主检出。
- 拒绝自身或祖先的 `.ait` 标记以及 `.ait`、`.ait-server` 内部目录；不读取旧数据库。
- 拒绝已被 Git 跟踪的 `.ait-server`。持有路径锁后，在 `.git/info/exclude` 幂等追加
  `/.ait-server/`，并检查实际忽略结果；仓库规则覆盖本地排除时拒绝打开。
- 初次初始化把根 `AGENTS.md` 的原样 UTF-8 内容保存成 system Message；不存在时为空。
  文件不能是 symlink，最多 128 KiB。不查父目录指令、不解释内容，也不在重开时刷新历史。
  Project name 取初始目录名，base_commit 冻结初次 HEAD，created_at 使用 Unix 毫秒。

只创建 `.ait-server` 运行目录，不创建用户项目目录。失败后已经创建的 runtime 目录、锁文件、
排除规则和已提交数据库保留；不会自动删除用户文件。Git 子命令只读、无 stdin、移除继承的
`GIT_*` 环境变量、输出最多 16 KiB、每个子命令最多等待 3 秒。

## 3. 数据事务与重试

本切片的 Catalog 与 Project 使用不同 SQLite application_id 和 schema version 1；后续
[ADR-024](adr-024-server-agent-configuration.md) 将 catalog 升至 v2，Project 仍为 v1。只允许初始化真正
空的数据库，拒绝外来表、错误 family、新版本及 symlink 数据库/sidecar。没有旧数据迁移。
SQLite 使用 FULL synchronous、外键和短事务；本批不需要 WAL 或异步连接池。

打开过程：

1. 只读检查并规范化路径，在 catalog 保存 `(open, key, operation_id, canonical_path)` 意图。
2. 若有完成回执，只读返回。若当前进程已持有该路径，校验 owner 后登记此次回执。
3. 获取路径锁并建立 Git 排除，打开 Project 数据库。
4. 一个事务原子提交新 Project 与根 Message；已有项目只读取原身份和原快照。
5. 获取本机 Project ID 锁，原子持久化本机代次上限，再将更大的 owner_epoch 提交到项目库。
6. 一个 catalog 事务登记项目摘要并完成 operation，然后把存储和租约交给 application 持有。

Project 事务与 catalog 事务没有跨库原子承诺。第 4 步已提交、第 6 步未提交时，用同一个 key
重试会读回已有身份和根 Message，并完成原 operation。第 6 步提交后连接断开，重试返回同一
回执。Catalog 丢失后可用新的 catalog 登记已有 Project，Project 历史保持原样；丢失 catalog
同时意味着其 operation receipt 丢失，不能宣称跨 catalog 重建仍保留相同 operation_id。

单个 bearer 服务代表一个本机认证主体。key 以 catalog + method 为范围；业务 fingerprint
是 open 的规范路径或 close 的 Project ID，不包括 request_id、client_id、凭据值和 epoch。
不同参数复用同 key 返回 `idempotency_conflict`。首批不回收回执。

完成回执只包含 operation_id、project_id，当前状态由 get/list 查询。重启不会自动打开 catalog
中所有项目；未完成意图由显式重试恢复。重放旧 open 不会重新接管已经关闭的项目；要重新打开
需新 key。重放已完成 close 优先于 owner 检查，也不会关闭后来重新打开的项目。

## 4. 所有权和释放顺序

固定锁序：`<canonical-root>/.ait-server/project.lock` →
`HOME/.ait-server-project-locks/<project-id>.lock` → Project owner_epoch。
ID 锁目录不受 `--data-dir` 影响；同一用户的所有实例必须使用一致的 HOME。
复制项目数据库保留同一 ID，不能靠换路径或换 data-dir 绕过本机独占。锁文件不 unlink，
崩溃后由 OS 释放。本协议不是跨用户、跨机器或网络文件系统的分布式锁。

ID 锁旁的 `<project-id>.epoch` 保存本机代次上限。持锁读取本机和数据库代次，先用原子文件
替换及 fsync 预留 `max(local, database) + 1`，再条件式更新 Project owner；允许失败留下代次
间隙，不允许重复使用已发布代次。因此从旧数据库副本来回切换也不能使旧 owner 再次有效。
计数器损坏、耗尽或 symlink 都拒绝接管，不自动重置；不能把这些文件当作可清理的临时锁。

close 必须携带当前 owner_epoch；先查完成回执，再校验内存和数据库 generation，提交 close
回执，最后依次销毁数据库连接、ID 锁和路径锁。Project/Message 创建事实有拒绝 UPDATE/DELETE
的数据库 trigger；owner 保存在独立表中。后续写事务必须在同一事务内验证 owner。

每个 server 同时只接纳一个短项目任务，不建立无限排队；争用返回 `resource_exhausted`。
任务追踪 token 与数据目录锁的生命周期都延伸到阻塞任务本身，连接响应 future 被取消也不能
撤销已接纳的提交或提前释放进程所有权。draining 关闭准入并等待这些任务。正常关闭先释放
项目数据，再释放 data-dir 锁；15 秒关闭期限超时返回错误，仍在运行的阻塞任务继续持有锁，
直到完成或进程被终止。

## 5. 查询与当前限制

get/list 只读取 catalog 摘要；owner_epoch 非空表示本进程当前持有，空不表示其他进程没有持有。
list 按 Project UUID keyset 分页，每页 1–50 项，默认 20，不读取根 Message 内容或打开项目。
这些摘要可能对应离线/已移动的目录；重新打开时才做 Git/路径准入检查。

首期并存仍要求独立 clone。静态旧目录检查不能防止旧 daemon 之后注册同一路径。
本批没有 Session、任务执行、历史分页、事件订阅或通用 `operation.get`；通过原 key 重试
对应 open/close 方法查询其完成回执。

验收与覆盖率见 [M1 项目打开报告](../reports/independent-server-m1-projects.md)。
