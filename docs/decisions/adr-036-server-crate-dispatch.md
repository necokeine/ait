# ADR-036：请求先进入所属 crate 再分发到能力组

> 后续修订：[ADR-037](adr-037-server-model-context.md) 抽出公共 Context 并删除 Host 回调，
> 能力 crate 直接使用 Tokio 与具体上下文；本文的 crate 优先分发顺序继续有效。

- 状态：Accepted。
- 日期：2026-09-25。
- 授权：用户要求继续简化 dispatch，并在进一步分发前先进入各个 crate。
- 范围：独立 server 请求分发和 API 宿主适配；补充 ADR-035。

## 决策

请求通过 API 的名称、方向和 capability 检查后，只按 metadata、filesystem、provider、terminal
四个 crate 分流，调用各自的 `dispatch::dispatch(group, host)`。能力组的 match 属于该 crate，
API 不再在统一路由函数内展开所有业务组，也不提前截获 Session、Files、Terminal 或 Agent wait。

各 crate 的 `dispatch::Host` 是宿主集成端口。普通阻塞 RPC 由所属 crate 选择具体函数，并通过
类型化 `Operation<Service, Result>` 交给 Host。API 为请求上下文实现 Host，提供实际服务和
既有 `jobs::run` 调度；该公共调用路径移动 method/params，不再为普通 RPC 复制 method。
Agent 完成等待的特殊调度由 provider dispatcher 选择，API 继续跟踪等待任务及其预算。

Host 对需要连接或宿主协调的能力提供窄回调，包括订阅、文件传输、Terminal stream、Daemon
生命周期、Agent runtime 和 Worktree 后续 setup。回调不拥有能力目录，也不维护全局业务路由表。
既有跨能力协调仍由宿主执行：Worktree 成功后尝试 setup、Agent 批量关闭关联 Terminal。

这些端口使用标准库 `Future` 和静态泛型分发，不使用 boxed future 或运行时 handler registry。
metadata/filesystem/terminal 不新增 Tokio、HTTP 或 server-protocol 依赖；API 的具体错误和
WebSocket 类型通过 Host 的关联输出类型留在 API 内。既有 crate 依赖图保持不变。

## 响应与连接所有权

普通结果使用 `Result<Reply, ErrorCode>`。成功 Reply 包含一个响应值和一个后续动作：无动作、
Labels 激活、Checkout 激活、Workspace 事件或初始 Status。API 先将响应入队，成功后才执行
动作；错误没有后续动作，发送失败则丢弃尚未激活的句柄。

文件、Terminal、Session 以及长期等待沿用其连接状态机和专用发送路径。这些路径现在在进入
所属 crate 后由 Host 接入。heartbeat、terminal input 与二进制帧处理仍使用既有的连接入口；
它们没有接入 request 分发或被转换成带响应的请求。

鉴权、协商、占位分类、队列预算、任务跟踪、取消和 drain 仍归 API。纯业务执行仍在各业务
crate 的 RPC/service 内。Message、领域 Session、Run 的定义及终止条件均不改变。

## 验证

保留原方法、消息方向、错误和安装规则；完整能力仍为 122/195。
新增测试验证 crate 内分组选择、Host 错误透传、provider wait 选择、统一 RPC 调用和响应先于
后续事件。完整 WebSocket/进程测试验证实际执行和连接资源释放。
结果、命令和覆盖率见[实施报告](../reports/server-crate-dispatch.md)。
