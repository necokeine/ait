# ADR-030：纵向拆出 server-filesystem

> Tokio 依赖限制已由 [ADR-037](adr-037-server-model-context.md) 修订；filesystem 可按需要使用 Tokio。

> 后续边界更新：[ADR-031](adr-031-server-provider.md) 将 Workspace 自动化迁入 metadata，
> Agent 能力迁入 provider，并删除四个空横向 crate；当前依赖表以 ADR-031 为准。

- 状态：Accepted。
- 日期：2026-09-24。
- 授权：按可行性分析开始拆分 Git、Forge/PR、文件/目录和 skill 能力。
- 范围：独立 server；不修改旧 daemon 和 `ait-*`。
- 更新：ADR-029 中 GitHub provisioning、Worktree/recovery DTO、二进制文件 framing 和相关 crate 依赖的归属。Project/Workspace 模型与存储继续遵循 ADR-029。

## 决策

新增 `server-filesystem`，按能力纵向封装协议、业务和本机执行：

| 模块 | 所有权 |
| --- | --- |
| `protocol` | checkout、Forge/PR、文件/目录、二进制文件帧、Worktree、recovery、GitHub 仓库搜索/clone 的 DTO；5 个 skill 占位方法声明 |
| `ports` | Git、Forge、文件读写/上传、Worktree、recovery 和 GitHub CLI 契约与运行数据 |
| `service` | Checkout/Forge、单次下载授权、连接内上传状态机、分块读取、Worktree/恢复/clone 协调 |
| `rpc` | 参数校验、业务调度、错误/结果投影、文件与 diff 快照比较 |
| `local` | 本机文件系统、Git/gh 子进程、目录检查/创建、上传临时文件与 Worktree 实现 |

48 个已实现方法迁入新 crate，5 个 skill 方法仍为 `not_implemented`。
不新增 skill 安装器、选择持久化或第二个 daemon config writer。
`config.json` 中的 skills 配置继续由 metadata 管理；后续 skill 实现通过配置所有者协调写入。
公共 JSON 方法、字段、二进制帧布局、协议版本、命令预算与磁盘路径保持不变。
服务仍公布 195 个可协商方法、102 个已实现 capability。

## metadata 与 filesystem

依赖单向为 `server-filesystem -> server-metadata`，metadata 不依赖任何 workspace crate。
Project/Workspace 记录、registry、labels、配置、图标与 server-id 文件留在 metadata。
Worktree/recovery/GitHub 使用宿主已初始化的同一组 registry 句柄；不新建独立缓存或复制记录。

GitHub provisioning 从 metadata Directory 分离。Directory 的 Clone 共享端口 adapter；
宿主向 GitHub 服务提供共享这些 adapter 的 Directory，用于 clone 完成后的 Project 注册。
纯 remote identity 解析由 metadata 保留并公开，确保 Project key 与 clone URL 解释一致。
注册失败保留已 clone 的目录和 checkoutPath；创建 Worktree 后注册失败仍尝试既有回滚。
恢复操作保留保存的分支、worktree 根和嵌套目录，再依次解除 Project/Workspace 归档。
Agent attention 留在 `server-application`，不进入 filesystem。

`project.create_directory.request` 的 Project 注册协调仍由 metadata 负责；
`DirectorySource` 是 metadata 消费者定义的端口，其本机 mkdir、检查、空目录回滚实现归 filesystem。
Workspace descriptor 中的 Git/Forge 状态摘要仍是 metadata 的读取模型，不引入反向依赖。

## 宿主边界

`server-api` 保留鉴权、Hello/RPC 信封、连接预算、队列、HTTP/WS 发送、Tokio timer、
任务跟踪、取消与 blocking 调度。每个连接独立持有 filesystem 的 Uploads，禁止跨连接续传。
上传 writer 丢弃时释放未完成临时文件；下载授权仍最多 256 个、有效期一分钟、单次消费。
文件与 diff 观察状态及内容投影归 filesystem，timer 和订阅释放归宿主。
订阅响应入队后才激活观察任务；Worktree/recovery 的 Workspace 事件仍在响应后发送。
HTTP 下载端点、响应头和流调度留在 API，文件读取和最终 revision 校验归 filesystem。

`server-workspace` 保留实际的 setup/script 进程运行实现，不改成空转发 crate。
文件二进制帧及其错误类型由 filesystem 定义；通用协议不再拥有文件业务类型。
通用 `server-protocol` 负责映射 filesystem dispatch 错误和聚合方法目录。
协议消费者会间接编译 filesystem 的本机依赖，这是单 crate 纵向封装的取舍。

## 依赖约束

| Crate | 允许的直接 workspace 依赖 |
| --- | --- |
| server-domain、server-metadata | 无 |
| server-filesystem | server-metadata |
| server-protocol | server-metadata、server-filesystem |
| server-ports | server-domain |
| server-application | server-domain、server-ports、server-metadata |
| server-storage | server-domain、server-ports、server-metadata |
| server-workspace | server-ports |
| server-api | server-application、server-domain、server-protocol、server-metadata、server-filesystem |
| server-bin | 当前独立 server crates |

filesystem 不依赖 Tokio、HTTP、SQL、Agent/domain、application、API 或旧 Ait crate。
内部本机实现依赖 ports；service 通过端口调用，不依赖具体本机 adapter。
依赖守卫检查所有直接边，包括 dev/build/optional/target-specific 依赖。

## 验证与限制

保留真实 Git、假 gh CLI、JSON、二进制文件帧、WebSocket 和进程集成测试。
新增快照去重、分块读取 revision 检查、上传隔离/清理与依赖反向边回归。
本次不扩展多 Forge、skill 执行、Provider、跨进程文件事务或已有路径检查的安全模型。
实际执行和覆盖率见 [实施报告](../reports/server-filesystem-extraction.md)。
