# NEC-251：Control 机械模块化迁移对应

执行基线为 `5cb1a02`（PR #62 的 main 合并提交），包含 NEC-248 已审核 head
`b82f2f9c94bf0ca6044131c8bd0cfbc1869a9056`。原 `control.rs` 为 6,852 行，
`session_agent_config.rs` 为 3,964 行。本次只移动现有实现并调整模块引用与内部可见性。

## 生产代码

所有路径均相对 `crates/application/src/control/`。`mod.rs` 保留 service 字段、
构造器、builder、`execute` / `submit` 的 response envelope，以及穷尽的 Command 路由。
其余公开方法仍是同一个 `LocalControlService` 的 inherent methods。

| 原有职责 | 迁移位置 |
| --- | --- |
| 命令准入、同步/异步 continuation、CAS commit | `execution.rs` |
| WorkingSet、LoadedWorkingSet、序列化默认值 | `state/mod.rs` |
| 有界 record 选择、revision 检查与持久化调用 | `state/records.rs` |
| record 编解码、workdir hydration、Put/Delete diff | `state/codec.rs` |
| 原 `agents.rs` 的目录与 Agent 配置 reducer | `catalog/mod.rs` |
| 原 `agents.rs` 的凭据访问、Provider 保存与发现 | `catalog/providers.rs` |
| 原 `agents.rs` 的 legacy snapshot/catalog 迁移 | `catalog/migration.rs` |
| Session 排他准入、canonical Project 文件锁 | `admission.rs` |
| Project 注册/default Agent、archive、Git、Session worktree | `project/{mod,archive,git,worktrees}.rs` |
| Session 创建/派生/绑定、消息与 Run 创建、标题生成 | `conversation/{mod,messages,title}.rs` |
| 取消、shutdown/drain、执行、journal/receipt、finalization、终态落库、恢复 | `runs/{mod,workspace,journal,finalization,settlement,recovery}.rs` |
| 原 `api_run.rs` 和 `progress.rs` | `runs/api_run.rs`、`runs/progress.rs` |
| 原生审批生命周期/授权、Run 权限快照/管理员上限 | `approvals.rs`、`permissions.rs` |
| Cron、设置、事件投影/回放、错误转换 | `cron.rs`、`settings.rs`、`events.rs`、`errors.rs` |

33 个 Command 的原路由保持如下：

| Command | 路由 |
| --- | --- |
| `GetRun`、`ExportProject`、`GetSettings`、`ListProjects`、`ListAgents`、`ListAgentProviders`、`ListSessions`、`ListMessages`、`ListRuns`、`ListCrons` | `execution::try_execute` → `state/records` → facade `read_command`；export/settings 投影调用对应模块 |
| `SaveAgentProvider`、`DiscoverProviderModels`、`RefreshProviderModels` | `execution::try_execute` → `catalog/providers` 的原凭据边界 |
| `RegisterProject`、`SetProjectDefaultAgent` | facade `apply_command` → `project` |
| `RegisterAgent`、`UpdateAgent`、`SetSessionConfig` | facade `apply_command` → `catalog` |
| `CreateSession`、`SetSessionAgent`、`RenameSession`、`SetSessionTitle`、`SendMessage`、`ForkSession`、`DeriveSession` | facade `apply_command` → `conversation` 及其子模块 |
| `CancelRun`、`ResolveNativeApproval` | facade `apply_command` → `runs` / `approvals` |
| `CreateCron`、`SetCronEnabled`、`TriggerCron` | facade `apply_command` → `cron` |
| `ImportProject` | facade `apply_command` → `project/archive` |
| `SaveSettings`、`ResetSettings` | facade `apply_command` → `settings` |

## 测试迁移

原 `session_agent_config.rs` 的 48 个具名测试及其 cfg 条件保留；其中两个 catalog 测试
按 `dev-mock-provider` / debug 配置互斥，因此一次默认运行执行其中 47 个。
路径均相对 `crates/application/tests/`。

| 新文件 | 具名测试数 | 原测试覆盖的边界 |
| --- | ---: | --- |
| `workspace_admission.rs` | 6 | Session 排他、跨 service/process 与 canonical alias 租约、不同 Project 并发、独立 worktree baseline、commit 前准入 |
| `workspace_finalization.rs` | 7 | 取消与 integration 的 gate、transport future 丢弃后的监督、terminal store/running/checkpoint 故障 |
| `session_configuration.rs` | 3 | 私有配置复用/复制、活动调用取消、无关损坏记录隔离 |
| `permission_profiles.rs` | 12 | API/Codex 权限快照、fork/derive、CAS 后重新校验、管理员上限、只读默认值与错误脱敏 |
| `native_approvals.rs` | 9 | 审批持久化/去重/取消、三类 grant、scope/path/symlink 上限、敏感值隔离 |
| `provider_catalog.rs` | 7 | Provider 类型分派、host/draft discovery、credentials、默认内置目录与 dev mock |
| `provider_migration.rs` | 4 | 退役目录、引用保护、legacy snapshot 与 v2 archive |

`fixtures/` 仅共享原有 command setup、信号等待、被动 Agent/Gateway fake 和可暂停的
store wrapper。每个行为断言仍留在所属测试文件。已有 `support.rs` 及其他测试套件保持不变。

## 兼容性与验证

- `ait_application::{LocalControlService, PermissionPolicyLimits}`、所有公开方法签名与
  `StartupRecoveryPlan` 返回值行为保持原样；HTTP、daemon、project-local、desktop 调用方无需改动。
- WorkingSet/journal 的 serde 字段、record schema、port、DTO、Run 状态机、Git 行为与
  worker/supervisor 所有权不变。Agent continuation 仍只在 CAS 成功之后执行；租约与
  finalization guard 保持原作用域；progress drain 仍先于 terminal persistence。
- 迁移核对覆盖 280 个生产函数/方法（含已有模块及其单元测试）和 113 个测试/fixture
  函数/方法；忽略格式、内部可见性和模块引用路径后，签名与函数体逐项一致。
- 验收命令：`cargo fmt --all --check`、
  `cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace`。
  PR 与 issue 记录实际执行结果。

这是 NEC-249 Stage 1 的源码导航边界；record context、I/O port、domain 模型等后续阶段
继续分别由 NEC-252、NEC-253、NEC-250 承接。
