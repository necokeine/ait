# ADR-001: 对齐并执行 API Agent 工具

- 状态：Accepted
- 日期：2026-09-18
- 关联：NEC-313、NEC-312、NEC-247、NEC-290

## 背景

默认 API ToolSet 已包含 28 个 provider-neutral schema，但生产 HostTools 过去只执行文件与 shell 子集。模型仍可能看到只有 schema、没有生产执行路径的 Web、提问、Todo、Skill、Subagent 和计划退出工具。名称还必须与 Ait/OpenCode 约定一致，不能用兼容别名掩盖缺失能力。

## 决策

生产 API Agent 使用以下精确名称，不保留旧名别名：

| 能力 | 名称 |
| --- | --- |
| Web | `web_fetch`、`web_search` |
| 用户交互 | `ask_user_question` |
| Todo | `todo_write` |
| Skill | `skill` |
| 子任务 | `subagent`、`subagent_fork` |
| 计划确认 | `exit_plan_mode` |

执行器拆成 Project-local HostTools 与 AgentTools，并由 `CompositeRunTool` 合并。Provider 只收到合并执行器实际声明的 schema。

### 本地工具

- `skill` 只读取 Session worktree 内 `.agents/skills`、`.opencode/skills`、`.ait/skills` 或 `skills` 下精确名称的 `SKILL.md`，目录与文件均不跟随符号链接，结果受 64 KiB 上限约束。
- `todo_write` 校验并返回完整替换列表。该列表作为普通 ToolResult 持久化并进入后续模型上下文，不另建第二份 Todo 聚合状态。
- `web_fetch` 仅接受 HTTP(S) 文本。每一跳都解析并固定 DNS 地址，拒绝凭据、本机、私网、链路本地、保留及文档地址，禁用环境代理，限制跳转、时间、下载量与返回文本量。
- `web_search` 使用 DuckDuckGo 公共 HTML 入口，返回有界的标题、URL、摘要，并把结果标记为外部不可信内容；重要结论仍应使用 `web_fetch` 检查来源。

### 用户交互

`ask_user_question` 和 `exit_plan_mode` 不在 worker 内直接访问 UI。application 以 ToolExecution ID 保存 `tool_interactions`，通过私有 worker IPC 阻塞原 ToolUse，并由 HTTP/Desktop 显式提交答案、批准、拒绝或取消。记录受 Run lease、worker connection、Run 取消与总 deadline 约束；重启恢复复用同一 ID 和已提交结果，不创建第二个提示。

计划批准表示当前 API Agent 可以继续执行该计划。API Provider 没有独立的全局 plan/build 权限模式，因此该工具是一次持久化的计划审阅关口，而不是修改 Run 权限快照。

### 子 Agent

`subagent` 从自包含 prompt 开始；`subagent_fork` 继承当前 ToolUse 之前的已完成 Message path。两者都使用当前 Agent 的 provider/model 配置，在前台最多执行八个模型轮次和 16 个嵌套工具调用，并共享父 Run 的取消与 runtime deadline。子调用不暴露递归 delegation 工具；其 token、cost 与嵌套工具用量累计到父 Run。

## 当前明确限制

- 不支持 `run_in_background=true`。后台子任务需要可持久化的 child Run、收集协议、恢复和取消所有权，不能安全地伪装成一个普通 ToolResult。
- 不支持每次子调用指定不同 provider、model 或 reasoning effort；生产 schema 会移除这些字段。跨路由需要独立的 Agent/凭据/预算准入设计。
- `web_search` 依赖公共 HTML 端点，无可用性 SLA，也不替代一手来源验证。
- Skill 只来自当前 Project 的显式目录，不读取宿主用户目录或远端市场。

这些限制是 fail-closed 行为：未实现的字段不广告，后台请求被 schema 和执行器共同拒绝。

## 后果与验证

Unix 且 shell 可用时，默认 API Run 现在可执行 14 个工具；无 shell 时为 13 个。新协议能力 `tool-interactions-v1` 将 daemon/worker minor version 提升到 3，旧 worker 会在握手阶段被拒绝。

验证覆盖 schema/旧名拒绝、Project-local Skill、Todo、Web 私网防护与解析、Subagent 上下文/工具循环/用量、持久化提问与计划批准完整回路，以及 Desktop 转义、答案收集与 pending 计数。工作区继续执行 format、Clippy、Rust tests 和 Desktop typecheck/tests。
