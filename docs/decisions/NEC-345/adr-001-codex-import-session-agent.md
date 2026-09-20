# ADR-001：Codex Thread 导入时保留 Session Agent 配置

- 状态：Accepted
- 日期：2026-09-20
- 来源：NEC-345
- 修订：ADR-016 的导入 Agent 配置种子与 Provider catalog 规则

## 背景

Codex Thread 同步已经从 `thread/read` 保存 `model` 和 `reasoningEffort` metadata，但创建 Ait
Session 时只绑定界面选中的命名 Agent。结果是导入会话可能改用另一个模型或推理等级；当原生模型
没有在 Codex Provider 中启用时，后续也无法把该状态表达为合法的 Ait Agent 配置。

## 决策

`SyncCodexThread` 按 ADR-018 的权威存储边界执行可恢复的 catalog → Project 两阶段对账：

1. 用户选择的 enabled Codex Agent 为新 Session 提供 `provider_id`、system prompt 和 metadata
   缺失时的配置回退。已绑定 Session 以当前 Agent 为回退，不因重复请求换绑到另一个命名 Agent。
2. 非空 `thread.model` 覆盖回退模型；`thread.reasoningEffort` 的非空字符串或显式 null 分别表示
   指定等级或 Provider 默认值。模型改变但 effort 缺失时使用 Provider 默认值；metadata 全部缺失时
   保留回退配置。
3. 原生配置与命名 Agent 不同时，创建稳定、Session-owned Agent；后续同步更新同一 Agent 并增加
   revision。配置不变的重复同步不创建重复 Agent，也不增加 revision。
4. Provider catalog 缺少该模型时追加 `id = name = thread.model` 的模型条目；若模型已存在但缺少
   导入的 reasoning effort，则补充该等级。该单调追加在全局 catalog 事务中先提交；提交成功后
   丢弃旧 Project 读取并重读同一版本上下文，不把全局 Provider 与项目记录混入一个事务。
5. Session 私有 Agent、Session、Message 与 `codex.history.synced` event 随后在单个 Project 事务
   中提交。若第二阶段失败，已补录的 catalog 能力保留，重试幂等复用；Provider event 只随实际
   catalog 变化发布，history event 只随成功的 Project 提交发布。CAS 冲突均重新读取后计算。

原生 Thread metadata 是导入配置种子，不是运行时真实性证明。继续该 Session 时，ADR-016/017
规定的 writer 准入和 `thread/resume` 实际配置校验仍然生效。此流程不导入凭证、认证状态或
Codex 全局设置。

## 失败与兼容性

- model 为空、null 或缺失时不创建伪模型，继续使用回退 Agent 的模型。
- catalog 补录成功但 Project 导入失败时不会回滚全局模型；这是安全的单调能力扩展，后续重试
  复用它且不重复 Provider event，不存在跨 SQLite 文件的原子提交承诺。
- 现有 Session Agent 不可用、Provider 类型不为 Codex、Project 归属冲突或确定性 Agent ID 已被
  其他 Session 占用时，同步整体失败且不发布部分状态。
- wire command 不新增字段；`agent_id` 继续必填，但语义是新导入 Session 的回退策略 Agent。
- Desktop 仍可展示用户选择的“Agent for new sessions”；导入完成后的 Session 视图返回最终
  Session-owned Agent ID。

## 验证

离线 application 集成测试使用生产 `PortableSqliteControlStore` 覆盖缺失模型/effort 的 catalog
补录、失败后重试与最终事件一致性、原生 model/reasoning 的 Session Agent 投影，以及相同或另一
有效回退 preset 下重复同步的私有 Agent 幂等性；并保留既有 Codex 历史、并发 CAS、外部 writer
与输入恢复回归。
