# ADR-001：Codex 操作投影与消息渲染

- 状态：Proposed（NEC-198 实现，待评审）
- 日期：2026-09-07
- 依赖：ADR-001 v4、ADR-004、ADR-012

## 决策

1. Codex app-server 的完成态原生 item 在 `WorkspaceAgent` 边界归一化为有界的
   `WorkspaceOperation`。它只保留类型、状态、标题、摘要、详情和 Project 路径；
   单轮最多 200 条且总字符数不超过 256,000，摘要、详情和路径另有限长与数量。
2. 操作记录随最终 assistant Message 原子保存到 `data.codex.operations`，桌面端将其
   显示为可折叠审计记录。该投影是 Codex harness 的展示事实，不等同于 Ait 宿主执行的
   `ToolUse`、`ToolResult` 或 `ToolExecution`，因此不伪造 call id、审批或重试语义。
3. assistant 文本使用不解释原始 HTML 的安全 Markdown 子集，支持标题、列表、引用、
   fenced/inline code、HTTPS 链接和 GFM 风格表格。所有模型文本先转义，再由受控模板生成
   元素；用户输入继续按纯文本显示。
4. Markdown 文件引用和操作记录中的路径只作为无本地权限的字符串传给 renderer。点击后
   经 preload 发送 `project_id + path + line? + column?`；main process 对 Project 根与目标执行
   `realpath`，拒绝 `..` 和符号链接越界。含行号时优先用 VS Code file URI 定位，协议
   不可用时退回系统默认应用；无行号路径直接使用默认应用打开。

## 结果

- 历史 Codex 回复可同时恢复最终文本和原生操作记录，而不改变 Message 树角色或工具领域模型。
- 表格与文件位置在桌面会话中可读、可横向滚动、可键盘聚焦。
- renderer 不获得任意本地文件权限，模型生成的路径也不能越过当前 Project 安全边界。

## 验证

- Rust adapter 测试覆盖 app-server command item 到 `WorkspaceOperation` 的归一化。
- application 测试覆盖 operation 与 commit 元数据一起持久化。
- desktop 测试覆盖表格、链接、路径/行列解析、操作记录展示以及符号链接越界拒绝。
