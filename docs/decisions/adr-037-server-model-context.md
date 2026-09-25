# ADR-037：公共 Context 与具体 crate 分发

- 状态：Accepted。
- 日期：2026-09-25。
- 授权：用户要求直接使用 Tokio 简化分发，随后明确要求把 Context 放入公共结构层，删除因
  Context 定义在 server-api 而引入的 Host trait。
- 范围：独立 server；修订 ADR-036 的 Host 边界及 ADR-029、ADR-030、ADR-031、ADR-033
  对能力 crate 的 Tokio 依赖限制。不改变 Message、Session、Run 领域语义。

## 公共结构与依赖方向

新增 `server-model`，只依赖 Serde、thiserror、Tokio 和 Tokio-util，不依赖任何 workspace
crate，也不依赖 HTTP/WebSocket 库。它拥有：

- `Request` 与具体 `Context`：请求 ID、method、params、Runtime、Outbound 和连接剩余订阅额度。
- `Runtime`：公共 ServerInfo、准入锁、任务追踪、取消、生命周期意图与既有并发预算。
- `Outbound`：有界消息/字节队列和 Text/Binary 数据帧；持有字节 permit 直到 socket 写完成。
- 公共响应信封、稳定错误码、版本/预算/ServerInfo 与订阅释放 DTO。

Context 不携带 API Shared，也不聚合所有业务服务与连接状态。每个能力 crate 以自己的具体
`dispatch::State` 持有服务，以自己的 connection 类型持有观察者、presence 或上传状态。
这些 State 共享同一个 `Arc<Runtime>`；分发接收具体 Context/State/connection，不使用 Host、
async-trait、boxed future、Any/TypeMap、动态 handler registry 或回调路由。

| Crate | 允许的直接 workspace 依赖 |
| --- | --- |
| server-model、server-domain | 无 |
| server-metadata | server-model |
| server-filesystem、server-terminal | server-model、server-metadata |
| server-provider | server-model、server-domain、server-metadata |
| server-protocol | server-model 与四个能力 crate |
| server-api | server-model、server-protocol 与四个能力 crate |
| server-bin | 当前独立 server crates |

metadata/filesystem/provider/terminal 可以直接使用 Tokio。server-model 的 Runtime 属于运行时
基础结构，并非纯领域模型；domain 保持无 Tokio，protocol 不直接使用 Tokio。依赖守卫覆盖
普通、dev、build、optional 和 target-specific 依赖，并拒绝 model 对业务或 API 的反向边。

## 分发与所有权

API 完成协商、方向和 capability 检查后，先进入所属 crate 的 dispatcher，再由 crate 选择组
和具体业务执行。原 API 内的 Session/labels/daemon、checkout/file、Agent runtime/execution、
Terminal 连接逻辑迁入对应 crate。API 保留 HTTP/WS、认证、握手、物理连接循环与装配。
文件 HTTP 下载入口仍在 API，文件分块读取和二进制预览在 filesystem。

各 crate 直接在 Tokio 上执行。普通阻塞任务通过公共 Runtime 接纳，移入 blocking 池后继续
持有任务跟踪和 permit，即使响应 future 被丢弃也必须 drain。Agent 完成等待继续由 provider
接纳并追踪；Terminal 保留独立阻塞预算和后台 reconciliation。

需要跨能力收尾时返回明确的数据结果：metadata 请求释放某个订阅，API 依次释放四类连接
状态后回应；provider 完成 Agent 关闭后返回 Terminal IDs，API 调用 terminal 完成关闭并发送
合并响应。没有把宿主函数包装成 trait 或闭包传回能力 crate。provider 在执行 Agent 关闭前
仍检查 Terminal 能力是否安装。

普通结果/错误发送由 Context/Outbound 统一处理；Workspace update、标签/checkout 订阅激活
和初始 status 继续在响应成功入队后发生。连接剩余额度在 API 按全部订阅种类计算，具体 crate
负责同目标订阅替换规则。通用 release 幂等地移除各类观察者。原错误、JSON 字段、方法和
能力安装规则保持兼容。

公共类型由 metadata/protocol 旧路径重导出，业务错误转换移到定义业务错误的 crate，避免
model 引用具体服务。API 将公共 Frame 转换成 axum WebSocket Message；model 不依赖 axum。

## 验证

迁移已有队列和公共 RPC 测试到 model；新增已开始的阻塞任务在响应取消后仍保留预算和追踪
的回归。原 WebSocket/进程测试验证订阅额度与释放、事件顺序、跨能力关闭、二进制传输和
shutdown drain。命令、覆盖率和限制见[实施报告](../reports/server-model-context.md)。
