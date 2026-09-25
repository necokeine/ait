# ADR-046：Codex 增量显示历史与运行中追加输入

- 状态：Accepted
- 日期：2026-09-25
- 延续：ADR-039、ADR-040、ADR-041；遵循 ADR-001 v4
- 范围：独立 server 的 `server-provider`，不连接旧 daemon 的领域 Run/Message

## 决策

1. `AgentSession` 输出有独立观察 ID 的 `Progress`，以及完整原生 `Timeline` 项。Codex adapter 只接受当前 thread/turn 的 v2 通知，转换 assistant 文本、reasoning summary、工具 started/output；忽略旧版重复事件。工具运行中输出是最多 16 KiB 的 UTF-8 尾部预览，完整结果仍以原生 completed item 为准。
2. Timeline SQLite 升到 v3，增加 append-only `progress` 表。增量与完整项共享单调序号和 epoch，先提交，再发布 `agent_stream`。完整原生项仍保存在 `entries`；对客户端及游标读取投影为未发送的文本尾部，工具按 callId 发布完整状态快照。每个观察 ID 可安全重试，完整项不能覆盖或接收后到的增量。
3. 断线客户端通过现有订阅与 `agent.timeline.get.request` 游标补齐增量；重启后加载原生历史不会重复追加完整正文。搜索按原生项拼接文本，支持匹配跨片段内容。刷新发现原生改写或未完成片段与恢复历史不一致时，原子保留旧代的完整项和增量，再更换 epoch。
4. `agent.message.send.request` 接受显式 `activeTurnBehavior: "steer"`。活动 Agent 调用 native `turn/steer`，携带 `expectedTurnId`，且回执必须匹配；空闲 Agent 沿用普通 start。未提供该字段仍报忙，保留语音调用独占一轮的语义。已知 pending permission 与 slash command 不接受 steer；没有暗中撤销审批或中断权限等待。
5. Native 明确拒绝不终止原 turn；超时、断连、错误回执等不确定结果关闭失败会话，绝不自动 start/retry。同一 turn 的 `latest_turn` 不变，语音取消的 turn 归属检查继续有效。已接收 steer 的活动时间通过待提交状态重试落盘，元数据写失败不能把已接收输入误报为可重发。
6. 完成、失败、取消仅在状态持久化后发布对应 turn 终态事件。原生正文不延续已提交增量或超过预算时失败关闭；I/O 写失败保留待提交项。`supportsStreaming`、`supportsReasoningStream` 只由 Codex adapter 声明。

## 边界与限制

进度是可重放的显示投影，不是可修改的领域 Message。没有创建领域 Run，也没有把任意原生 turn 结束解释为领域 Run 的完成屏障。

不新增 RPC 方法，不改变静态接口总数。此次只支持显式 steer，不实现默认 interrupt、队列、客户端 messageId 幂等、多模态或审批中的自动 steering。原始 reasoning text、旧版 exec delta、token usage、plan/MCP/tool policy 的完整映射继续作为后续工作。极长流受现有帧、项目大小、订阅背压及本轮 192 KiB 单项增量文本预算约束。

## 验证

见 [实施与 Test coverage 报告](../reports/server-codex-streaming.md)。离线 app-server fixture 覆盖实时输出、错误 turn、原生拒绝、完成竞态与不确定接收；真实 server WebSocket 验证游标、断线订阅和进程重启。SQLite 测试验证 v1/v2 迁移、提交失败、观察去重与原生刷新改写。
