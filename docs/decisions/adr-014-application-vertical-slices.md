# ADR-014：application 按业务能力组织记录与事务上下文

- 状态：Accepted
- 日期：2026-09-19
- 修订：NEC-250 的实现位置约束、NEC-251 的机械模块布局
- 保持：NEC-150 v4、NEC-224 record storage、NEC-252 typed transaction 语义

## 背景

NEC-251 将原有巨型 `control.rs` 机械拆成 `model`、`state`、`runs`、`project` 等模块，随后
NEC-250 和 NEC-252 固定了领域状态权威与 typed record transaction。虽然运行边界已经正确，
源码仍按技术类别横向集中：一个业务能力的 record、context、read plan 和 reducer 分散在多个目录；
`LocalControlService` 的实现也容易被误读为单体 application model。

## 决策

1. application record 与 typed context 由所属业务模块拥有：
   `catalog`、`project`、`conversation`、`cron`、`runs` 和 `settings` 分别保存自己的
   `record.rs`、`context.rs`。跨能力用例显式导入所需类型，不再通过中央 `control/model` 或
   `control/state` 命名空间间接访问。
2. 持久化通用机制集中在 `control/persistence`，仅包含 record codec、兼容 hydration、
   `RecordAccess`、typed diff 与 revision CAS transaction，以及生成 context capability 实现的
   内部宏。它不拥有命令路由、业务 reducer 或具体用例 context。
3. `control/use_cases` 保存公开 Command 到 typed transaction 的穷尽路由和有界读取计划。
   该层决定一个用例需要哪些 record；`ControlStore` adapter 仍只负责 filter 的物理查询、
   revision CAS、append-only 与 event outbox 原子提交。
4. application 的持久化形状统一使用 `Record` 后缀。关键状态转换继续委托给
   `ait_domain`：Project 默认值使用 `ProjectDefaults`，Session 指针使用 `SessionReference`，
   API Run 执行使用 `domain::Run`。不得为兼容字段建立第二份可独立修改的状态。
5. API Run 集成按角色拆成执行入口、provider agent adapter、durable `RunStore` bridge 和
   terminal repair。worker fencing、Message 投影和 CAS 仍由同一个 store bridge 保证；拆分不改变
   `RunCoordinator`、worker 协议或恢复语义。
6. `LocalControlService` 保留为 transport 共用 facade 和 composition root。新增行为应优先进入所属
   feature 模块，不得重新建立中央 aggregate/state 文件；只有跨 feature 的穷尽 Command 路由可以
   留在 `use_cases`。

## 依赖方向

```text
LocalControlService facade
  -> feature use case (catalog/project/conversation/cron/runs/settings)
       -> ait_domain rules
       -> application persistence transaction
            -> ait_ports::ControlStore
                 -> storage adapter
```

storage adapter 不依赖 application feature 类型，domain 不依赖 repository、async runtime 或
adapter。record codec 是所有 storage adapter 共享的逻辑持久化边界，因此属于 application，而不是
SQLite adapter。

## 兼容性

本决策只改变 application 私有源码组织和私有类型命名。公开 HTTP/CLI/Desktop contracts、
`LocalControlService` API、`ControlStore` port、durable JSON、数据库 schema、事件形状、Run 状态机、
权限和恢复行为均保持不变。

## 验证

- application 单元和集成测试继续覆盖 record codec、旧记录恢复、CAS 冲突、Session 指针、
  API tool loop、worker fencing、审批和 Workspace finalization。
- 交付执行 workspace format、build、clippy、tests 与 `cargo llvm-cov --workspace --html`。
