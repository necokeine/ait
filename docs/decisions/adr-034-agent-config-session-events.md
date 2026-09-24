# ADR-034：Agent 后续 turn 配置与 Session 事件/心跳

- 状态：Accepted。
- 日期：2026-09-24。
- 授权：继续实现独立 server 的 Agent 和 Session 接口，参考固定 Paseo 版本。
- 更新：ADR-032 的配置边界，以及 ADR-029 的 heartbeat 占位状态。

## Agent 配置

`server-provider` 增加 `agent.model.set.request`、`agent.thinking.set.request` 与
`agent.config.apply.request`。协议、校验、Provider 能力校验、worker 分发和持久化协调都归 provider。
配置原子更新现有 Agent JSON record，不引入新文件或第二个配置 writer。

单字段方法要求显式提供 `modelId` / `thinkingOptionId`，允许 null。批量方法的 `config` 支持
这两个字段：省略保持原值，null 清除宿主覆盖并把 null 交给原生 Provider，空对象是有效空修改。
model 为非空、不含控制字符、最多 256 UTF-8 字节的字符串；Codex thinking 使用当前本机 schema
中的 none/minimal/low/medium/high/xhigh。模式、feature 等批量字段明确返回 unsupported_capability。

校验后，两个字段与 updatedAt 一次提交；写失败不改变原配置，也不影响已接纳的 native turn。
活动 turn 期间允许配置下一轮，响应附带 applies-next-turn notice，不取消或重启当前 turn。
AgentSession.start_turn 显式接收这一轮冻结的配置，而不是读取可变的全局配置。模型/推理参数随
turn/start 交给 Codex；注册时缓存的旧配置不再覆盖后续修改。原生接纳后的 runtime facts 通过
pending-runtime 写入重试保存，写失败不能把已接纳的 turn 错报为未接纳。

归档 Agent 不接受配置修改。模型是否存在和当前认证能否使用模型仍由原生执行决定；accepted
表示配置已保存，不表示已调用模型验证。只读 sandbox 和 approvalPolicy=never 保持不变。
这些设置属于 Paseo native Agent，不修改 ADR-001 的 Agent revision、Message 树或 Run 不变量。

## Session 所有权

这里的 Session 是连接事件/活动协议，不是 ADR-001 的可移动 Message 引用。
`server-metadata::protocol::session` 拥有 DTO 和已支持的事件类别；`service::session` 拥有
进程内 presence、订阅准备/激活/释放、通知选择。metadata 不依赖 Tokio、API 或 provider。
provider 通过现有单向依赖发布已提交的 attention 事件；API 注入有界 outbound callback。

API 拥有真实 WS 连接、鉴权、协商、连接身份、共享 16 个订阅的配额、drain 和断连生命周期。
订阅响应入队后才激活，之前的消息最多缓存 64 条 / 1 MiB；超限使该连接关闭，不回滚业务提交。
每次 set_subscription 都创建一个新 owner，通用 subscription.release.request 仅释放本连接的 ID。
断连释放全部 owner 和 presence。事件不持久化、不回放，也不暗示目录或 timeline bootstrap。

## 已接通的事件

`session.events.set_subscription.request` 支持三类真正接有 producer 的事件：

- `agent_attention_required`：已提交的 native 完成/失败；取消、内部 Agent、归档和已删除记录不通知。
  terminal registry 写失败时保留 pending event，成功后才发布一次。事件包含 agentId、reason、
  timestamp、shouldNotify 与 subscriptionId；不携带模型输出或 credentials。
- `status.daemon_config_changed`：成功的 config set/reload，使用既有公开配置投影。
- `status.server_info`：server 进入 draining 的公开状态；无初始快照，客户端用 server.info 读取。

请求其他 Paseo 事件类别返回 unsupported_capability，避免声称能够交付尚未存在的 producer。
未安装 Agent worker / daemon 服务的 API 组合也拒绝订阅相应类别。

## 心跳与通知

`session.heartbeat` 是无应答的已协商客户端事件，解析 Paseo deviceType、focusedAgentId、
focusedTerminalId、lastActivityAt、appVisible 与可选 appVisibilityChangedAt。时间必须是 RFC3339；
未来 activity 在接收时截断，presence 有效期为 180 秒。心跳不产生磁盘写入、不改变 Run、
不充当 WebSocket Ping/Pong，也不自动清除持久化 attention。

所有订阅者均获得 attention 状态。任一新鲜且前台可见的连接正在看目标 Agent 时，shouldNotify
全部为 false；否则从订阅了该事件、启用 notifications 且 presence 新鲜的连接中选择最近活动者。
同一连接多个订阅最多一条事件的 shouldNotify 为 true。无心跳、过期、释放或断开的连接不会获选。
没有 push 投递、notification 文案生成或 terminal focus 清除；terminal 心跳字段仅兼容解析。

## 参考与验证

参考 getpaseo/paseo@2c8e8a826810337492cc5a38bb0bbd705b6fb632：messages.ts、
session.ts、session/agent-config/agent-config-session.ts、agent-attention-policy.ts、
agent/providers/codex-app-server-agent.ts；对照本机 codex-cli 0.153.4 导出的 TurnStartParams schema。

本轮增加五个接口；Agent 由 14/32 到 17/32，Session 由 2/5 到 4/5。
creation.subscribe 需要创建回执；timeline、流式、import/refresh、权限与模式/feature 仍待各自实现。
结果、覆盖率、并行 Terminal 改动的验证范围记录在 [实施报告](../reports/server-agent-session.md)。
