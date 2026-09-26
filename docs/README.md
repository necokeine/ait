# 概念与架构文档

- [工作区改动整合](reports/local-workspace-consolidation.md)：PR #109 合并后，server 回归修复、
  本地 SDK、主题同步与品牌资产的提交范围及独立验证结果。

- [ADR-052：原生 Provider 能力补齐](decisions/adr-052-native-provider-capabilities.md)：
  Codex/Claude 的配置、授权、用量和原生能力协商边界；[实施清单](plans/provider-parity.md)、[能力矩阵与验证报告](reports/provider-parity.md)。

- [ADR-051：AIT 品牌识别与日间视觉系统](decisions/adr-051-ait-brand-identity.md)（Accepted）：
  飞鸟主标、跨端与多种云端 Agent 服务的品牌定位、字标、配色、留白、跨端应用和资产规则；
  含完整候选图、日间精修稿与提示词归档，生产矢量资产及客户端接入待实施。

- [ADR-050：Claude Code Provider](decisions/adr-050-claude-code-provider.md)：独立 Rust server 的
  本机 Claude Code 协议、模型发现、流式输出、审批及会话恢复；
  [当前能力报告](reports/provider-parity.md)、[初版历史报告](reports/claude-code-provider.md)。

- [Server Paseo 测试扩展](reports/server-paseo-tests.md)：逐测试上游映射、回归修复、验证结果和 workspace 覆盖率。

- [Paseo 本地 SDK library](reports/paseo-local-sdk.md)：显式本地依赖、独立构建入口、包解析验证和测试范围。

- [Apple 构建说明](operations/apple-builds.md)：桌面 DMG、iOS 模拟器、iPhone 归档与签名 IPA；[实际构建报告](reports/apple-builds.md)。

- [ADR-049：独立 App 的 Rust server 浏览器连接](decisions/adr-049-app-rust-browser-transport.md)：一次性 WebSocket 票据、显式页面来源和 `dev:app` 启动入口；[实施报告](reports/app-rust-server.md)。

- [ADR-048：Paseo workspace 与 Rust 桌面服务启动](decisions/adr-048-paseo-desktop-rust-launcher.md)：前端构建依赖、主进程凭据与子进程所有权；[启动与验证报告](reports/paseo-desktop-startup.md)。

- [ADR-047：移除 Plugin 并独立实现 Schedule / Browser](decisions/adr-047-server-schedule-browser.md)（Accepted）：两个新能力 crate，生产 175 项均安装；[实施报告](reports/server-schedule-browser.md)区分接口接通、上游差异与 Test coverage。

- [ADR-046：Codex 流式输出与运行中追加输入](decisions/adr-046-codex-streaming-and-steering.md)
  （Accepted）：持久增量游标、完整原生项去重投影、显式 `turn/steer` 与接收失败语义；
  [实施报告](reports/server-codex-streaming.md)记录剩余差异和 Test coverage。

- [ADR-045：移除 Hub、Chat 与 Loop 接口](decisions/adr-045-remove-hub-chat-loop.md)（Accepted）：按用户范围删除 19 个接口及前端映射；[报告](reports/server-removed-groups.md)记录当前 183 项范围与验证。

- [ADR-044：Paseo 前端适配 Rust transport](decisions/adr-044-paseo-client-rust-transport.md)
  （Accepted）：桌面 Bearer bridge、方法映射（现按 ADR-047 缩减为 171 项）、分连接能力协商与订阅所有权；
  [实施报告](reports/paseo-client-rust-adapter.md)记录真实 SDK 联调和 Rust 剩余缺口。

- [ADR-043：Skills 选择与文件安装](decisions/adr-043-server-skills.md)（Accepted）：五个 Skills 接口、三目标同步、删除确认与事务恢复；[实施报告](reports/server-skills.md)记录配置、上游差异和测试。

- [Paseo 客户端源码导入](reports/paseo-client-import.md)：上游 desktop 导入 `apps/paseo`，
  app 导入 `apps/app`；保留来源版本、许可证和完整性校验，记录后续构建整合范围。
  [连接实测](reports/paseo-server-connection.md)：桌面启动依赖未齐，原版协议不能直接连接 Rust server；记录真实握手与基础 RPC 对照结果。

- [Push Token 管理实施报告](reports/server-push-tokens.md)：持久租约、连接级登记与心跳续租、上游测试对应及投递限制。

- [独立 server 完整 WebSocket 接口差异](reports/server-interface-gaps.md)：固定 Paseo 完整入站基线、占位清单、已接通接口限制与本轮验证。

- [ADR-042：连接级语音、听写与双后端](decisions/adr-042-server-voice.md)
  （Accepted）：独立 server-voice 实现八个语音/听写方法、连接取消和播放确认；
  支持 OpenAI 兼容服务及本地 whisper.cpp/Piper。
  [操作说明](operations/server-voice.md)记录配置与协议，
  [实施报告](reports/server-voice.md)记录回归、覆盖率和后端差异。

- [ADR-041：Agent 原生控制与 Provider 诊断、用量](decisions/adr-041-agent-controls-provider-inspection.md)
  （Accepted）：补齐七个 Agent 和两个 Provider 方法，原生审批、模式/feature、回退恢复和子 Agent 展示。
  [实施报告](reports/server-agent-controls.md)记录行为边界、回归与覆盖率。

- [ADR-040：原生 Session 发现、导入、刷新与上下文导出](decisions/adr-040-native-session-import-refresh-context.md)
  （Accepted）：新增四个接口；provider 复用 metadata 目录服务，Timeline 原子分代并保留旧历史。
  [实施报告](reports/server-native-sessions.md)记录原生协议、回归与覆盖率。

- [ADR-039：Agent Timeline、Provider 发现与创建过程订阅](decisions/adr-039-agent-timeline-provider-creation.md)
  （Accepted）：新增十二个接口，原生历史的持久化展示投影、Provider 模型发现与缓存、
  metadata 创建回执及连接级观察者。[实施报告](reports/server-agent-timeline-provider-creation.md)记录验证与覆盖率。

- [ADR-038：server-protocol 仅依赖公共 server-model](decisions/adr-038-server-protocol-dependencies.md)
  （Accepted）：删除协议对四个能力包的依赖，迁移错误转换与目录一致性测试，收紧依赖守卫；
  包含当前九个 server crate 的完整依赖图。[实施报告](reports/server-protocol-dependencies.md)记录验证与覆盖率。

- [ADR-037：公共 Context 与具体 crate 分发](decisions/adr-037-server-model-context.md)
  （Accepted）：server-model 提供公共请求、队列与 Tokio 运行资源；各能力 crate 直接接收
  Context 和具体服务/连接状态，删除 Host 回调接口，API 负责组装与跨能力收尾。
  后台 diff 轮询使用独立有界预算，公平等待并在取消后停止投递。
  [实施报告](reports/server-model-context.md)记录回归与覆盖率。

- [ADR-036：请求先进入所属 crate 再分发到能力组](decisions/adr-036-server-crate-dispatch.md)
  （Accepted）：API 顶层仅按四个能力 crate 分流；crate 选择业务处理器，Host 端口保留 API
  调度和连接所有权，并统一普通响应与响应后的动作。
  [实施报告](reports/server-crate-dispatch.md)记录回归与覆盖率。

- [ADR-035：能力分组与安装规则归所属 server crate](decisions/adr-035-server-capability-groups.md)
  （Accepted）：metadata/filesystem/provider/terminal 自行声明方法分组并计算已安装能力，
  server-api 合并并连接处理器；名称、安装条件与消息方向保持兼容。
  [实施报告](reports/server-capability-groups.md)记录验证与覆盖率。

- [ADR-033：独立 server-terminal 与完整 Terminal 方法分组](decisions/adr-033-server-terminal.md)
  （Accepted）：10 个 Terminal 方法、真实 PTY、binary input/output/resize/snapshot/restore、连接级
  订阅和 resize 所有权；批量关闭、归档清理与 shutdown 接入。限制与覆盖率见[实施报告](reports/server-terminal.md)。

- [ADR-034：Agent 后续 turn 配置与 Session 事件/心跳](decisions/adr-034-agent-config-session-events.md)
  （Accepted）：模型/推理等级与批量配置原子保存；连接事件订阅、心跳、焦点抑制和断线释放归 metadata。
  [实施报告](reports/server-agent-session.md)记录五个新接口及验证范围。

- [ADR-032：独立 server 接通 Codex 原生文本执行](decisions/adr-032-server-native-provider-execution.md)
  （Accepted）：Provider worker 接通创建、恢复、发送、取消和等待结果；read-only Codex 首片，
  保留 native 历史边界，已实现 capability 增至 107。
  [实施报告](reports/server-native-provider-execution.md)记录并发取消、重启恢复、进程回收和覆盖率。

- [ADR-031：拆出 server-provider 并统一 Workspace 自动化与 state 入口](decisions/adr-031-server-provider.md)
  （Accepted）：14 个 Agent 方法归 provider，7 个 Workspace 方法归 metadata；
  attention 通过窄端口委托 Agent 更新，删除四个空横向 crate，domain 保持纯依赖。
  [实施报告](reports/server-provider-extraction.md)记录验证与覆盖率，
  [可行性分析](reports/server-provider-feasibility.md)保留拆分前的调研。

- [ADR-030：纵向拆出 server-filesystem](decisions/adr-030-server-filesystem.md)
  （Accepted）：集中 Git、Forge/PR、文件/目录、Worktree、恢复和 GitHub clone；skill 保留占位。
  [实施报告](reports/server-filesystem-extraction.md)记录边界、回归与本轮测试覆盖率。

- [ADR-029：统一 Paseo Project 并纵向拆出 server-metadata](decisions/adr-029-server-metadata.md)
  （Accepted）：废除独立 server 早期 Project 租约体系，将 Project/Workspace 业务协议、记录、
  用例、文件存储及 server metadata 归入独立 crate。
  [实施报告](reports/server-metadata-extraction.md)记录回归、依赖约束和测试覆盖率。

- [ADR-028：GitHub 仓库发现与独立 Project 克隆注册](decisions/adr-028-github-project-provisioning.md)
  （Accepted）：新 server 接通仓库搜索与克隆注册两个 Paseo WebSocket 方法；
  [实现报告](reports/paseo-github-project-provisioning.md)记录测试、覆盖率及与 Paseo 的差异。

- [ADR-027：独立 AgentSession 与 AgentManager 生命周期边界](decisions/adr-027-independent-agent-session-manager.md)
  （Accepted）：为新 server 建立 Provider session 创建、恢复、关闭与 durable snapshot 注册边界；
  [实现报告](reports/independent-agent-session-manager.md)记录已验证行为及尚未接入的执行接口。

- [ADR-026：规范化 Paseo WebSocket 接口并按能力分期接入](decisions/adr-026-canonical-paseo-websocket-surface.md)（Accepted）：
  登记 191 个 Paseo 入站名称并统一为 dotted method；188 个规范方法均可协商，真实实现与占位入口
  通过 `implemented_capabilities` 区分。[第一阶段报告](reports/paseo-websocket-surface-phase-1.md)
  记录首批 15 个 Project/Workspace 方法；[第二阶段报告](reports/paseo-websocket-surface-phase-2.md)
  记录 9 个 daemon/config/diagnostics/lifecycle 方法、进程内重启、测试和明确差异；
  [第三阶段报告](reports/paseo-websocket-surface-phase-3.md)记录 5 个 Workspace 标签方法、
  connection-owned 订阅、跨 catalog/workspace 的恢复事务和通用订阅释放；
  [第四阶段报告](reports/paseo-websocket-surface-phase-4.md)记录 3 个 Worktree 方法、真实 Git
  lifecycle、registry 协调与测试；[第五阶段报告](reports/paseo-websocket-surface-phase-5.md)记录
  5 个 Workspace setup/script 方法、真实子进程、信任准入及 Terminal/Proxy 明确差异；
  [第六阶段报告](reports/paseo-websocket-surface-phase-6.md)记录 9 个 Agent runtime 目录与元数据生命周期
  方法、Paseo `StoredAgentRecord`、归档级联及 Provider runtime 明确差异；
  [第七阶段报告](reports/paseo-websocket-surface-phase-7.md)记录 4 个 Workspace attention/recovery 方法、
  精确分支 worktree 恢复及事件/placement reconciliation 差异；
  [第八阶段报告](reports/paseo-websocket-surface-phase-8.md)记录 7 个 checkout status/diff/commit-history
  方法、connection-owned diff 订阅、真实 Git 行为及尚未对齐的 observer/highlight/metadata 边界；
  [第九阶段报告](reports/paseo-websocket-surface-phase-9.md)记录 13 个 branch/commit/merge/pull/push/
  discard/stash 方法、120 秒写预算、真实本地 remote 验证及 Provider/observer 差异；
  [第十阶段报告](reports/paseo-websocket-surface-phase-10.md)记录 10 个 Forge/PR/search/timeline/check 方法、
  bounded GitHub CLI adapter、本地 push 与真实 WebSocket 验证，以及多 Forge、cache/poll 与 Provider 差异；
  [第十一阶段报告](reports/paseo-websocket-surface-phase-11.md)记录 11 个文件/目录方法、revision 写入、
  文件订阅、二进制上传下载及一次性 HTTP 下载 token，并更正来源名与已发布方法的统计口径；
  [第十二阶段报告](reports/paseo-websocket-surface-phase-12.md)记录全部 188 个规范方法的占位入口、
  已实现能力标记、请求分发整理及测试；
  [层级路由报告](reports/server-websocket-hierarchical-routing.md)记录按 dotted prefix 查找的只读路由树、
  完整方法叶子的处理器归属与回归覆盖率。

- [ADR-025：先移植 Paseo Project / Workspace 模型与 registry](decisions/adr-025-paseo-registry.md)（Accepted）：
  新 server 改为以 Paseo 类型与行为为基准；先移植记录、协议 DTO 和 registry，暂停 Session 切片。
  [移植报告](reports/paseo-registry-port.md)记录原始 Zod 样本对照、registry 验收与当前接入边界。

- [ADR-024：独立 server 的 Agent 配置与不可变 revision](decisions/adr-024-server-agent-configuration.md)（Accepted）：
  新 Agent 配置、历史 revision、显式默认选择、凭据引用与 catalog v1 → v2 备份升级。
  [验证报告](reports/independent-server-m1-agents.md)记录 CAS、回执、重启、秘密隔离与覆盖率。

- [ADR-023：独立 server 的项目打开与所有权](decisions/adr-023-server-project-opening.md)（Superseded by ADR-029）：
  历史 M1 切片定义 Project Git 准入、根 Message、SQLite、catalog 回执、本机所有权
  及 `project.open/list/get/close`；该切片现已移除。
  [验证报告](reports/independent-server-m1-projects.md)记录恢复、隔离、WS 与覆盖率验证。

- [ADR-022：独立 server 与全新内部 crate](decisions/adr-022-independent-server.md)（Accepted）：
  独立 `server` binary 与旧 daemon 并存，内部依赖全部新建；定义进程隔离、数据命名空间、
  WebSocket、输入接纳与恢复边界。[实施计划](plans/independent-server.md)按服务骨架、离线闭环、
  单 Provider 接入和故障矩阵分期；M0 的服务骨架已扩展到 M1 项目打开切片，内部依赖仍全部独立。
  [使用说明](operations/independent-server.md)记录启动配置、鉴权、协议和关闭行为。
  [验证报告](reports/independent-server-m0.md)记录 workspace 检查、新服务测试与覆盖率。

- [ADR-021：Codex 原生任务取消固定运行时限](decisions/adr-021-codex-unlimited-runtime.md)（Accepted）：
  原生 writer 可持续执行，取消与失联回收保留；桌面中断告警使用独立布局并区分恢复问题。
  [验证报告](reports/codex-unlimited-runtime.md)记录时钟与浏览器回归。

- [NEC-345：Codex Thread 导入时保留 Session Agent 配置](decisions/NEC-345/adr-001-codex-import-session-agent.md)（Accepted）：
  同步原生 Thread 时用其 model/reasoning metadata 建立 Session 自有 Agent，并以可恢复的 catalog →
  Project 两阶段流程补录 Codex Provider 缺失的模型与推理等级。[实现与验证报告](reports/codex-import-session-agent.md)记录回归与覆盖率。

- [NEC-344：移除 Project JSON archive 接口](decisions/NEC-344/adr-001-remove-project-archive-interfaces.md)（Accepted）：
  删除 CLI、HTTP 与 application/contract 内的 Project archive 导入导出能力；Project 恢复统一使用
  目录内的 `.ait/project.sqlite3` 与既有目录打开流程。[实现与验证报告](reports/nec-344-remove-project-archive-interfaces.md)
  记录测试与覆盖率。

- [ADR-020：Codex 输出预算与超限诊断](decisions/adr-020-codex-output-limits.md)（Accepted）：
  Codex 原生输出默认上限独立为 8 MiB，超限显示指标、实际值与上限；大输出的实时预览和
  完整历史采用不同传输边界。[实现与验证报告](reports/codex-output-limits.md)记录回归与覆盖率。

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
