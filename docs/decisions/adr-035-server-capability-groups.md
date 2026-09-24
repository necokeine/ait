# ADR-035：能力分组与安装规则归所属 server crate

- 状态：Accepted。
- 日期：2026-09-25。
- 授权：用户要求各 crate 独立统计 implemented groups 和 installed capabilities，由 server-api 合并。
- 范围：独立 server 的能力声明、安装条件及路由接线；补充 ADR-029/030/031/033/034。

## 决策

`server-metadata`、`server-filesystem`、`server-provider`、`server-terminal` 各自提供
`capabilities` 模块：

- `Group` 标识本 crate 的方法分组。
- `IMPLEMENTED_GROUPS` 复用本 crate 的 protocol 方法常量，包含已实现的 request 和 event，
  不包含 catalog 占位方法。
- `InstalledServices` 描述宿主实际安装了哪些可选服务；这些独立布尔标记只表示服务存在性。
- `installed_capabilities` 根据本 crate 的规则过滤上述分组，返回静态方法名迭代器。

metadata 拥有基础 server/connection 方法、Session 订阅和 heartbeat 的声明，它们始终安装。
Directory 安装同时启用 Project 配置、图标和 Project/Workspace 目录方法。
provider 的 Agent lifecycle 在 runtime 或 execution 任一服务存在时安装；执行与后续 turn
配置只在 execution 存在时安装，两个服务同时存在也不重复声明 lifecycle。
filesystem 和 terminal 按各自服务是否存在过滤方法组。

`server-api::capabilities` 只把实际服务的存在性传给各 crate，合并其返回值，并使用带 crate
归属的分组枚举连接传输处理器。路由注册与测试复用同一份合并后的 implemented groups，
API 不再逐项列出业务 protocol capability 数组，也不再维护跨服务的安装判断。

## 兼容性与依赖

能力名称、可用条件、消息方向和占位错误语义保持不变。完整安装仍为 122 个已实现方法、
195 个可协商方法；空宿主仍为 6 个已实现方法、190 个可协商方法。
合并后数组按 crate 分组排列；能力协商按名称判断，数组顺序不作为优先级。

Paseo catalog 继续由 server-protocol 维护，已有方法的消息方向来自 catalog，额外方法默认
为 request。`server.status.unsubscribe` 保留 API 内部兼容路由，继续复用
`server.status.subscribe` 的 capability，不新增已公布的方法名称。

server-protocol 对 metadata 基础协议的既有导出保持不变。能力包不引用 server-api 或
server-protocol；不新增 crate 依赖。分组声明不转移执行、响应发送、订阅激活、任务跟踪、
取消或 drain 的所有权，也不修改 ADR-001 v4 的领域边界。

## 验证

各能力 crate 检查所有服务安装组合、方法唯一性和完整安装覆盖；API 检查合并后的唯一性、
基础能力、占位目录与路由方向。真实 WebSocket 和进程回归继续验证协商和执行接线。
命令、结果及覆盖率见[实施报告](../reports/server-capability-groups.md)。
