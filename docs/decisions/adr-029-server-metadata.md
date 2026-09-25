# ADR-029：统一 Paseo Project 并纵向拆出 server-metadata

> Tokio 依赖限制已由 [ADR-037](adr-037-server-model-context.md) 修订；metadata 可按需要使用 Tokio。

- 状态：Accepted。
- 日期：2026-09-24。
- 授权：废除独立 server 早期 Project，全部使用新的 Project/Workspace，并开始完整拆分。
- 范围：独立 server；旧 daemon 与 `ait-*` crate 不变。
- 替代：ADR-023 的 Project 租约切片、ADR-025 对该切片的过渡保留，以及 ADR-022 中受此拆分影响的 crate 依赖表。

> 后续边界更新：GitHub provisioning、Worktree/recovery DTO 和二进制文件 framing 已按
> [ADR-030](adr-030-server-filesystem.md) 迁入 server-filesystem；Workspace 自动化和 state 按
> [ADR-031](adr-031-server-provider.md) 归入 metadata，当前依赖表以 ADR-031 为准。

## 唯一 Project 模型

server 统一使用 Paseo `PersistedProjectRecord` 与 `PersistedWorkspaceRecord`。
新建身份仍为 `prj_` / `wks_`，已有字符串身份、JSON schema、文件位置与行为保持兼容。

删除 `project.open/list/get/close` 的协议、路由、capability、应用实现、UUID Project 类型、
根 Message 初始化、Project SQLite adapter、Project 路径/身份锁与 owner epoch。
这些旧方法不再协商，对已握手连接返回 `method_not_found`。
Project/Workspace 统一通过 `project.*.request`、`workspace.*.request` 操作。

不自动把旧 SQLite Project 转成 Paseo Project，也不删除磁盘上的历史项目数据。
用户可以通过新接口注册已有目录；新接口不会读取或维护旧项目数据库、租约和根 Message。
Agent presets 继续使用 `catalog.sqlite3`，新 catalog 只创建 Agent 表；旧 catalog 的历史
Project 表保持原样且不再被业务代码访问。既有 v1 Agent schema 升级及备份继续有效。

## 新 crate 的所有权

`server-metadata` 是完整业务切片，拥有：

- `model`：Project/Workspace 持久化记录、labels 及校验。
- `ports`：registry、标签事务、配置、图标、目录检查与 GitHub provisioning 契约。
- `service`：directory、标签订阅和 server 配置/状态用例。
- `protocol`：Project/Workspace/label/config/icon/GitHub 的业务 DTO，以及 server identity、
  lifecycle、ping 和 heartbeat 方法声明；Workspace worktree/setup/recovery 的 DTO 也归这里。
- `rpc`：消息解码、业务执行、descriptor 投影、错误分类、标签快照/事件激活及生命周期意图。
- `storage`：Project/Workspace JSON registry、标签恢复 journal、配置/图标与 server-id 文件。

registry 磁盘提交、缓存发布、observer 差异和 labels prepared/committed 恢复流程不变。
标签文件与 Workspace 文件的事务实现留在同一个 crate 内。Agent runtime registry 复用
metadata 的通用原子 JSON 引擎；其 Agent 类型及校验仍由 Agent 侧拥有。

## 依赖与宿主边界

`server-metadata` 不依赖任何 workspace crate，也不引入 Tokio、HTTP、SQL 或 provider。
模型、端口、用例和存储仍以模块区分；存储实现端口，用例不直接依赖具体文件 adapter。

| Crate | 允许的直接 workspace 依赖 |
| --- | --- |
| server-metadata、server-domain | 无 |
| server-protocol | server-metadata |
| server-ports | server-domain |
| server-application | server-domain、server-ports、server-metadata |
| server-storage、server-workspace | server-domain（有实际消费者时）、server-ports、server-metadata |
| server-api | server-application、server-domain、server-protocol、server-metadata |
| server-bin | 当前独立 server crates |

通用 Hello、RPC 信封、版本协商、二进制 framing 仍由 `server-protocol` 定义。
该 crate 消费 metadata DTO 和业务错误，metadata 不反向引用公共信封。
共同使用的 ServerInfo/Version/Limits/Lifecycle 定义归 metadata，protocol 仅导出同一类型。
这使协议 crate 通过 metadata 间接编译文件存储依赖，是单 crate 纵向封装的明确取舍；
不再宣称所有协议消费者的依赖图都只有 Serde。`server-domain` 保持独立纯领域依赖。

`server-api` 负责 HTTP/WS、鉴权、消息队列、预算、连接订阅所有权、blocking 调度与 drain。
metadata 标签事件经注入的 sink 交付，监听句柄在响应入队后激活，断连时由连接释放。
restart/shutdown 只在 metadata 产生意图，宿主执行 admission 关闭、任务 drain 和重启。
data-dir 的 `instance.lock` 仍由宿主持有；metadata 只负责其保护下的 server-id 文件。

Git/worktree/Forge/shell/文件传输/Agent session 的实际执行继续留在原运行模块；
它们统一使用 metadata 的 Project/Workspace 记录与投影。目录检查和 GitHub CLI 实现
由宿主注入 metadata 定义的端口，避免 metadata 依赖本机执行 adapter。

## 心跳与兼容性

`connection.ping` 的 DTO、校验与 echo 处理迁入 metadata，行为保持不变。
`session.heartbeat` 在本次拆分时保持占位；后续已按 [ADR-034](adr-034-agent-config-session-events.md)
接入进程内 presence 与通知策略，不新增心跳文件。
WebSocket Ping/Pong 控制帧、HTTP health/readiness 和物理连接存活由 transport 拥有。

原有 188 个 Paseo canonical methods 不变；删除四个额外的旧 Project 方法后，生产服务
公布 195 个可协商名称、102 个已实现 capability。首次迁移不改变公共协议版本或新体系磁盘格式。

## 验证

依赖守卫检查新 crate 无反向依赖和旧 Ait 依赖，并继续禁止 domain/runtime 污染。
迁移保留原有 schema fixture、registry/labels 恢复、配置/图标和真实 WS 回归。
新增旧方法拒绝、新体系重启持久化、历史项目文件保留和 Agent-only catalog 初始化回归。
实际检查、覆盖率及限制记录在 [实施报告](../reports/server-metadata-extraction.md)。
