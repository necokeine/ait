# ADR-041：Agent 原生控制与 Provider 诊断、用量

- 状态：Accepted。
- 日期：2026-09-25。
- 范围：独立 server，延续 ADR-032/034/039/040。
- 对照：固定 Paseo `2c8e8a826810337492cc5a38bb0bbd705b6fb632` 的协议、Codex rewind、
  permission、commands、provider catalog；本机 Codex 0.153.4 app-server schema。

## 所有权

剩余七个 Agent 方法和两个 Provider 方法由 server-provider 纵向实现：协议 DTO、Provider
端口、Codex stdio 适配、串行协调、配置和展示持久化均归该 crate。server-api 继续只合并
能力并分流；server-metadata 保留 Workspace placement 和 Session attention 事件所有权。
不增加 crate 依赖，不修改 ADR-001 v4 的领域 Message、Session 或 Run 语义。

## 模式、feature 与命令

`agent.mode.set.request` 支持 read-only、auto、full-access，依次映射为 Codex
never/read-only、on-request/workspace-write、never/danger-full-access。默认仍是 read-only。
选项原子写入 Agent 配置，运行中的 turn 不修改，下一次 turn/start 明确发送审批策略与
sandboxPolicy。`agent.config.apply.request` 同时支持 modeId、featureValues；模型、推理等级
沿用三态 patch，modeId 不接受 null，featureValues 必须是对象并合入原有选择。

`agent.feature.set.request` 目前仅支持布尔 fast_mode。开启前通过 model/list 校验选定或
默认模型的 fast service tier；不支持或校验失败时整组配置不提交。后续 turn/start 使用
serviceTier fast，关闭时显式 null。模型目录增加 supportsFastMode 元数据。
Codex 0.153.4 的 TurnStart schema 没有 Paseo 使用的 plan-mode 参数，因此不提供 plan_mode。

`agent.commands.list.request` 从原生 skills/list 返回当前 cwd 启用的 skills；已有 Agent
使用其配置，未注册草稿可通过 draftConfig 提供 provider/cwd。发送 `/skill 参数` 时使用
原生 skill 输入并附加文本参数。没有实现的 compact/goals/custom prompts 不出现在列表里；
这不等于接通另一个协议分组的 `agent.skills.*` 安装/选择管理接口。

## 临时审批与连接恢复

原生 server request 的命令执行审批、文件变更审批及阻塞式 requestUserInput 转成临时
permission 请求。原生 thread/turn 必须匹配当前 Agent，原生请求 ID 在存续期唯一；宿主
另发 UUID，避免重启后复用原生 ID 导致误答。每个 Session 最多 32 个待审批项，输入限
64 KiB；native frame、队列及请求超时继续沿用既有预算。

`agent.permission.resolve.request` 仅处理该 Agent 当前待审批 UUID；支持 allow once、deny，
问题答案通过 updatedInput.answers 校验问题 ID 和值后转换为原生格式。拒绝持久 policy
amendment、工具输入改写及无法兑现的会话级 grantRoot 授权。未知原生交互继续关闭连接并
报告失败。拒绝后的可选 interrupt 与完成竞态不撤销已经发送的拒绝响应。

审批请求不进入 SQLite Timeline 或 Message history。待审批快照通过 agent.get 恢复，
agent_stream 推送 permission_requested/resolved，metadata 发布 permission attention。
客户端断线不取消原生请求；完成、取消或关闭后 UUID 失效。进程启动时只清理已注册
Provider、带原生 handle 且不再有 live session 的残留 permission attention，不改动无原生
handle 的其他元数据记录。原生回答写入失败时关闭 Session；回答成功后 Agent JSON 更新
失败无法撤回回答，后续 terminal/close 会清理状态，不承诺跨进程 exactly-once。

## 回退与恢复

`agent.rewind.request` 仅支持 conversation；files/both 返回显式失败。以原生 user message
为排他边界，移除目标所在 turn 及后续 turn。先停止本进程 writer，再 fork 原生 thread；
目标非首 turn 时使用 lastTurnId 指向前一个 turn，首 turn 则 fork 后仅对新 thread 执行
rollback。检查新 ID、目录、空闲状态和完整前缀，禁止回写源 thread。

通过 Agent persistence 指针切换到新 thread，旧原生历史保留。JSON registry 与 SQLite
没有共同事务，因此先持久化新 handle 和 aitTimelineReplacement 标记，再按 ADR-040
原子替换展示代，最后清除标记。中途失败不声称成功；Timeline 查询、resume 或下次发送
会先完成该标记的恢复，再允许执行。恢复保留后来发生的 Agent 归档。投影旧代仍保存在
retired_entries，插件条目保留，客户端旧 cursor 收到 reset/staleCursor。

原生 fork 成功而 registry 写入失败可能留下无宿主引用的 thread，但源历史和旧指针仍保留。
原生指针切换后故障可能暂时出现新 handle 配旧投影；标记防止恢复时误用普通 append。
本宿主不锁住外部 Codex 进程，对源历史的跨进程并发修改仍由 Provider 协调。

## 原生子 Agent 与 Provider 检查

`agent.provider_subagents.list.request` 分页读取 subAgentThreadSpawn，根据原生父 ID 建图，
只返回指定宿主 Agent 的原生后代，并保留直接/间接父关系。上限 4096 个候选、64 个分页
游标，拒绝循环游标、冲突父关系及可达环。原生子 Agent 不注册成另一个宿主 Agent。

`agent.provider_subagents.timeline.get.request` 先验证后代关系，再读取并复核直接父 ID、
原生 ID 和目录；展示缓存使用宿主父 ID + 原生子 ID 的独立键。查询复用持久 epoch/cursor
规则并转换为 Paseo rows。当前只包含已完成/中断/失败 turn 的完整条目，没有持续的原生
子 Agent 更新订阅；idle/notLoaded 按 completed 展示，不能进一步区分所有终止原因。

`provider.diagnostic.request` 检查可执行文件、app-server 协议和登录类型，不输出账号邮件、
凭证或原生 stderr。`provider.usage.list.request` 使用 account/rateLimits/read 的原生额度
窗口和计划信息，保留各 bucket、used/remaining 百分比和重置时间；无窗口或不可用时明确
返回 unavailable，不用本地 token 估算冒充账户额度。不实现余额换算、额外计费 API 或缓存。

以上当前只有 Codex 适配器。协议接通不代表其他 Provider 或 Paseo 所有可选行为均已实现。
验证和 Test coverage 见[实施报告](../reports/server-agent-controls.md)。
