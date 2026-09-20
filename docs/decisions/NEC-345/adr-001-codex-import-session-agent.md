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

`SyncCodexThread` 在历史投影与 Session 更新的同一条 record transaction 中完成以下对账：

1. 用户选择的 enabled Codex Agent 为新 Session 提供 `provider_id`、system prompt 和 metadata
   缺失时的配置回退。已绑定 Session 以当前 Agent 为回退，不因重复请求换绑到另一个命名 Agent。
2. 非空 `thread.model` 覆盖回退模型；`thread.reasoningEffort` 的非空字符串或显式 null 分别表示
   指定等级或 Provider 默认值。模型改变但 effort 缺失时使用 Provider 默认值；metadata 全部缺失时
   保留回退配置。
3. 原生配置与命名 Agent 不同时，创建稳定、Session-owned Agent；后续同步更新同一 Agent 并增加
   revision。配置不变的重复同步不创建重复 Agent，也不增加 revision。
4. Provider catalog 缺少该模型时追加 `id = name = thread.model` 的模型条目；若模型已存在但缺少
   导入的 reasoning effort，则补充该等级。Provider、Agent、Session、Message 和 outbox event
   原子提交，CAS 冲突后重新读取原生 Thread 与最新 catalog。

原生 Thread metadata 是导入配置种子，不是运行时真实性证明。继续该 Session 时，ADR-016/017
规定的 writer 准入和 `thread/resume` 实际配置校验仍然生效。此流程不导入凭证、认证状态或
Codex 全局设置。

## 失败与兼容性

- model 为空、null 或缺失时不创建伪模型，继续使用回退 Agent 的模型。
- 现有 Session Agent 不可用、Provider 类型不为 Codex、Project 归属冲突或确定性 Agent ID 已被
  其他 Session 占用时，同步整体失败且不发布部分状态。
- wire command 不新增字段；`agent_id` 继续必填，但语义是新导入 Session 的回退策略 Agent。
- Desktop 仍可展示用户选择的“Agent for new sessions”；导入完成后的 Session 视图返回最终
  Session-owned Agent ID。

## 验证

离线 application 集成测试覆盖缺失模型的自动补录、原生 model/reasoning 的 Session Agent 投影、
重复同步的 Agent/模型幂等性，以及既有 Codex 历史、并发 CAS、外部 writer 与输入恢复回归。
