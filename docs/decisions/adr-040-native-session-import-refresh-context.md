# ADR-040：原生 Session 发现、导入、刷新与上下文导出

- 状态：Accepted。
- 日期：2026-09-25。
- 范围：独立 server，延续 ADR-032/039；不连接旧 daemon 的领域 Message 存储。
- 对照：Paseo `2c8e8a826810337492cc5a38bb0bbd705b6fb632` 的 import-sessions、
  Session handlers 和 activity-curator；本机 Codex 0.153.4 app-server schema。

## 所有权

四个规范方法由 `server-provider` 纵向实现：`provider.sessions.recent.list.request`、
`agent.import.request`、`agent.refresh.request`、`agent.fork_context.request`。
协议 DTO、原生历史端口、Codex 适配器、Agent 协调与展示存储均归 provider。
导入需要的 Project/Workspace 开放与恢复继续使用 `server-metadata::Directory`；生产 host
把同一个 Directory 和 creation service 注入 provider worker，不复制目录存储逻辑。
既有 provider → metadata 依赖不变；server-api、server-protocol 不增加业务处理。

## 原生 Session 与导入

原生历史仍由 Provider 所有。Codex `thread/list` 使用 updated_at 倒序和分页游标，排除
ephemeral 与子 Agent thread；服务层筛选 cwd、since、query、Provider 和已导入 handle。
默认返回 20 条，上限 200；搜索扫描最多 500 个候选，无 query 按 limit 加已导入数扫描，
上限 4096。每页最多请求 100 条，并对循环分页游标及过多分页报错。
Provider 故障放进安全的 providerErrors，不回传 stderr 或原生错误正文。

导入通过短生命周期 `thread/read(includeTurns=true)` 校验完整历史、原生 ID、canonical cwd、
时间戳及模型配置。不调用 thread/start、thread/resume 或 turn/start。原生仍在执行或历史
不完整时拒绝导入，校验通过后才允许 metadata 创建 placement。

同一 provider/handle 已有未归档 Agent 时拒绝重复导入；已归档记录复用原 Agent ID，保留用户
标题、配置和非父级标签，清除旧 `paseo.parent-agent-id` 后合入请求标签。新导入保存原生模型、
推理等级和创建时间，执行模式仍限定为 read-only。legacy provider/sessionId 字段保留；
同时给出新旧字段且值不一致时拒绝。

## 刷新与展示历史分代

refresh 先验证 Agent 所属活动 Workspace。若本进程持有运行中的 turn，先取消并等待结束，
再关闭 native writer，通过只读历史重新加载。原生仍报告 active 时拒绝覆盖，成功后取消
Agent 归档并更新 runtime facts。writer 留到下一次发送消息时恢复。

Timeline schema 从 v1 原子迁移到 v2，增加 retired_entries。已有原生条目是新历史的完整
前缀时，仅追加新条目并保留 epoch 和已有 sequence；发现修改、删除或重排时，把整代旧行
保存在 retired_entries，事务内切换 epoch 并建立新的当前投影。插件展示条目保留，在替换
后的原生历史后排列。事务提交后发布 `agent.timeline.replacement`；客户端旧 cursor 失效，
Timeline get 返回 reset/staleCursor，fork_context 拒绝过期边界。

普通 append 仍禁止改写已保存条目。分代操作只适用于 Provider 拥有的完整原生历史刷新；
所有旧展示行留存，不修改 ADR-001 的不可变 Message 树，也不把此表当作领域 Session 容器。
单条超限或 SQLite 错误回滚整次投影切换，不发布部分事件。退休代暂无清理接口。

## 上下文附件

fork_context 遵循 Paseo 的文本附件语义，不创建原生 fork，也不改变 Agent 或 Session 指针。
cursor 优先于 message ID，采用包含边界的选择；message ID 选择最后一个匹配的 assistant
条目。附件只包含 user/assistant 文本及工具名称，不包含原始工具输入、reasoning 或 plugin
payload。文本上限 512 KiB，完整结果仍受 900 KiB 响应预算约束，超限明确失败。

## 限制与失败边界

目前只有 Codex 适配器；没有真实模型调用验证，不承诺其他 Provider 或旧 Codex schema 的
兼容性。recent 的 lastPromptPreview 为空，查询只能在有界候选内搜索。fork_context 的工具
摘要只展示名称，尚未实现 Paseo 的细分工具描述；展示投影仍遵循 ADR-039 的逐条 identity。

Workspace registry、Agent registry、Timeline 之间没有跨存储事务。原生读校验失败不创建
placement；后续存储失败可能留下已创建的 Workspace 或尚未关联 Agent 的展示投影，重试
会复用已有目录。Agent registry 失败时不声称导入完成。refresh 关闭 writer 后读取失败时，
旧投影仍保留，但先前运行状态可能已经变为 closed。连接事件仍按提交顺序发布，不承诺
跨三个存储的统一快照。外部进程在读取后再次修改原生历史的竞态仍依赖 Provider 协调。

refresh 不自动恢复已归档/丢失的 Workspace，不启动 writer；这与 Paseo 的 legacy workspace
恢复及 eager resume 不同。手动刷新补齐外部历史变更，尚无持续文件监控、原生 rewind、
provider subagents 或跨 Provider 导入。

验证及 Test coverage 见[实施报告](../reports/server-native-sessions.md)。
