# 概念与架构文档

- [ADR-019：桌面启动失败时删除旧本地数据库](decisions/adr-019-desktop-startup-database-reset.md)（Accepted）：旧 catalog 阻断启动时提供确认删除与空库重启入口，不备份、不迁移、不递归删除项目数据库。
  [实现与验证报告](reports/desktop-startup-database-reset.md)记录删除范围和恢复流程验证。

- [ADR-018：Project 独立恢复与运行期独占接管](decisions/adr-018-portable-project-runtime-ownership.md)（Accepted）：format-3 以运行锁和接管代次替代永久 coordinator 归属，项目事务/版本/事件自足，全局索引可重建；旧格式通过显式离线转换进入新协议。
  [实现与验证报告](reports/adr-018-portable-project-runtime-ownership.md)记录接管、绑定、恢复与平台限制。

- [ADR-017：Codex 统一原生 Thread 与 Worker](decisions/adr-017-unified-native-codex-worker.md)：所有 Codex 请求通过 ait-worker，删除每 Run worktree 与历史 prompt 包装；旧 Ait 会话一次性清理，自动 Git 提交成为独立 Run 收尾。
  [实现与验证报告](reports/adr-017-unified-native-execution.md)记录当前能力、回归范围与覆盖率；下列历史 ADR 中冲突的 Codex 执行条款以 ADR-017 为准。
  [项目菜单导入验证](reports/codex-project-import.md)：显式发现匹配 Thread、选择导入或同步、服务端项目归属筛选与桌面异步隔离。
  [重复 Thread ID 修复](reports/codex-thread-list-deduplication.md)：兼容分页重叠与归档移动，保留游标循环校验。
  [未绑定会话误判修复](reports/codex-import-optional-bindings.md)：对齐 Rust 可选字段序列化，补充真实 HTTP 与 Desktop 导入/同步集成回归。
  [空元数据卡片修复](reports/codex-empty-text-metadata.md)：隐藏导入文本的空附加信息，保留原消息数据。
  [过程折叠展示](reports/codex-activity-display.md)：连续命令分组，组内命令独立单行折叠且图标对齐，空推理仅显示静态 Reasoning 标签，保留详情、状态和展开位置。
  [Turn 耗时核对](reports/codex-turn-timing.md)：确认原生耗时字段与真实返回，定位 Ait 尚未传递到界面的时间信息。

## 工程规范

- [Rust style guide](policy/rust.md)：所有 Rust 修改必须遵循的代码规范，包括测试模块拆分与项目报告中的测试覆盖率要求。

## 当前基线

- `decisions/adr-016-codex-history-import.md`（Proposed）：以 Codex app-server 历史为权威，将
  Thread 投影为 Ait Session，完整终态 Turn 按 userMessage 边界原子发布为普通 Message 链。
  基于 0.153.4 schema 与隔离实测，定义稳定分页、writer 接管/释放、输入结果不明对账、同 Run
  steer、Project 内 fork 来源与共享、全局绑定预留和 NativeCwd 互操作边界；待 Accepted 后生效。
  2026-09-19 实现复核已补 writer/config 准入、durable input、统一终态投影、CAS 重读、冷尾部确认
  和桌面 ProviderItem；具体支持范围及未完成分期见该 ADR 的“实现复核”。
  [实现修正与验证报告](reports/adr-016-implementation-fixes.md)记录回归测试及覆盖率比较。

- `decisions/adr-015-workspace-capability.md`：Project Workspace 文件/Git/lease 能力独立为
  `ait-workspace`，本机实现为 `ait-workspace-local`；Run execution 的 Workspace 协议仍留在
  `ait-ports`，并删除无生产消费者的旧同步 ProjectEnvironment 边界。

- `decisions/adr-014-application-vertical-slices.md`：application record 与 typed context 按
  Catalog、Project、Conversation、Cron、Run、Settings 业务能力归属；通用 persistence 仅保留
  codec/access/transaction，命令路由与读取计划显式归入 use-case 层。

- `decisions/NEC-296/adr-001-new-session-draft.md`：桌面新建 Session 先进入初始 system Message 的本地派生草稿，首条输入原子接纳后才创建 Session；接纳回执独立于视图读取，结果不明时以稳定 ID 恢复。

- `decisions/NEC-313/adr-001-aligned-api-agent-tools.md`：API Agent 对齐 Ait/OpenCode 的工具命名，并实现 Web、Project-local Skill、Todo、持久化用户交互及有界前台 Subagent；同时固定当前不支持的后台与跨模型子任务边界。

- `decisions/NEC-310/adr-001-minimax-api-provider.md`：MiniMax OpenAI-compatible Chat Completions、模型发现、全入口 Provider kind 与 worker 工具循环。

- `decisions/NEC-309/adr-001-gemini-api-provider.md`：Gemini 原生 GenerateContent / 模型发现适配、全入口 Provider kind、worker 工具循环与无 provider call ID 的关联规则。

- `decisions/NEC-304/adr-001-cron-sessions-and-desktop.md`：Cron 的 Desktop 配置入口，以及每个 occurrence 原子创建独立 Session 与 Run 的当前语义；修订 NEC-150 与 ADR-013 的 Sessionless Cron 条款。

- `decisions/NEC-303/adr-001-cli-config-command.md`：CLI 设置入口由 `settings get|set|reset`
  改为 `config get|set|reset`，不保留旧命令别名；application、HTTP 与持久化语义不变。

- `decisions/NEC-301/adr-001-global-default-and-small-agents.md`：全局 Default Agent 回退、Small Agent 短调用选择，以及暂不参与 prompt 组装的 Agent system prompt 保留字段。

- `decisions/NEC-294/adr-001-project-editing-and-sidebar.md`：Project 名称与默认 Agent 原子编辑、独立展开状态及按 Project 读取的 Session 导航摘要；修订 NEC-233 的侧栏展示限制。

- `decisions/NEC-290/adr-001-api-tool-approval-grants.md`：API Provider 的交互升级、固定 Run 基线与一次性 grant、持久化/worker fencing、Session 和 Cron 审批入口；使用与离线 GUI 演示见 `operations/api-tool-approvals.md`。

- `decisions/NEC-269/adr-001-composer-permission-default.md`：Prompt 移除无功能加号、权限选择器置首，新建/重置设置默认 Workspace Write，保留已存权限与 Run 快照。

- `decisions/NEC-263/adr-001-shell-and-prompt-permissions.md`：Prompt 三级权限选择、API Shell 的系统隔离与能力过滤、流式仓库浏览/计数及默认折叠工具结果。

- `decisions/NEC-250/adr-001-application-domain-state.md`：application 领域状态权威、边界投影、统一 Run lifecycle 与旧 Service 收敛。

- `decisions/NEC-253/adr-001-project-workspace-port.md`：控制面 Git/文件系统/lease 的异步 port、本地阻塞预算、取消与授权事实边界。 各 public 调用共享 deadline，queued future drop 立即释放资源，创建失败报告 retained path/state。
- `decisions/NEC-252/adr-001-typed-control-transactions.md`：命令专属 typed context、显式读取计划、按 typed record 生成变更的 transaction、Project 规范化查重与 CAS 前 Git 复核，以及 API Run bridge 的直接存储边界。

- NEC-248 将生产 Run 接入受监督的独立 worker；协议、事务 receipt、恢复与平台限制见 `operations/worker-processes.md` 和 NEC-169 ADR。

- `decisions/NEC-257/adr-001-cli-command-and-address-simplification.md`：删除顶层 `events`、将 Provider 操作移到 `agent provider`，并以全局 `--host` / `--port` 固定 HTTP 连接 daemon。

- `decisions/NEC-235/adr-001-global-and-project-storage.md`：全局 `ait.sqlite3` catalog 与每 Project `.ait/project.sqlite3` 的物理拆分、实际运行路径、可恢复提交、旧库迁移和独立备份。

- `decisions/adr-013-session-worktrees.md`：每个 Session 使用固定的
  `<Project>/.ait/<session-id>` linked worktree；Session-bound Run、工具、标题生成和恢复均以该目录为工作目录，Project 主检出保持干净。

- `decisions/NEC-247/adr-001-api-provider-host-tool-loop.md`：公共 API Provider 复用 RunCoordinator 的持久化工具循环、固定权限与能力过滤；包括文件 worker drain、持久化取消与错误/panic 原子结算，WF-13 默认离线验收。

- `decisions/NEC-192/adr-001-permission-integration.md`：三级权限跨入口集成核验、command/file
  审批上限、隔离授权路径往返、恢复时管理员上限重检与权限错误脱敏。

- `decisions/NEC-195/adr-001-name-only-project-creation.md`：省略工作目录时在当前用户 Documents
  独占创建同名目录；沿用 Git/原子注册流程，明确冲突拒绝、CAS 重试与失败目录保留语义。

- `decisions/NEC-241/adr-001-entity-cli.md`：类型化实体 CLI、专用凭据 stdin、完整 Command 映射覆盖和权限设置迁移。

- `decisions/NEC-234/adr-001-api-provider-run-permissions.md`：OpenAI、DeepSeek 等普通 API
  Provider 与 Codex 共用三级 sandbox Run 快照及管理员上限；NEC-247 工具执行器沿用该上限。

- `decisions/NEC-233/adr-001-project-scoped-desktop-data.md`：Desktop 读取拆分为 Project catalog、
  全局 Agent/Provider catalog 与显式 `project_id` 的单 Project 投影；删除 Electron `workspace.view`，
  并让 Session/Message/Run/progress 与恢复提示全链路保持 Project 范围。

- `decisions/NEC-227/adr-001-development-mock-provider.md`：以非默认编译 feature 隔离开发专用 Mock Provider，并复用真实 Run/Message 持久化状态机。

- `decisions/NEC-208/adr-001-codex-run-permissions-and-native-approvals.md`：桌面权限设置到
  Codex Run 参数的不可变快照、fail-closed 管理员上限，以及可重连的原生审批端口与 UI。

- `decisions/NEC-226/adr-001-atomic-session-derivation.md`：桌面派生意图由 daemon 在 Session
  租约与同一 CAS 快照内决定复用或分叉，消除 renderer 叶子快照与接纳之间的竞态。

- `decisions/NEC-218/adr-001-deepseek-reasoning-efforts.md`：DeepSeek adapter-owned
  `off / low / high / max` 能力目录、发现合并优先级与桌面对话框选择器。

- `decisions/NEC-224/adr-001-record-oriented-control-storage.md`：控制面按实体记录和 Project 范围读取，
  SQLite 具名表迁移，以及公开 Workspace Snapshot 命令/API/IPC 的移除；物理双层数据库继续遵循 NEC-146。

- `decisions/NEC-212/adr-001-workspace-run-recovery-and-git-settlement.md`：在 NEC-209 隔离
  worktree/补偿发布协议之上保存稳定 operation/lease 与完整结果 checkpoint，并在 daemon readiness
  后安全续跑 queued Run 或对账已 checkpoint 的 Git 结果。

- `decisions/NEC-205/adr-001-live-run-progress-and-recovery.md`：daemon 异步 Run、Codex 统一进度事件、
  有界 checkpoint、cursor 回放后持续监听，以及桌面端增量展示与断线状态同步。

- `decisions/NEC-209/adr-001-codex-workspace-isolation.md`：以规范化工作区写入租约和每 Run 隔离 worktree 保证 Codex 并发准入、Git 提交归属及冲突保留。

- `decisions/NEC-204/adr-001-codex-phased-output-timeline.md`：按 Codex item 聚合并持久化阶段化输出时间线，桌面端以折叠过程和独立最终答复收尾。

- `decisions/NEC-203/adr-001-remove-test-provider-identities.md`：移除 Tool、Manual、ProviderFailure 与 ApprovalRequired 的生产 Provider 身份，保留 seam 测试与旧快照引用保护。

- `decisions/NEC-201/adr-001-codex-provider-model-discovery.md`：Codex Provider 通过 app-server `model/list` 动态发现 picker 可见模型及逐模型推理等级。

- `decisions/NEC-198/adr-001-codex-operation-and-message-rendering.md`：Codex 原生操作的有界展示投影、安全 Markdown 表格与受 Project 根约束的文件/行号跳转。

- `decisions/NEC-196/adr-001-remove-retired-provider.md`：退役内置 Provider 的删除范围、未引用目录项清理及旧数据兼容边界。

- `decisions/adr-010-provider-discovery-and-agents-page.md`：Provider 两段式连接与模型选择、无持久化副作用的模型发现接口，以及独立 Agents 管理页面。

- `decisions/adr-009-session-exclusion-and-agent-providers.md`：Session 独占准入、AgentProvider 共享连接、命名/匿名 Agent 配置及 reasoning effort 的当前修订；相关条款优先于旧设计。

- `decisions/NEC-150/adr-001-core-domain-model-v4.md`：核心领域模型，Accepted，是术语与聚合边界的权威版本。
- `decisions/NEC-161/adr-003-session-agent-binding-and-identities.md`：实现评审修订；Message 使用 UUID，Project description 默认空字符串，Session 持有可在空闲时显式重绑的 Agent。
- `decisions/NEC-162/adr-004-electron-desktop-boundary.md`：Electron 桌面端接入、Rust API/设置单一语义来源，以及从任意 Message 原子创建分支的边界。
- `glossary.md`：从 ADR-001 v4 提炼的快速术语表。

## 配套设计

- `refactors/NEC-251-control-modules.md`：Control facade、use-case/record 模块和七个测试套件的机械迁移对应及兼容性边界。

- `decisions/adr-012-codex-native-tool-set.md`：仅 Codex Provider 使用 core 原生工具、app-server 分层提示词，以及两轮 Python Hello World 真机验收。

- `decisions/adr-011-default-api-tool-set.md`：默认 System Prompt、参考 DeepSeek Harness 的工具定义、模型覆盖与 API 请求组装；工具执行仍归宿主。

- `decisions/NEC-148/adr-001-reliability-portability-baseline.md`：结构化可观测性、无凭证归档、SQLite 备份与性能基线。
- `decisions/NEC-152/local-api-cli-vertical-slice.md`：本地 HTTP/CLI 纵向切片、SSE cursor 重连、SQLite 恢复与可执行验收说明。
- `decisions/NEC-147/adr-001-message-session-store-boundaries.md`：MessageStore 初始化与 append-only 边界、独立 SessionStore，以及 append 后 CAS 的失败保留语义。
- `decisions/NEC-149/adr-001-project-path-and-instruction-snapshots.md`：Project 路径授权、指令优先级/revision 与新 Session 根快照事务边界。
- `decisions/NEC-146/adr-002-split-sqlite-poc-v3.md`：系统目录与 Project `.ait` 双层 SQLite 设计及 SQL PoC。
- `decisions/NEC-151/ADR-002-agent-provider-contract.md`：Agent 配置与 Provider Adapter 契约。
- `decisions/NEC-151/ADR-003-agent-adapters-codex.md`：Agent Adapter crate 与 Codex 集成边界。
- `decisions/NEC-154/adr-002-rust-workspace-runtime-architecture.md`：Rust workspace 与运行时架构，Accepted 实现基线。
- `decisions/NEC-166/entity-operation-http-api.md`：按实体/操作拆分的本地 HTTP API 路由，替代统一 command 入口。
- `decisions/NEC-169/adr-001-ait-worker-contract.md`：`ait-worker` 功能边界、daemon 私有协议、恢复语义与分阶段实现计划。
- `decisions/NEC-174/adr-001-codex-session-execution.md`：Codex Session 输入、assistant result 与 Git commit 的首个可执行闭环。
- `decisions/NEC-174/adr-002-codex-run-reasoning-effort.md`：历史 Run 覆盖值设计，已由 ADR-009 的 Agent 配置取代。
- `decisions/NEC-176/adr-005-session-naming-and-generated-metadata.md`：Session 手工命名、首次交互临时标题与只读 AI 检索元数据生成。
- `decisions/adr-006-project-git-provenance.md`：Project 初始 Git 基线、可选远端仓库地址，以及 human user Message 的干净 HEAD 快照约束。
- `decisions/adr-007-rig-llm-client.md`：agent-adapters 内 Rig LLMClient 的配置、模型查询、单次调用和凭证/SDK 类型边界。
- `decisions/adr-008-control-command-execution.md`：Control 命令内部完成 Run 执行，结果 DTO 与执行指令分离，以及查询/Cron 重放的无执行副作用边界。

## 运维手册

- `operations/worker-processes.md`：生产 daemon/worker 拓扑、ACK 与 fencing、恢复、进程树回收、权限/资源上限及凭证边界。

- `operations/releasing.md`：Ait desktop 版本准备、双平台 GitHub Release、产物校验与失败恢复。
- `operations/reliability-security-observability.md`：数据保留、附件 mark-and-sweep、数据库备份/恢复、可靠性测试矩阵与性能基线。
- [CLI 用户流程](../workflows/README.md)：逐个用户目标的可执行步骤、可观察结果、失败恢复、当前差距与 CLI 集成测试映射。

配套设计仍保留各自原始评审状态；实现前若与 ADR-001 v4 冲突，以 v4 及明确列出的 Accepted 修订为准。同号 ADR 来自不同设计 issue，因此目录包含 issue 编号以避免歧义。

## 历史归档

`archive/domain-model/` 保存 NEC-144 初稿及 ADR-001 v1-v3，只用于追溯，不应作为新实现依据。
