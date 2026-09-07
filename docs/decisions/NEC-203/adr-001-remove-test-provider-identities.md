# ADR：移除测试场景的 Provider 身份

- 状态：Accepted
- 日期：2026-09-07
- 来源：NEC-203，审查并清理 built-in Agent Provider。
- 修订：NEC-152 的确定性验收模式，以及 NEC-196 的退役 Provider 兼容范围。

## 决策

`ProviderKind` 只保留真实执行适配器：`Codex`、`OpenAI` 和 `DeepSeek`。fresh workspace
只注册 `builtin-codex`；OpenAI 与 DeepSeek 仍由用户创建连接。

删除 `tool`、`manual`、`provider_failure`、`approval_required` 四个领域枚举值、内置目录项和
`apply_run_mode` 执行旁路。新 Run 统一在初始状态提交后，通过 Codex workspace executor 或 API
Provider gateway 执行。公共 API、archive 和桌面 Provider/Agent 选择面不再能创建这些测试模式。

## 测试语义

删除生产身份不删除测试覆盖：

- ToolUse、ToolResult 顺序与工具失败由 runtime 的 `ScriptedAgent`、`ScriptedTools` 验证；
- pending → approved → resume 由 `RunApproval` scripted decision 验证；
- Provider 失败由 failing workspace executor / gateway 验证，并检查失败 Run 持久化和 Session 释放；
- queued 查询、Cron 去重和取消通过拒绝 running checkpoint 的 fake store 构造，不需要伪 Provider；
- HTTP/CLI 纵向测试向 `WorkspaceAgent` port 注入确定性 fake，只用于测试进程，不进入 Provider 目录。

## 旧数据

沿用 NEC-196 的退役策略。读取旧快照时，未被 Agent、Run 或 credential 引用且保持标准
`builtin-<kind>` 形态的四个目录项会被移除；下一次正常提交保存清理后的目录。

任何仍被引用的退役项，以及带自定义 ID、URL 或 credential 的未知连接，都必须拒绝解码或导入。
失败不删除 Message、不改写历史 Run，也不把 Agent 静默重绑到 Codex。兼容测试对四种退役 kind
覆盖 Agent、Run、credential、自定义 ID 和 URL 引用矩阵。

## 结果

Provider 目录只表达可实际调用的 adapter，Run 状态表示执行结果而不是 Provider 类型。工具审批和
Run 协调继续以 ADR-001 v4 的 port、checkpoint 与状态机为权威边界，测试夹具不再污染领域枚举、
持久化格式和用户界面。
