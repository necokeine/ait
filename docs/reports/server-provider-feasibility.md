# server-provider 与 Workspace automation/state 拆分可行性

- 日期：2026-09-24。
- 状态：拆分前调研快照；已按 [ADR-031](../decisions/adr-031-server-provider.md)实施，
  当前结果见 [实施报告](server-provider-extraction.md)。
- 范围：当前工作树中的独立 server，包含已完成的 metadata/filesystem 拆分。
- 结论：可以按 Agent → server-provider、Workspace automation/state → server-metadata 组织；
  state 必须通过端口访问 Agent，setup/script 则明确扩展 metadata 的职责到 Workspace 自动化。

## 接口归属

这里统计已实现的业务请求，不把未实现的方法、服务端事件或 HTTP 入口算入。

| 当前业务 | 数量 | 建议归属 |
| --- | ---: | --- |
| Agent preset configure/get/list/default get/set | 5 | server-provider |
| Agent runtime list/history/get/update/archive/delete/detach/attention/items close | 9 | server-provider |
| Workspace setup status/run、script list/start/stop | 5 | server-metadata |
| Workspace clear_attention/mark_unread | 2 | server-metadata，通过端口委托 Agent 操作 |
| 合计 | 21 | provider 14，metadata 7 |

依据是 [preset 方法](../../crates/server-provider/src/protocol/agent.rs)、
[runtime 方法](../../crates/server-provider/src/protocol/agent_lifecycle.rs)、
[automation 方法](../../crates/server-metadata/src/protocol/workspace_automation.rs) 和
[state 方法](../../crates/server-metadata/src/protocol/workspace_state.rs)。
此前按依赖把 Workspace attention 计入 Agent 相关的 16 个接口；按业务入口归属拆分时，
这两个方法计入 metadata，因此为 14 + 7。

## server-provider 的范围

该名称可用，但需要定义为独立 server 的 Agent/Provider 能力包，覆盖配置、目录、持久化和
原生 session 协调。内部仍区分端口、应用服务与适配器，不把 Agent 等同于 SDK 对象。
[ADR-001 v4](../decisions/NEC-150/adr-001-core-domain-model-v4.md) 的领域边界和
[ADR-027](../decisions/adr-027-independent-agent-session-manager.md) 的原生 session 所有权继续有效。

建议迁移：

- `server-protocol::agent/agent_lifecycle` → provider 的 `protocol`。
- `server-api::agents/agent_runtime` 的解码、校验、投影和业务错误 → provider 的 `rpc`。
- `server-application::agents/agent_runtime/agent_manager` → provider 的 `service`。
- `server-ports::agent/agent_runtime/agent_session` → provider 的 `ports`。
- `server-storage` 的 Agent SQLite catalog、升级/备份、回执及 Agent JSON registry → provider 的 `storage`。
- Workspace attention 对 Agent 的筛选和更新实现 → provider 的适配模块。

建议保留 `server-domain` 的纯 Agent 类型作为内部依赖，继续由 crate 依赖守卫保证领域纯度；
不要为了维持旧导入路径而让 domain 反向 re-export provider。Provider 的协议、运行协调和存储
不再散落在旧横向分层 crate 中。若后续还要合并纯领域 crate，应单独明确如何维持纯依赖约束。

现有两种 Agent 数据不能因为这次移动而合并：preset 是带不可变 revision 的配置，
runtime record 是 Paseo 原生 session 的持久化快照。目前没有自动将 preset 转成 session 的生产链路。
保留 `catalog.sqlite3` 和 `agents/agents.json` 的格式、路径及含义。
通用 JSON 原子文件引擎仍可复用 metadata；Agent 类型、校验与写入权由 Agent 侧持有。
宿主的 `OwnedCatalog` 继续保护 data-dir lease，关闭数据库前不能释放实例锁。

生产 server 尚未组装 AgentManager，`AgentClient/AgentSession` 的具体实现目前只有测试 fake。
这次拆分不能据此把 create/resume/send/cancel、Provider discovery 或模型执行标记为已实现。
`agent.history.get.request` 现在查询的是历史 Agent 目录，也不是完整消息时间线。

## Workspace setup/script 可以完整归入 metadata

[WorkspaceAutomation](../../crates/server-metadata/src/service/workspace_automation.rs) 只依赖 Workspace
registry 和自动化端口，没有 Agent 或 filesystem 依赖。协议 DTO 已在 metadata。
服务、端口、快照结构、RPC 投影可以直接迁移。

[LocalWorkspaceAutomation](../../crates/server-metadata/src/local/workspace_automation.rs) 读取 `paseo.json`，
管理 setup 线程、脚本子进程、端口、输出截断和进程回收。它只需要标准库、serde_json、tempfile
以及 Unix libc 常量，因此也能迁入 `server-metadata::local::workspace_automation`，无须引入
server-filesystem、Provider、Tokio 或 HTTP 依赖。服务仍通过端口调用，由宿主注入本机实现。

建议按用户希望的业务聚合方式把这一整套迁入 metadata，并在新 ADR 中明确：metadata 同时负责
Workspace 管理和自动化。执行器放在 `local`，与 `model`、`storage` 分开。
这是对 ADR-029/030 中 shell 执行留在 server-workspace 的边界更新，不能只改 import 后沿用旧说明。
沿用前两次纵向拆分的取舍，协议消费者会间接编译所在能力包的本机依赖。

setup/script 的进程状态和日志快照当前在内存中；持久化的是 Workspace 的信任来源等记录。
迁移不应顺带引入重启后自动恢复脚本或新的状态文件。
Worktree 创建后启动 setup 的跨能力协调仍由宿主完成，保持现有返回结果、执行顺序和失败语义。

## Workspace state 需要依赖倒置

[WorkspaceState](../../crates/server-metadata/src/service/workspace_state.rs) 的两个方法实际更新
`PersistedAgentRuntimeRecord`：清除非 permission 提醒，或选择最新已完成的根 Agent 标记未读。
它不是独立的 Workspace 状态文件读写。

直接把现有实现搬进 metadata，并让它使用 provider 的 Agent registry，会形成
`metadata → provider → metadata` 循环。建议：

1. metadata 拥有 Workspace state 的 DTO、RPC、Workspace 校验与批次结果协调。
2. metadata 定义窄的 `WorkspaceAttention` 端口，传递 Workspace ID、时间、Agent ID 和稳定错误；
   不暴露完整 Agent record、Provider session 或通用 Agent registry。
3. provider 实现端口，拥有 Agent 候选筛选、父子关系判断及 registry 原子更新。
4. binary 注入同一 Agent registry 的共享句柄，避免第二套缓存和写入器。

端口设计需要保留现有一次 Agent 列表读取、批次部分成功、结果顺序、归档/内部 Agent 过滤、
permission 提醒保留、根 Agent 选择、单调时间及更新阶段复核等语义。
不能把现有批量请求改成全成或全败，也不能用完整 Agent CRUD 接口伪装窄端口。
Workspace/Worktree recovery 已在 filesystem，继续按物理恢复能力归属，不随 state 名称搬回。

## 拆后依赖与宿主职责

建议的直接 workspace 依赖：

| Crate | 业务依赖 |
| --- | --- |
| server-domain、server-metadata | 无 |
| server-provider | server-domain、server-metadata |
| server-filesystem | server-metadata |
| server-protocol | server-provider、server-filesystem、server-metadata |
| server-api | server-protocol 及三个能力包 |
| server-bin | 组装上述服务及适配器所需的 server crate |

metadata 不依赖 provider；provider 通过实现 metadata 的端口参与 Workspace 操作。
provider 的 SQLite 依赖是其存储实现所需，不传回 domain。
通用协议 crate 只聚合业务方法并映射错误，能力包不能反向依赖它。

迁完后 `server-application`、`server-ports`、`server-storage`、`server-workspace` 的现有职责
均有新归属，可删除空 crate 和旧依赖，不保留转发壳。依赖守卫需要注册单数 `server-provider`；
当前预留的复数 `server-providers` 并不是已有实现。

API 中这 21 个业务接口的执行和投影可以全部移出，仍保留路由、鉴权、HTTP/WS、连接预算、
blocking 调度、队列、订阅、任务取消和 drain，以及 Worktree 创建后启动 setup 的宿主协调。
因此“业务实现移出”不意味着没有 HTTP 入口或不再转发业务 RPC。

## 实施验证要求

迁移以保持 wire/disk 格式和 195 个可协商名称、102 个已实现 capability 为目标。
除模块测试外，重点回归 preset CAS/回执/升级，Agent registry 原子持久化，manager 恢复/关闭失败
所有权，attention 批量结果，setup 信任准入/脚本清理，以及真实 WS 路由与宿主关停。
按仓库要求完成格式、lint、workspace tests 和覆盖率报告；更新 ADR、依赖守卫与文档索引。

## Test coverage

Not applicable — no Rust behavior changed。本轮只做源码、调用链和依赖调研，新增本文及索引；
未执行 Rust 测试或重新采集覆盖率。已核对 `cargo metadata --format-version 1 --no-deps --offline --locked`。
已有验证基线见 [filesystem 拆分报告](server-filesystem-extraction.md)，不能视为新拆分已通过验证。
