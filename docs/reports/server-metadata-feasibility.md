# server-metadata 拆分可行性调研

- 日期：2026-09-23。
- 代码基线：`00c8e82`；调研开始时工作区干净。
- 范围：独立 `server`、`bins/server`、`crates/server-*`，不涉及旧 daemon/Desktop。
- 状态：历史调研。后续授权废除早期 Project，实施结论以
  [ADR-029](../decisions/adr-029-server-metadata.md) 和
  [实施报告](server-metadata-extraction.md) 为准；下文描述调研时的代码基线。

## 结论

可行。适合将 Project / Workspace 元数据及 server 状态、配置、应用层 ping 等做成一个
完整的纵向 crate：`server-metadata` 同时拥有业务协议、记录、用例、存储接口与文件实现。
只搬 registry 或新增一个转发 facade，不能满足“从 WebSocket 包定义到底层存储全部拆出”。

这属于中等规模的边界重构。初步迁移候选的 31 个生产源码文件共 8,683 个物理行，
覆盖协议、记录/端口、directory/labels/daemon 用例与 API、registry/config/icon 存储。
这个数字包括文件内尚需分离的 glue 和少量其他功能，不是精确净迁移量；不含测试、fixture、
连接路由/宿主改动、GitHub provisioning，以及早期 Project lease 切片。

可以保持现有 JSON、路径和 wire 行为，首次迁移没有必须改数据格式的理由。
真正需要处理的是 crate 依赖方向、连接所有权、跨文件事务，以及并存的两套 Project 模型。

## 现状：一条业务链跨越七个 crate

| 职责 | 主要位置 | 拆分时的处理 |
| --- | --- | --- |
| 业务请求、响应、descriptor、capability | `server-protocol/src/{directory,project,workspace,project_config,project_icon,workspace_labels,daemon}.rs` | 元数据业务定义迁入新 crate；保留 wire 字段、枚举及缺失/null/值三态 |
| 消息解码、结果投影、筛选/排序/分页 | `server-api/src/{directory,workspace_labels,daemon}.rs` | 业务处理和投影迁入；连接调度、发送和生命周期执行留给宿主 |
| Project/Workspace 用例 | `server-application/src/directory.rs` | registry 操作、配置、图标、元数据创建/归档/更新迁入 |
| 标签与 server 配置用例 | `server-application/src/{workspace_labels,daemon}.rs` | 连同订阅事件模型与业务状态迁入 |
| 持久化记录 | `server-domain/src/{registry,workspace_labels}.rs` | 迁入新 crate 的纯模型模块 |
| registry、配置、图标、标签端口 | `server-ports/src/{registry,provisioning,workspace_labels,daemon}.rs` | 随业务迁入；外部 Git/目录能力由宿主注入 |
| JSON 文件与恢复 | `server-storage/src/registry/`、`workspace_labels.rs`、`daemon_config.rs` | Project/Workspace 文件实现和事务恢复一起迁入 |
| 项目配置、图标文件 | `server-workspace/src/{project_config,project_icon}.rs` | 属于元数据文件存储，也应迁入 |
| 进程组装与身份 | `bins/server/src/{host,instance}.rs` | 改为组装新 crate；持久 server-id 可迁入，运行锁由宿主持有 |

生产组装位于 [host.rs](../../bins/server/src/host.rs)。现有文件包括：

```text
<data-dir>/projects/projects.json
<data-dir>/projects/workspaces.json
<data-dir>/projects/workspace-labels.json
<data-dir>/projects/workspace-labels.transaction.json
<data-dir>/projects/icons/...
<data-dir>/config.json
<data-dir>/server-id
<project-root>/paseo.json
```

`instance.lock` 是宿主运行排他资源；心跳不是这个锁的替代。
启动 `config.toml` 与可变 `config.json` 也不是同一份配置。

## 建议的业务范围

首批完整迁入 Paseo Project/Workspace 的注册、打开/创建、列表、名称/title、pin、归档、
Project remove、labels、配置和图标，以及 daemon 状态/可变配置/诊断、应用层 ping。
同步迁移对应方法声明、请求/响应、事件、descriptor、记录校验、用例、存储及测试。

最近加入的 `project.github.clone.request` 与
`workspace.github.search_repositories.request` 已并入 `Directory`，不能在搬文件时漏掉。
建议其业务 DTO 和“clone 后注册 Project”的协调随 directory 迁入，GitHub CLI 执行通过
迁入的端口继续注入 `server-workspace` 的实现，避免 metadata 反向依赖 workspace adapter。

Workspace descriptor 中的 Git/Forge/script runtime **数据形状**随 descriptor 迁入，
实际 Git 操作、Forge 请求、shell/setup、文件传输、Agent session 执行仍有各自的运行职责。
现有 descriptor 投影给出 `Done`、空 scripts、缺失 runtime 等默认值；搬迁不能顺便宣称
这些尚未聚合的运行态已经实现。

`workspace_state.rs` 将“改 Agent attention”和“恢复 Git worktree”放在一起，
`worktrees.rs` 也同时包含 Git 操作与 registry 协调。这些是跨能力用例：
可以调用新 crate 的 registry/投影接口，不应仅凭 `workspace.*` 前缀将 Agent/Git 执行搬入。
如以后要求整个 Workspace 生命周期都归 metadata，需进一步用宿主注入的 Agent/Git 端口
迁入协调逻辑；不能让 metadata 直接依赖现有 application。

## 心跳实际有哪些

| 名称 | 当前行为 | 拆分建议 |
| --- | --- | --- |
| `connection.ping` | 已实现：校验并原样返回 `nonce`，在 routing 的 `dispatch_base` 中 | 为它明确业务 DTO 和处理入口，迁入 metadata |
| `session.heartbeat` | methods 表登记为 Event；入站 Event 统一走 placeholder，返回 `NotImplemented` | 方法归属随拆分登记；真正的 presence/超时语义属于后续功能实现 |
| WebSocket Ping/Pong 帧 | connection 的 receive 循环跳过控制帧，不维护业务存活时间 | 保留在 transport；现有应用代码没有周期探测/失联回收机制可搬 |
| `/healthz`、`/readyz` | HTTP 存活与准入状态 | HTTP endpoint 和 readiness 决策继续由宿主/transport 拥有 |

相关证据：[方法登记](../../crates/server-protocol/src/methods.rs)、
[base handler](../../crates/server-api/src/connection/routing.rs)、
[连接收包](../../crates/server-api/src/connection.rs)。

当前没有业务 heartbeat 文件存储。若后续补 heartbeat，应明确 connection/client 身份、
单调时钟、超时、断连清理和重启语义；不能在纯迁移中假设每次心跳都要写 registry。

## 必须解决的边界

### 1. 两套 Project 并存

`project.add.request` / `project.list.request` 使用 Paseo 字符串 ID 与 JSON registry。
另一套 `project.open/list/get/close` 使用 `Projects`、UUID ProjectId、owner epoch、
根 Message、SQLite 和目录/身份租约。

后一套是独立 server 自己的早期切片，不是旧 daemon；其
[SqliteCatalog](../../crates/server-storage/src/catalog.rs) 还与 Agent presets 共用 schema、
迁移和连接实现。因此不能把两个 `project.list` 当作同一入口，也不能把整个
`server-storage` 移进 metadata 后宣称只是迁移 Project metadata。

建议本轮边界以 ADR-025 的 Paseo metadata 为准，并在实施报告明确保留的 lease 切片。
如果“Project 全拆”包含所有历史 server Project 接口，则必须将 lease 协议/用例也纳入
单独迁移步骤，处理 SQLite 共用实现与根 Message 边界；这是额外范围，不能默默删除接口，
也不能把保留此切片的方案称为“所有 Project 代码都已迁出”。

### 2. 新 crate 不能反向依赖现有业务宿主

直接把原文件搬进去并保留原 import，会很容易形成：

```text
server-application -> server-metadata -> server-ports -> server-metadata
server-protocol -> server-metadata -> server-protocol
server-api -> server-metadata -> server-api
```

第二个环有具体消费者：`server-protocol/agent_lifecycle.rs` 使用
`ProjectPlacementPayload`，`worktrees.rs` 使用 `WorkspaceDescriptorPayload`。
metadata 如果为了复用 `ErrorCode` 而依赖 server-protocol，就无法让这些消费者单向复用
已经迁走的业务 DTO。

建议 `server-metadata` 不依赖现有 `server-*` crate，内部拥有自身的模型、端口、业务错误、
DTO 和文件实现。外部 crate 单向依赖它；`server-api` 将业务错误映射到公共错误信封。
现有 `server-domain` 移出 registry/labels 后保持独立，不反向依赖 metadata。

业务 method/capability 声明也应归 metadata，由总协议目录/路由聚合；不能在两个 crate
各维护一套同名方法。总路由继续精确匹配完整方法，保留 Request/Event/Response 方向检查。

通用 Hello、RPC envelope、版本协商、二进制 framing 留在 `server-protocol`。
`ServerInfo` 同时用于握手和 `server.info`，需要分清 transport 协商字段与 metadata 提供的
server facts，并由 API 组合成兼容的输出。不要为搬走这个共用类型引入双向依赖或复制
互相漂移的完整定义。

同一 crate 包含 DTO 和存储，就会放宽原 ADR-022 的纯横向 crate 分层。
可以用模块与可选 `storage` feature，使类型消费者单独构建时不启用文件实现；但 Cargo
feature 会在同一构建中统一，feature 不是严格的依赖隔离墙。若要求协议消费者的整个依赖图
绝不包含存储实现，就需要额外的纯契约 crate，这与“只新增一个 crate”的最小方案不同。

### 3. 标签事务必须整组迁移

`FileWorkspaceLabelStore`（现位于 `server-metadata/src/storage/workspace_labels.rs`）持有具体的
`FileBackedWorkspaceRegistry`，通过 `commit_workspace_label_mutation` 同时写 catalog、
workspace 与 prepared/committed journal；不确定结果会冻结 workspace 后续写入。

这些实现应在新 crate 内一起迁移，保留锁顺序、缓存发布、回滚/重开恢复和 freeze 语义。
不能把它拆成两个普通 CRUD 调用，或另外打开同一路径的独立 registry 实例。

### 4. 共享文件引擎与外部消费者

`server-storage/registry/agent_runtime.rs` 使用同一个私有 `FileRegistry<R>` 引擎。
Project/Workspace 搬走后，应显式保留可复用的文件引擎接口或安排其归属，不能让
metadata 为此反向依赖 Agent registry；Agent runtime record 不必随之迁移。

Agent directory、WorkspaceAutomation、WorkspaceState、Worktrees 使用 registry 记录/端口，
Worktrees 与 WorkspaceState 的 API 还使用 `directory::workspace_descriptor`。
迁移必须同时更新这些消费者，统一使用新 crate 的记录及投影函数。

### 5. 连接和宿主资源不能跟着业务 handler 直接搬

现有 API handler 直接使用私有 `Shared`、`Outbound`、`jobs::run` 和连接订阅集合。
metadata 应返回业务结果、事件/订阅句柄或宿主意图，API 负责受监督 blocking 调度、
响应发送和断连释放；订阅仍须“响应进入发送队列后再激活”，避免事件先于快照。

restart/shutdown 的协议与业务意图可归 metadata，真正停止 admission、drain 任务、
释放锁和重建 server 仍由 API/binary 执行。`server-id` 文件读写若迁入，宿主运行锁
仍须活到所有已接纳的阻塞写任务结束。

## 建议目录与迁移顺序

```text
crates/server-metadata/src/
  lib.rs
  protocol/       # 本能力的方法、DTO、事件、descriptor
  model/          # Project/Workspace/label records 与校验
  ports/          # registry、配置、图标及外部能力契约
  service/        # directory、labels、server metadata 用例
  rpc/            # 无 Axum/Shared 的业务解码、执行与结果投影
  storage/        # JSON registry、事务恢复、config、icon、server-id
```

内部仍按 model <- ports/service <- storage/rpc 的职责约束组织；业务 service 不依赖具体
文件实现，model 不引入 Tokio/HTTP/SQL/provider。测试放在各模块独立子文件。

1. 写 Proposed ADR，确定 metadata 与 Project lease 的范围、更新依赖规则和方法归属表。
2. 迁移 records、DTO、端口、registry、标签恢复事务与文件实现，保持 wire/磁盘格式。
3. 迁移业务 handler/用例/投影，拆开连接 glue；宿主注入 GitHub、目录/Git 等外部能力。
4. 更新所有消费者、route/capability 聚合、启动组装及架构守卫，移除旧实现与重复定义。
5. 完成既有行为回归后，另行实现 heartbeat/presence；不在搬迁时扩大已实现能力声明。

[依赖守卫](../../bins/server/tests/dependencies.rs) 当前枚举了所有 server crate，
新建 `server-metadata` 会触发 `unregistered server package`。
正式实施要同步记录 ADR、更新 `docs/README.md` 与守卫，不能仅关闭守卫。

## 实施验收与 Test coverage

本次调研没有改 Rust、Cargo 或运行行为。Coverage：**not applicable — no Rust behavior
changed**；未生成新的 workspace/crate 行覆盖率、覆盖行数或 coverage artifact，
不把历史报告作为本次测量。

已完成静态模块/消费者/数据路径核查，`cargo metadata --format-version 1 --no-deps
--locked --offline` 成功解析基线依赖。
`cargo test -p server-bin --test dependencies --offline` 在上述基线上通过：2 passed，
0 failed，0 ignored；范围仅为 server-bin 的依赖守卫，默认 features，未执行其他测试目标。
文档的 7 个本地链接均存在，无行尾空白；`git diff --check` 通过。
本次不执行 workspace 全量 format/lint/行为测试；这些是正式改码时的验收要求。

实施后的最低回归范围：

- pinned fixture 的 JSON schema、camelCase、可选/null/值、legacy enum/ID 和包方向。
- registry 并发更新、顺序、lexical path 语义、写失败后的磁盘/cache 一致性与 observer 差异。
- labels prepared/committed journal、重开恢复、不确定状态冻结和删除后的 workspace 更新。
- 真实 WS 的 Project/Workspace/config/icon/labels/ping；跨连接订阅隔离、释放、背压与 drain。
- Agent placement、worktree/setup/recovery、GitHub clone 注册等跨模块调用的行为保持。
- 重启后 server-id 稳定、instance_id 更新，文件格式与位置保持，旧目录无需迁移可读。
- 确保 `session.heartbeat` 在未实现时仍不进入 implemented capabilities。

改码交付前运行 `cargo fmt --all --check`、
`cargo clippy --workspace --all-targets -- -D warnings`、`cargo test --workspace` 与
`cargo llvm-cov --workspace --html`。报告 workspace 和新 crate 的实测行覆盖率及覆盖行数，
给出可审阅 artifact，并把测试通过情况与覆盖率分开陈述。
