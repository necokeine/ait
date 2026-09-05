# 概念与架构文档

## 当前基线

- `decisions/NEC-150/adr-001-core-domain-model-v4.md`：核心领域模型，Accepted，是术语与聚合边界的权威版本。
- `decisions/NEC-161/adr-003-session-agent-binding-and-identities.md`：实现评审修订；Message 使用 UUID，Project description 默认空字符串，Session 持有可在空闲时显式重绑的 Agent。
- `decisions/NEC-162/adr-004-electron-desktop-boundary.md`：Electron 桌面端接入、Rust API/设置单一语义来源，以及从任意 Message 原子创建分支的边界。
- `glossary.md`：从 ADR-001 v4 提炼的快速术语表。

## 配套设计

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
- `decisions/NEC-174/adr-002-codex-run-reasoning-effort.md`：Codex 推理强度作为 Run 级固定覆盖值的传递、校验与桌面能力投影边界。
- `decisions/NEC-176/adr-005-session-naming-and-generated-metadata.md`：Session 手工命名、首次交互临时标题与只读 AI 检索元数据生成。
- `decisions/adr-006-project-git-provenance.md`：Project 初始 Git 基线、可选远端仓库地址，以及 human user Message 的干净 HEAD 快照约束。

## 运维手册

- `operations/reliability-security-observability.md`：数据保留、附件 mark-and-sweep、数据库备份/恢复、可靠性测试矩阵与性能基线。

配套设计仍保留各自原始评审状态；实现前若与 ADR-001 v4 冲突，以 v4 为准。同号 ADR 来自不同设计 issue，因此目录包含 issue 编号以避免歧义。

## 历史归档

`archive/domain-model/` 保存 NEC-144 初稿及 ADR-001 v1-v3，只用于追溯，不应作为新实现依据。
