# 概念与架构文档

## 当前基线

- `decisions/NEC-150/adr-001-core-domain-model-v4.md`：核心领域模型，Accepted，是术语与聚合边界的权威版本。
- `glossary.md`：从 ADR-001 v4 提炼的快速术语表。

## 配套设计

- `decisions/NEC-146/adr-002-split-sqlite-poc-v3.md`：系统目录与 Project `.metafab` 双层 SQLite 设计及 SQL PoC。
- `decisions/NEC-151/ADR-002-agent-provider-contract.md`：Agent 配置与 Provider Adapter 契约。
- `decisions/NEC-151/ADR-003-agent-adapters-codex.md`：Agent Adapter crate 与 Codex 集成边界。
- `decisions/NEC-154/adr-002-rust-workspace-runtime-architecture.md`：Rust workspace 与运行时架构，Accepted 实现基线。

配套设计仍保留各自原始评审状态；实现前若与 ADR-001 v4 冲突，以 v4 为准。同号 ADR 来自不同设计 issue，因此目录包含 issue 编号以避免歧义。

## 历史归档

`archive/domain-model/` 保存 NEC-144 初稿及 ADR-001 v1-v3，只用于追溯，不应作为新实现依据。
