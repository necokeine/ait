# ADR-001：Codex 阶段化输出时间线

- 状态：Proposed（NEC-204 实现，待评审）
- 日期：2026-09-07
- 依赖：ADR-001 v4、NEC-198 ADR-001、ADR-012

## 决策

1. `WorkspaceAgent` 按 Codex app-server 的 `itemId` 分别聚合 `agentMessage`，保留 item
   首次出现的顺序、消息边界和 `commentary | final_answer | unknown` 原生阶段。原生操作仍归一化为
   `WorkspaceOperation`，有序输出只通过 id 引用它，不改变 Ait 的 `ToolUse`、`ToolResult` 或
   `ToolExecution` 语义。
2. 同一 item 的 delta 只在该 item 内累积；`item/completed.text` 作为完整权威值与 delta
   协调，相等或包含时只保留一份，不把完整文本再次追加。持久化 Message 的 `text` 只保存
   `final_answer`；旧协议没有阶段时退回最后一条非空 agent message。
3. 展示时间线与最终文本在同一个不可变 assistant Message 中原子保存：操作详情继续位于
   `data.codex.operations`，有序消息/操作引用写入 `data.codex.output_items`。因此刷新、daemon
   重启和历史恢复使用同一份持久化事实，不依赖运行时内存。
4. 桌面端按 `output_items` 还原过程事件顺序，将所有非最终消息和原生操作放进默认收起的
   Process 区域；最终答复独立、默认展开并排在最后。完成后滚动锚点指向最后一条消息的最终
   答复。没有 `output_items` 的历史快照不改写 Message：桌面端只在读取时把既有 operations
   投影到折叠 Process，并把既有正文放在最后，继续保持可读且不再以工具记录收尾。

## 结果

- 多条过程消息不会再被跨 item 拼接或被最后一个完成事件覆盖，最终 Message 正文也不再混入
  commentary。
- 阶段、边界与过程顺序成为可恢复的展示数据，同时保持 Message 树和宿主工具审计模型不变。
- 新旧快照均可显示；新投影中的未知阶段由桌面端把最后一条消息作为兼容性最终答复。

## 验证

- adapter 测试覆盖多 item、delta/完整文本协调、阶段与消息/操作顺序。
- application 测试覆盖 `output_items` 原子持久化及 SQLite 重启后恢复。
- desktop 测试覆盖有序投影、默认折叠过程、独立最终答复、旧数据回退和树预览。
