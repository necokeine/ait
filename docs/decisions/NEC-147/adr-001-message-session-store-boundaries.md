# ADR-001：Message 与 Session Store 接口边界

- 状态：Accepted
- 日期：2026-09-04
- 来源：NEC-147 实现评审
- 范围：独立 Message/Session 领域服务；不替代 Run 持久化事务

## 决策

1. `MessageStore` 是已初始化 Message forest 的常态读写接口。具体 store 在初始化流程中持久化根 System Message；常态接口不暴露 `insert_root`。
2. `MessageStore` 只负责不可变 Message 的查询与 append。Session ref 的创建、查询和 version compare-and-swap 属于独立 `SessionStore`。
3. Session 输入由 application service 顺序协调：先向 `MessageStore` append 直接子 Message，再用新 Message ID 调用 `SessionStore.advance_head`。
4. Session CAS 冲突或更新失败不回滚、不删除已 append Message。错误返回保留的 Message ID，调用方可把它展示为兄弟分支或据此恢复。
5. Session 历史切换仍通过在目标 Message 上创建新 Session 完成，不复制或改写 Message。

## 一致性边界

拆分后，普通 `MessageService` 不再要求两个 store 共享事务：Message append 是第一个 durable fact，Session head 是可重试的引用更新。这符合“冲突不丢消息”的要求，也避免把 Session 生命周期塞进 Message persistence port。

Run 协调器仍使用专用 `RunStore` 事务接口。Run 输出需要同时推进 Run head、连续 `run_seq`、可选 follow Session 与 outbox，这些不变量继续由 `RunStore.append_message` / `append_tool_result` 原子提交；本 ADR 不拆散该事务边界。

## 结果

- Message adapter 可以独立于 Session 实现和测试。
- Session adapter 只处理 ref、状态与乐观并发。
- CAS 失败会产生可见兄弟分支，而不是隐式丢弃输入。
- store 初始化流程必须验证唯一根节点是合法 System Message；初始化失败时不得暴露半初始化的 `MessageStore`。
