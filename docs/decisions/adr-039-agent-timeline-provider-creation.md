# ADR-039：Agent Timeline、Provider 发现与创建过程订阅

- 状态：Accepted。
- 日期：2026-09-25。
- 范围：独立 server；延续 ADR-032/034/037/038，不连接旧 daemon 的领域存储。
- 基线：Paseo `2c8e8a826810337492cc5a38bb0bbd705b6fb632` 的协议、Timeline 查询、
  Provider catalog、creation service 和 plugin session identity；本机 Codex 0.153.4 导出的协议 schema。

## 边界

`server-provider` 拥有 Timeline DTO、查询、SQLite 展示投影、原生 Codex 历史适配、Provider
发现缓存与 Timeline 连接观察者。`server-metadata` 拥有 Agent/Workspace 共用的创建回执、
幂等意图、进度快照和创建订阅。公共 `server-model::events` 只提供有界、暂停后激活的传输
观察者，不包含业务枚举。API 仍仅承担鉴权、协商、预算、分流和断线释放。

依赖图不变。server-protocol 的目录只登记方法字符串，不重新依赖能力 crate。
server-domain 不增加 Tokio、存储或 Provider 依赖。

## Timeline 与原生历史

新增 `agent.timeline.{get,search,list_prompts,append,set_subscription}.request`。
原生 thread/read 在独立短生命周期 app-server 进程中读取 includeTurns=true 的历史，
不会通过 thread/resume 抢占 writer。执行过程中只持久化 item/completed 的完整条目；
不把 token delta 当作不可变消息。原生工具以 Paseo 的 generic tool detail 保留展示数据。

`agents/timeline.sqlite3` 是 append-only 展示投影，不是 ADR-001 的 Message 树或 Session 容器。
Codex 仍拥有原生历史；展示投影不可用于重新构造或修改 Provider prompt。插件 append 只接受
plugin 展示条目，同样不进入模型上下文。重复 source identity 返回原 sequence，内容不同则
拒绝；数据库提交后才推送 agent_stream。失败事务不发布条目，持久化失败的原生事件留待重试。

每个 Agent 的 epoch 和 sequence 持久化，支持 tail/before/after、分页、过期游标重置、搜索
位置和 user prompt 索引。当前 projected 是逐条 identity 投影，尚不合并相邻 assistant/reasoning
条目，也不更新已有 tool lifecycle 条目。搜索对原始消息作大小写和空白归一化，不包含 Paseo
Markdown 渲染后的补充匹配。native RPC 保持 2 MiB 行预算，单个展示条目 256 KiB、插件 data
64 KiB，超限明确报错；不静默截断历史。

Timeline subscription 独立拥有 release ID，Agent 选择经过 registry 解析；连接断开、release
和 server drain 释放观察者。响应入队后才激活暂停期间的事件；慢客户端或溢出关闭连接，
客户端通过持久化游标补读。

插件 provenance 沿用 Paseo 的 `plugin:<id>` client label 约定；本 server 的所有调用仍需要
同一完整权限 token。client_id 不是独立鉴权身份，不能据此提供多租户隔离，也不声称已经
实现 Plugin 安装/宿主体系。普通 client label 的 append 被拒绝，payload 不能自带 pluginId。

## Provider catalog

新增 available/models/modes/features 列表和 snapshot get/refresh 共六个方法。与执行器共用
注册的 AgentClient，Codex 模型来自真实 model/list（分页、去重、循环游标防护），不硬编码
模型清单。只发布适配器确实支持的 read-only 模式；没有实现的 feature 返回空列表。
不存在的 Provider 明确拒绝，启动或发现失败只发布安全错误，不回传 native stderr。

按 canonical cwd 缓存，最多十六个目录，六十秒过期；显式 refresh 刷新指定 Provider 并发布
providers_snapshot_update，Session 事件订阅增加这个生产者。快照支持稳定内容 hash 和
ifNoneMatch；完整 entries 已接通，compactSnapshot 编码仍不提供。refresh 当前同步等待发现
完成再返回 acknowledged；诊断、用量与 recent sessions 仍为占位。

## 创建回执

新增 `creation.subscribe.request`，并接通已有 Agent/目录来源 Workspace 创建的 idempotencyKey
与 subscribe 参数。metadata 的 `creations/receipts.json` 原子存储意图和进度，资源 ID 在执行
前保留。相同 kind/key/intent 重放既有结果，参数不同报 idempotency_conflict。观察者可先于
创建注册；subscribe=false 仅读快照。未显式提供 key 时生成唯一 key，并在 creation 快照返回，
避免把连接局部 request_id 当作全局幂等键。

Agent 创建按 accepted → agent_ready → 可选 prompt_started → completed 推进；initialPrompt
通过原生 text turn 提交，creation.completed 只表示资源注册及首条 prompt 接纳完成，
不表示 Agent turn 或宿主 Run 已完成。Workspace 的 directory source 按 accepted →
workspace_ready → completed 推进；组合 Workspace+Agent/worktree 创建仍明确拒绝。

已完成回执跨重启可读。重启发现非终态回执时返回 failed/outcomeUnknown，保留原先预留的
资源 ID，不自动重放可能已执行的原生创建。已登记 Agent 可以正常读取/恢复；原生 session
已创建而 registry/回执尚未提交的跨系统原子性不在保证内，不能假装失败等于没有副作用。

实现、测试结果、覆盖率与剩余限制见 [实施报告](../reports/server-agent-timeline-provider-creation.md)。
