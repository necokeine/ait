# ADR-081：由状态变更唤醒 Workspace 与 Agent 目录订阅

状态：Accepted
日期：2026-10-04

## 背景

ADR-068 的 Workspace 目录订阅每 250 ms 重读一次完整投影。Agent 目录也使用相同
轮询。客户端侧边栏同时消费 Workspace 和 Agent 状态；完成事件已持久化时，订阅读取仍可能
排在其他阻塞任务之后，蓝色运行标记因此延迟消失。每个订阅的快速读取还反复访问 registry
和运行时缓存。Paseo 在 Agent 状态变化后直接唤醒相应 Workspace 更新生产者。

对照 Paseo 的 session event 清单，Ait 已有的项目、脚本、设置和活动功能还缺少
`project.update`、`script_status_update`、`workspace_setup_progress`、`activity_log` 的
部分生产与订阅链路。Hub、Chat、Loop 和 Plugin 属于 ADR-045、ADR-047 已移除的能力。

## 决策

- `model::changes::Changes` 是仅传递唤醒的共享信号，不携带业务事实。Project、
  Workspace 和 Agent registry 在成功持久化后通知；Terminal 活动状态、Git/Forge 缓存
  更新后通知。`metadata` 和 `provider` 目录订阅重读各自的投影并沿用已有差分与顺序号。
- 目录订阅在注册唤醒后先读取一次，随后按变更读取；每 5 秒做一次低频校验，覆盖
  外部进程写入、没有本进程通知的状态。已接纳的订阅读取等待共享任务额度，避免忙时丢失
  唯一一次唤醒。旧的未安装信号的组合仍保留原订阅行为。
- Project registry 的提交后观察者发送 `project.update`。脚本启动、停止及结束发送
  `script_status_update`；脚本自然退出至多每 5 秒检查一次。设置运行时的启动、命令进展、
  完成和失败发送 `workspace_setup_progress`；不可信来源的 Workspace 注册后推送 blocked
  状态。Agent 失败发送 `activity_log` 错误事件。
  这些事件仍由连接级订阅选择和有界队列投递；没有观察者时不发送给客户端。
- 原生 Provider 对子进程消息的 25 ms 内部检查独立于目录订阅。它负责获得原生完成事件；
  完成事件持久化后立即唤醒目录投影，不再等待 250 ms 目录轮询。

## Paseo 事件核对

以 Paseo `SessionEventSubscriptionSchema` 和各事件生产点为对照，Ait 的连接级事件如下。
资源级的 `workspace.update`、`agent.update`、Agent timeline、Terminal stream、checkout
diff、创建回执与标签订阅继续由各自的订阅接口负责。

| Paseo 事件 | Ait 状态 |
| --- | --- |
| `project.update` | 本决策补入 Project 提交后的生产者 |
| `providers_snapshot_update` | 已有 Provider 刷新生产者 |
| `agent_attention_required` | 已有 Agent 完成、失败和审批生产者 |
| `agent_permission_request`、`agent_permission_resolved` | 已有原生审批生产者 |
| `checkout_status_update` | ADR-069 已有 Git fetch 生产者 |
| `script_status_update` | 本决策补入脚本启动、停止、结束生产者 |
| `workspace_setup_progress` | 本决策补入设置进度生产者 |
| `agent.provider_subagents.update` | 已有原生子 Agent 生产者 |
| `terminal_attention_required` | 已有 Terminal 活动生产者 |
| `status.server_info`、`status.daemon_config_changed` | 已有服务状态生产者 |
| `activity_log` | 本决策补入 Agent 失败事件；其余操作仍以各自的错误响应报告 |
| `status.plugin_catalog_changed`、`status.plugin_settings_changed` | Plugin 已由 ADR-047 移除 |
| `hub.execution.agent.update`、`hub.execution.agent.stream` | Hub 已由 ADR-045 移除 |

## 后果与验证

Workspace 与 Agent 目录的通常更新频率由实际变更决定，空闲连接不再每 250 ms
重读目录。单次唤醒可能合并多次变更，目录投影保留最终状态；订阅初始响应、断线重连和
低频校验保留恢复路径。外部写入和脚本自然退出最多受 5 秒校验间隔影响。

本决策修订 ADR-068 关于目录订阅频率的描述。Project、脚本、设置与活动事件的
Paseo 对照范围限于 Ait 已安装的能力；已移除的 Hub、Chat、Loop、Plugin 不重新安装。
