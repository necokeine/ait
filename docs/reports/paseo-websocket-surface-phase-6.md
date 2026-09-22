# Paseo WebSocket 接口移植：第六阶段

- 日期：2026-09-22；分支：`new`。
- 基线：`b0da2a3`；Paseo 来源：`2c8e8a826810337492cc5a38bb0bbd705b6fb632`。
- 决策：[ADR-026](../decisions/adr-026-canonical-paseo-websocket-surface.md)。

## 已接通方法

本阶段接通 Agent runtime 目录与元数据生命周期分组的 9 个规范 WebSocket 方法。累计已接通 47 个
规范方法，剩余 144 个 catalog 条目仍为 catalog-only。

| 规范方法 | Paseo 来源名 | 状态与行为 |
| --- | --- | --- |
| `agent.list.request` | `fetch_agents_request` | unarchived 目录、可选 active scope、placement/filter/sort/page；subscribe/sync 明确拒绝 |
| `agent.history.get.request` | `fetch_agent_history_request` | 默认包含 archived、全文 placement search、filter/sort/page |
| `agent.get.request` | `fetch_agent_request` | full ID、唯一 prefix、精确 title；missing/ambiguous 使用 inline error |
| `agent.update.request` | `update_agent_request` | trim title、合并非空 label patch、原子持久化 |
| `agent.archive.request` | `archive_agent_request` | 清 attention、收敛运行状态、递归归档或 detach delegated children |
| `agent.delete.request` | `delete_agent_request` | 永久删除 durable snapshot；missing 与存储失败显式报错 |
| `agent.detach.request` | 同名 | 删除 parent 和全部 open-tab labels |
| `agent.attention.clear.request` | `clear_agent_attention` | 接受一个或多个 Agent ID，返回更新后的 snapshots |
| `agent.items.close.request` | `close_items_request` | 独立归档每个 Agent 并省略失败项；当前仅接受空 terminal 集合 |

所有下划线来源名只作为 catalog 审计记录，不注册为 wire alias。请求继续使用新 server 的统一
envelope，Paseo method payload 位于 `params`/`result`。

## 实现边界

`server-domain::agent_runtime` 复制固定 Paseo 快照的 `StoredAgentRecord`、status、attention、config、
runtimeInfo 和 persistence handle 结构。它与 ADR-024 的 Agent preset/revision catalog 是不同模型：前者
表示一次可运行/可恢复会话的 durable snapshot，后者表示新 server 的版本化配置模板。

`server-ports::agent_runtime::AgentRuntimeRegistry` 是存储边界；
`server-storage::FileBackedAgentRuntimeRegistry` 使用现有原子 JSON registry 基础设施，把 records 写入
`<data-dir>/agents/agents.json`。启动先验证已有文档；update 不能改变 ID；无效文档、无效 record 和 I/O
失败不会被静默覆盖。

`server-application::agent_runtime::AgentRuntimeDirectory` 组合 Agent、Workspace 和 Project registry：

- list/history 排除 internal records，并只为可解析 placement 的 records 建 row；active scope 同时要求
  Agent、Workspace 和 Project 未归档；
- 支持 labels、project keys、status、attention、thinking option、archive、case-insensitive search、
  多字段排序以及 1–200 page limit；
- snapshot 和 updated sort 使用 `updatedAt` / `lastActivityAt` 中较新的合法时间，wire 上规范化为 UTC ISO；
- get 依次解析完整 ID、唯一 ID prefix 和精确完整 title，且允许返回 placement 已消失的公共 record；
- archive 先清除 attention，把 initializing/running 改为 idle；同 Workspace 且没有 open-tab label 的
  delegated child 递归归档，跨 Workspace 或仍有 open-tab owner 的 child 执行 detach；
- delegated Agent 的 detach 删除 `paseo.parent-agent-id` 和全部 `paseo.open-agent-tab.*`；已经没有 parent
  label 的 Agent 保持不变；delete 永久移除 record。

API 从 stored record 生成 Paseo Agent snapshot。由于尚无 Provider runtime，snapshot 明确返回
`providerUnavailable:true`、`persistence:null`，省略 `activeTurn`，并返回空 available modes/pending permissions，
并只声明 stored snapshot 可支持的静态 capability flags。生产 binary 在每次启动和 restart 时组装这一
全新 registry/use case，不读取旧 Ait Agent 或 Session 数据。

## 与 Paseo 的对齐和差异

1. durable 字段名、serde defaults、status/attention 值和 public snapshot 主体取自 Paseo
   `StoredAgentRecord`/Agent schema。`config.toolPolicy`、`config.features`、`owner` 及 provider 扩展值保留为
   JSON；Paseo 的对应 feature/owner/tool schema 更严格，当前只在 registry 边界限制必填字符串、控制字符
   和大小。Rust `Option` 还会把部分“字段缺失”和显式 null 收敛为同一状态，因此嵌套 nullable 字段不能逐字节
   保持原 JSON；有效输入投影后的语义一致。
2. Paseo 每个 Agent 写入按 sanitized cwd 分组的独立 JSON 文件，并可扫描多个目录；当前用一个原子
   `agents.json` 数组。当前方案有完整 document 原子替换和进程内串行化，但写放大、手工迁移路径和单个
   record 损坏的隔离粒度不同。
3. filter、sort fields、history 默认包含 archived 和 ID/prefix/title lookup 与 Paseo 对齐；空 status/project
   filter 按无过滤处理。history search 当前是四个名称字段的大小写无关 substring，Paseo 还支持 typo-tolerant
   scoring。page cursor 是十进制 offset；Paseo cursor 编码最后一个 sort value/ID，因此并发插入或重排时的
   翻页稳定性不同。list 的 `subscribe`/`sync` 返回 `unsupported_capability`，没有 Agent directory event、
   generation 或 replay journal。
4. Paseo active list 在 Provider 不可用时会滤除/重新投影相关记录。当前没有 Provider catalog，为了使导入
   的 durable records 可查询，仍返回所有 public placed records 并统一标记 `providerUnavailable:true`；
   persistence resume handle 不外发。这是当前最明显的目录行为差异。
5. Project registry 没有保存/探测 checkout remote URL，所以 placement `checkout.remoteUrl` 为 null；
   branch、worktree root、managed ownership 和 main repo root 来自现有 Workspace record。Paseo 可从 live
   checkout service 补充更实时的 Git facts。
6. metadata update、attention clear、detach、archive metadata 和 delegated-child 分流与 Paseo 对齐。
   当前 archive 不取消 live Provider turn、不关闭 native session、不调用 plugin hook，也不发布 Agent/
   Workspace event；这些行为必须在新的 Provider/Event runtime 建立后接入。
7. delete 对 missing target 和 storage failure 都 fail closed。Paseo 的部分清理路径会记录文件删除失败并
   继续发出 deletion update；当前不会在 durable 删除结果不确定时声称成功。
8. `agent.items.close.request` 对 Agent 逐项归档并省略失败项，与 Paseo batch 结果一致；非空
   `terminalIds` 返回 `unsupported_capability`，而 Paseo 会调用 Terminal manager 并返回逐项结果。
9. create/resume/import/refresh/send/wait/cancel/rewind、config/mode/model/thinking/feature、permission、timeline、
   provider subagent 和 command list 尚未发布。它们依赖真正的 Provider execution、event 和 conversation
   persistence，不能复用 ADR-024 preset 或返回假成功。
10. 和前五阶段一致，新 server 使用统一 request/response/error envelope，不复制 Paseo method payload 内的
    第二层 `requestId`。
11. registry 接受时间字段为字符串，并在合法 RFC 3339 时间之间选取最新值；wire 投影会规范化合法时间。
    Paseo 在部分投影路径会对非法时间直接抛错，当前为保持已加载记录可诊断，会原样返回无法解析的时间字符串。

## 测试执行

从 Paseo Agent storage/directory/lifecycle tests 对应移植：StoredAgentRecord defaults/round-trip、canonical
method、camelCase filter、显式 null thinking、active/history archive 规则、placement search、multi-sort、
pagination、full/prefix/title lookup、metadata update、attention clear、detach、同 Workspace child archive、
跨 Workspace/open-tab handoff、delete、storage reopen/invalid file/identity protection、snapshot projection、
legacy method rejection，以及真实 binary WebSocket 读写和重启前 durable 文件检查。

本阶段新增 21 个测试：domain 2、protocol 4、storage 3、application 8、API 3、真实 binary WebSocket 1。
阶段性验证：

```text
cargo llvm-cov -p server-bin -p server-api -p server-application -p server-domain \
  -p server-ports -p server-protocol -p server-storage -p server-workspace \
  --json --summary-only --no-fail-fast -j1
  208 passed, 0 failed, 0 ignored
cargo clippy --workspace --all-targets -- -D warnings
  passed
cargo fmt --all -- --check
  passed
```

coverage 运行中的 12 个新 server test target 共 208 个测试通过，0 失败、0 ignored。生产 Rust 源码为
8,956/10,187 行，行覆盖率 87.92%；本阶段 4 个带可执行行的 Agent runtime 模块为 854/999 行，行覆盖率
85.49%。纯 DTO/trait 文件没有 LLVM 可执行行，不计入分母。完整 crate 级数据、命令和基线记录在
[phase 6 coverage artifact](paseo-websocket-surface-phase-6-coverage.json)。

`cargo test --workspace --no-fail-fast -j1` 的 72 个普通 target 中，697 passed、1 failed、5 ignored；
唯一失败是旧 `ait-daemon` 的 `daemon_is_ready_and_rejects_unsent_native_recovery_without_replay`：启动超过
既有 15 秒 desktop readiness window。该测试不经过新 server crate，本阶段没有放宽产品时限或修改旧
daemon 来隐藏环境失败。完整结果记录在同一 coverage artifact。
