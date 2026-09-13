## ADR-001：命令上下文与 record transaction

- 状态：Accepted
- 日期：2026-09-13
- 来源：NEC-252
- 继承：NEC-224 record-oriented storage、NEC-150 ADR-001 v4

## 决策

命令使用独立的 typed context。共享 reducer 仅声明所需的记录能力；没有包含全部记录族的共同写模型。读取计划集中于 application 的 read-plan 模块，先读取引用锚点，再读取直接依赖；每一阶段必须观测同一 revision，否则重建计划。普通 Session 操作不扫描 Project Message 集合，只有 list/export 显式扫描对应记录族。

`RecordTransaction<C>` 保存 observed revision、实际加载的记录 identity 及 typed baseline。提交先比较 context 中的 typed 值，只对发生变化的记录编码，产生显式 Put/Delete 和 pending events。Delete 必须属于实际加载的 identity；默认 Provider/Settings 等合成值不能成为删除依据。未声明的记录族不得进入 context。codec 与兼容迁移不进入 reducer。

存储 port 继续采用 NEC-224 的 `read` / `apply`；SQL transaction 不向 application 暴露。record changes、revision CAS、Session/Run 指针和 durable events 仍在同一个存储事务提交，Message append-only 仍由存储层强制执行。

新增 `ProjectWorkdir` 索引选择器，注册/导入只查询目标 Project ID 与规范化路径冲突记录。规范化在准备阶段完成；read plan 本身不执行文件系统操作。单库与拆分存储均复用现有 Project workdir 唯一索引，不改变数据库 schema。

CAS 重试只重新读取并计算领域变更。目录、Git worktree 等准备动作在重试循环外执行；重试中重新校验引用与权限，不能借旧的准备结果接受改变后的执行上下文。Agent/tool 调用继续只在成功提交之后执行。

准备引用以 typed 值保存，避免为重试校验再次序列化整段 Message payload。引用改变时返回可重试冲突，由新请求重新准备。循环内允许只读 Git 基线复核：使用禁用可选锁的 index diff、status 与 tree 读取，不调用 `write-tree`。

API runtime bridge 持有 ControlStore 与专用 Run context，直接提交同一 record transaction，不经完整 LocalControlService 投影。公开 Command/Response/Event 和持久化 record schema 不变。

## 验证

覆盖 read-plan 的最小选择、引用 revision 变化、祖先路径、一致 archive、无关损坏记录隔离、未加载删除拒绝、重复 CAS 提交与 API Run bridge 故障；执行 workspace format、clippy、tests。
