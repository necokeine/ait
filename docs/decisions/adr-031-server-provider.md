# ADR-031：拆出 server-provider 并统一 Workspace 自动化与 state 入口

> 后续 [ADR-032](adr-032-server-native-provider-execution.md) 已组装原生 Provider worker；
> 本文 14 个 Agent 方法与 102 个已实现 capability 是拆包完成时的计数。

- 状态：Accepted。
- 日期：2026-09-24。
- 授权：按 [可行性分析](../reports/server-provider-feasibility.md)实施拆分。
- 范围：独立 server；不修改旧 daemon 或 `ait-*`。
- 更新：ADR-022/024/027/029/030 中受影响的模块位置和 crate 依赖；保留既有协议与持久化语义。

## Agent 能力归属

新增单数 `server-provider`，作为 Agent/Provider 能力包。内部仍按职责分层：

| 模块 | 所有权 |
| --- | --- |
| `protocol` | Agent preset 和 Paseo Agent runtime 的 WebSocket DTO、14 个已实现方法声明 |
| `ports` | AgentCatalog、AgentRuntimeRegistry、AgentClient/AgentSession |
| `service` | preset 配置、默认选择、runtime 目录/元数据生命周期、AgentManager、Workspace attention 适配器 |
| `rpc` | 参数解码与校验、业务调用、结果投影和业务错误 |
| `storage` | Agent preset SQLite catalog、不可变 revision/回执、备份升级、Agent runtime JSON registry |

`server-domain` 继续拥有纯 Agent 身份、配置和持久化值类型，不依赖 provider、Tokio、SQL 或传输。
provider 的服务通过端口调用，storage 实现端口；不将 SQLite 或具体 Provider session 引入 domain。
Provider 名称表示能力包的部署边界，不改变 ADR-001 v4 的领域语义，也不把原生 session 当作
Message 树或宿主 Run。ADR-027 的资源所有权与恢复/关闭语义保持不变。

版本化 preset 和 Paseo runtime snapshot 继续是两种不同记录，不在此次重构中合并。
`catalog.sqlite3`、`agents/agents.json`、schema、回执去重和升级备份保持兼容。
Agent JSON adapter 继续复用 metadata 的通用原子文件引擎，Agent 校验和写入权仍归 provider。
binary 的 `OwnedCatalog` 保留 data-dir lease，直到数据库和仍在执行的阻塞任务释放。

AgentManager 随服务迁移，但生产 binary 仍未组装具体 Provider adapter；create/resume/send/cancel、
模型发现及其他未实现能力不因拆包而启用。历史 Agent 目录查询不宣称提供消息时间线。

## Workspace 自动化完整归入 metadata

metadata 拥有 setup status/run、script list/start/stop 五个方法的协议、RPC、服务、端口和快照类型。
本机 `paseo.json` 读取、脚本子进程、setup 线程、端口分配、输出截断与资源回收实现迁入
`server-metadata::local::workspace_automation`，由宿主通过端口注入服务。

metadata 的职责因此扩展为 Project/Workspace 管理和自动化；本机执行与 model/storage 分模块。
仍不依赖任何 workspace crate、Provider、Tokio、HTTP 或 SQL。新增的 Unix libc 依赖仅用于
既有进程实现；没有加入 unsafe。协议消费者间接编译本机实现依赖，沿用纵向能力包的取舍。

setup/script 的快照仍在内存中；Workspace 信任来源等记录仍由原 registry 持久化。
不添加自动恢复脚本或新的状态文件。Worktree 创建后启动 setup 继续由宿主协调，
保持先完成 Worktree/注册、再尝试 setup 的顺序和原失败语义。物理 recovery 仍归 filesystem。

## Workspace state 的窄端口

clear_attention/mark_unread 两个 Workspace 方法的 DTO、RPC、Workspace 校验与批次结果归 metadata。
具体 Agent 筛选、父子关系判断和持久化更新由 provider 的 `AgentWorkspaceAttention` 完成。

metadata 定义 `WorkspaceAttention` 与请求范围的 `WorkspaceAttentionScan`：

- `scan` 在批次开始时只读取一次 Agent 列表；snapshot 内部的记录不暴露给 metadata。
- 对每个 active Workspace，scan 执行 eligible Agent 更新并返回已提交的 Agent ID 和可选错误。
- metadata 保持请求顺序、重复 Workspace、逐项部分成功与聚合结果。
- mark_unread 在 Workspace 校验后委托 provider 选择并更新最新已完成的根 Agent。

端口只交换 Workspace ID、时间、Agent ID 和稳定错误，不暴露 Agent CRUD 或完整 runtime record。
宿主注入同一 Agent registry 的共享句柄，保持已有更新原子性与缓存可见性。
归档/internal 过滤、permission 提醒保留、根 Agent 选择、单调时间和更新时候选复核的既有行为不变。
这是单向的 `provider -> metadata` 依赖，metadata 不读取或写入 `agents.json`。

## 依赖与宿主

| Crate | 允许的直接 workspace 依赖 |
| --- | --- |
| server-domain、server-metadata | 无 |
| server-provider | server-domain、server-metadata |
| server-filesystem | server-metadata |
| server-protocol | server-provider、server-filesystem、server-metadata |
| server-api | server-protocol、server-provider、server-filesystem、server-metadata |
| server-bin | 上述独立 server crates |

删除职责已经迁完的 server-application、server-ports、server-storage、server-workspace，
不保留转发 crate。依赖守卫覆盖 dev/build/optional/target-specific 边，拒绝旧包重新加入。

通用协议负责信封、协商、方法目录和能力包错误到公开错误的映射；业务包不能反向依赖协议。
API 保留 HTTP/WS、鉴权、连接预算、blocking 调度、队列、订阅、任务取消、drain 和跨能力宿主协调。
此次移出的 21 个接口仍通过 API 路由，不改变传输与生命周期所有权。

可协商名称仍为 195 个，已实现 capability 仍为 102 个。公共方法、JSON 字段、错误码、协议版本和
磁盘路径不变。测试迁入所属模块，保留真实 WS/进程/重启验证；新增窄端口批次、故障和依赖回归。
执行结果及覆盖率见 [实施报告](../reports/server-provider-extraction.md)。
