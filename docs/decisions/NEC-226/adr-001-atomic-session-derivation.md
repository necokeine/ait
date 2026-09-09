# ADR-001：Session 派生的原子接纳

- 状态：Accepted
- 日期：2026-09-09
- 依赖：NEC-150 核心领域模型 v4、ADR-009、NEC-205、NEC-224
- 来源：NEC-226

## 问题

桌面端允许用户从任意 Message 继续。此前 renderer 依据本地快照判断选中 Message 是否为当前
Session 的叶子：是则提交 `SendMessage`，否则提交 `ForkSession`。该判断与 daemon 的 Session
独占接纳分属两个时刻；若 daemon 接纳前 Session head 已前移，`SendMessage` 会静默把输入追加到
最新 head，而不是用户选择的派生源。

## 决策

1. 新增意图型 `DeriveSession` command。请求同时携带当前 `source_session_id`、原始
   `at_message_id`、候选新 Session id、Project、Agent 和文本。Electron main 只提交该意图，
   renderer 不再决定复用还是分叉。
2. application 接纳命令时始终锁定候选新 Session id，并在同一个租约注册表临界区尝试锁定 source
   Session。source 已被占用不会作为 `SESSION_BUSY` 返回，而是成为必须分叉的接纳事实。
3. 在持有接纳租约和 Project workspace 租约时，从同一个 revision 读取 source Session、选中
   Message 的直接子节点、候选 Agent/Provider 与 Project。CAS 提交中仅当 source 租约成功、
   Session 无 active Run、current head 仍等于 `at_message_id`、该 Message 仍无子节点且 Agent
   未改变时复用 source Session；任一条件不满足均从原 `at_message_id` 创建新 Session。
4. CAS 冲突重试重新读取上述记录并重新判断，因此另一个 daemon 实例在接纳期间推进 head 或添加
   child 时也只能导致分叉，不会把输入追加到新 head。Message 仍保持不可变，Session 仍只是可移动
   current pointer。
5. Project、Agent、source Session、Message、文本和候选 Session id 的普通校验先于复用/分叉
   选择，错误原样返回；只有并发/资格条件触发分叉，不以分叉掩盖配置、归属、Git 或输入错误。
6. workspace 准入保守检查复用 Agent 与分叉 Agent 两个候选配置；只要任一候选会写 Git，就在读取
   baseline 和提交 user Message 前取得同一 Project workspace 租约。

## 结果

- 用户选择的 `at_message_id` 成为 daemon 端权威派生源；本地 UI 快照只负责展示。
- 空闲且未变化的当前叶子继续复用 Session，不额外制造分支。
- source 忙、head 前移、已出现 child 或 Agent 已改变时创建新 Session，父链固定连接到原始 source。
- `ForkSession` 保留为调用方明确要求无条件分叉的独立操作。

## 验证

- application 集成测试覆盖空闲未变化时复用、source 正在执行时分叉、接纳前 head 前移后从原 source
  分叉并校验父链，以及无效 Agent 错误不被 fallback 隐藏。
- SQLite 测试覆盖 `MessageChildren` 有界选择器；HTTP 测试覆盖同步与异步派生路由。
