# 概念与架构文档

## 当前基线

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

- `decisions/adr-012-codex-native-tool-set.md`：仅 Codex Provider 使用 core 原生工具、app-server 分层提示词，以及两轮 Python Hello World 真机验收。

- `decisions/adr-011-default-api-tool-set.md`：默认 System Prompt、参考 DeepSeek Harness 的工具定义、模型覆盖与 API 请求组装；工具执行仍归宿主。

- `decisions/NEC-148/adr-001-reliability-portability-baseline.md`：结构化可观测性、无凭证归档、SQLite 备份与性能基线。
- `decisions/NEC-152/local-api-cli-vertical-slice.md`：本地 HTTP/CLI 纵向切片、SSE cursor 重连、SQLite 恢复与可执行验收说明。
- `decisions/NEC-147/adr-001-message-session-store-boundaries.md`：MessageStore 初始化与 append-only 边界、独立 SessionStore，以及 append 后 CAS 的失败保留语义。
- `decisions/NEC-149/adr-001-project-path-and-instruction-snapshots.md`：Project 路径授权、指令优先级/revision 与新 Session 根快照事务边界。
- `decisions/NEC-146/adr-002-split-sqlite-poc-v3.md`：系统目录与 Project `.metafab` 双层 SQLite 设计及 SQL PoC。
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

- `operations/reliability-security-observability.md`：数据保留、附件 mark-and-sweep、数据库备份/恢复、可靠性测试矩阵与性能基线。
- [CLI 用户流程](../workflows/README.md)：逐个用户目标的可执行步骤、可观察结果、失败恢复、当前差距与 CLI 集成测试映射。

配套设计仍保留各自原始评审状态；实现前若与 ADR-001 v4 冲突，以 v4 及明确列出的 Accepted 修订为准。同号 ADR 来自不同设计 issue，因此目录包含 issue 编号以避免歧义。

## 历史归档

`archive/domain-model/` 保存 NEC-144 初稿及 ADR-001 v1-v3，只用于追溯，不应作为新实现依据。
