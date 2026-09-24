# ADR-032：独立 server 接通 Codex 原生文本执行

- 状态：Accepted。
- 日期：2026-09-24。
- 范围：独立 server；延续 ADR-027/031，不依赖旧 `ait-*`。
- 来源：`getpaseo/paseo@2c8e8a826810337492cc5a38bb0bbd705b6fb632` 的
  `agent-sdk-types.ts`、`agent-manager.ts`、`agent-loading.ts`、
  `providers/codex-app-server-agent.ts`、`providers/codex/app-server-transport.ts` 与
  `packages/protocol/src/messages.ts`；另核对本机 Codex 0.153.4 导出的 JSON schema。

## 原生资源与宿主边界

`server-provider::local::codex` 实现 AgentClient/AgentSession，直接启动 `codex app-server`，
经过 initialize/initialized 后创建 thread。恢复已有 Agent 使用已登记的 provider/session ID；
交互恢复调用 thread/resume，归档恢复只调用 thread/read，读取后立即关闭进程。
runtime facts 与 persistence handle 一致且成功落盘，Agent 才成为可用的 live session。

Provider 自己的历史文件继续由 Codex 管理；本轮不导入、改写或声称实现 ADR-001 的 Message 树、
Session ref 或宿主 Run。AgentSession 的 native turn 结束只驱动 Paseo runtime snapshot 的 idle/error，
不是领域 Run.completed。重试、压缩和工具循环由原生 Codex turn 执行；宿主没有另建输入队列。

`server-provider::service::agent_execution` 在专用线程上运行 Tokio current-thread runtime，拥有
AgentManager、registry 和 Agent metadata directory。阻塞 registry I/O 留在这个线程，HTTP/WS
reactor 只交换有界命令。API 继续拥有鉴权、协商、连接与响应队列、wait 请求接纳和 drain。
provider 允许 Tokio；server-domain 与 metadata 的纯依赖约束不变，crate 图仍是 ADR-031 的七个包。

同一 worker 串行处理 runtime metadata 与 native lifecycle；archive/delete 后，即使目录操作部分
失败，也关闭已归档或删除记录的 writer。resume、terminal 和 close 使用 registry.update 合并最新
记录，保留其他入口提交的 title/labels/attention，关闭不会重新插入已删除的 Agent。

## 已实现的最小闭环

新增五个 canonical request：agent.create、agent.resume、agent.message.send、agent.cancel、
agent.finish.wait（均带 `.request` 后缀）。生产 host 的 implemented capabilities 从 102 增至 107，
可协商名称仍为 195，规范方法仍为 188，剩余占位 88。

- create 接收 provider/cwd、可选 UUID、title、model、thinking、systemPrompt、labels 和 workspaceId。
  本阶段要求已有活动 Workspace；未给 workspaceId 时选择相同规范路径的最早活动 Workspace，
  并校验其 Project 活动。客户端可以先调用 workspace.open.request。
- resume 按 handle 查找本 server 已登记记录，不导入任意原生 thread、不改变 Agent ID；归档记录
  只能读历史身份，不允许发送消息。恢复前无 handle 的记录仍报错。
- send 接收非空纯文本（最多 64 KiB），先落盘 running 状态，再启动原生 turn；响应表示接纳。
  当前 Agent 忙时明确拒绝第二条输入，不静默丢弃、不把排队工作误报完成。
- cancel 发送 turn/interrupt，只有 provider 的 terminal event 被持久化后，wait 才观察到空闲。
  原生 completion 与 interrupt 的竞争不会因一条 RPC rejection 自动损坏连接。
- wait 返回 idle/error/timeout、当前 snapshot 和本进程最新完成 turn 的最后 assistant 文本。
  lastMessage 是临时结果缓存，重启后不冒充已加载的 timeline。持久化失败保留 terminal event 重试，
  不提前公布完成。仅有 running durable snapshot 而没有 live writer 时不能报告成功。

finish.wait 作为独立、受预算限制的传输任务，不占用 WebSocket 收包循环或 Provider 命令槽；
同一连接可以继续发 cancel。其他接纳的 native 工作不因客户端断开而取消。

## 明确的能力限制

本轮只接 Codex 的 read-only 模式：显式指定 sandbox=read-only、approvalPolicy=never，避免继承
用户本地更宽的权限配置。其他模式、permission/user-input 交互、MCP 配置、providerOptions、
附件/图片、initialPrompt、messageId 去重、idempotencyKey、activeTurnBehavior、创建时 worktree/
Git/env/subscribe、resume overrides 均未接通；带这些参数返回明确错误，不声称已应用。
原生主动请求交互时返回 unsupported RPC error 并关闭 session，绝不自动批准。

timeline、流式事件、模型/Provider catalog、原生历史导入、队列/steer、rewind、动态配置和其他
Provider 保持占位。只返回最后的完成文本，不将 token/tool 中间事件伪装成完整会话历史。
closed/stored snapshot 继续保守报告 providerUnavailable，成功恢复后才报告 live 可用性。

## 预算与回收

最多 32 个 live session、64 条 worker 待处理命令、32 个并发 wait；wait 默认/最大 30 秒，
超时只结束观察，不取消 turn。单个 native RPC 默认 10 秒，执行中的 turn 没有固定时限。
native JSON 行限制 2 MiB，通知队列有界；中间 token/tool 通知不保留。原生 stderr 不进入服务日志。

正常停机关闭 WebSocket 接纳，等待传输任务，排空已接纳 Provider 命令，再 close_all 并 join worker。
worker 持有 data-dir lease，直到 native runtime 释放。Unix 启动独立进程组，关闭时终止进程组并
wait 回收直接子进程；异常析构保留兜底终止。Windows 仅直接子进程回收路径，尚未验证进程树语义。
使用 AIT_SERVER_CODEX_BIN 可指定可执行文件，否则从 PATH 找 codex；不通过 shell 拼接命令。
鉴权仍由 Codex 自己的环境和存储负责，server 不读取或保存 provider token。
宿主仍沿用 15 秒停机总预算；原生 RPC、排队命令或进程回收超过这个预算时显式报告停机失败，
不宣称所有工作已完成。强制进程退出、跨进程 writer 争用与原生历史对账不在本阶段的恢复保证内。

实现与测试证据见 [报告](../reports/server-native-provider-execution.md)。
