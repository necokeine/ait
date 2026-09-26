# ADR-050：独立 server 的 Claude Code Provider

- 状态：Accepted；能力扩展及下述初版限制由 [ADR-052](adr-052-native-provider-capabilities.md) 更新。
- 日期：2026-09-26
- 前置：ADR-001 v4、ADR-027、ADR-032、ADR-039、ADR-040、ADR-041

## 初版决策

以下记录初次接入时的决策；当前完整能力和只读额度凭据访问边界以 ADR-052 为准。

独立 Rust server 在 `server-provider::local::claude` 增加 `ClaudeClient`，注册身份为
`claude`，与 Paseo 的 provider ID 一致。客户端已有 Claude Code 图标、名称与模式元数据。
生产启动默认调用本地 `claude`，可通过 `AIT_SERVER_CLAUDE_BIN` 指定可执行文件。

参考本地 Paseo 固定提交 `2c8e8a826810337492cc5a38bb0bbd705b6fb632` 的
`packages/server/src/server/agent/providers/claude/{agent,query,models,project-dir}.ts`。
Paseo 使用官方 TypeScript Agent SDK；本实现遵循 Rust 语言约束，直接实现同一 CLI
`--input-format stream-json --output-format stream-json` 双向控制协议。
握手、权限响应与中断信封对照 Anthropic 官方 Python SDK 的
[`query.py`](https://github.com/anthropics/claude-agent-sdk-python/blob/main/src/claude_agent_sdk/_internal/query.py)
和 [`subprocess_cli.py`](https://github.com/anthropics/claude-agent-sdk-python/blob/main/src/claude_agent_sdk/_internal/transport/subprocess_cli.py)。

1. CLI 保留认证、原生工具、MCP、技能、CLAUDE.md、项目/用户/local settings 和原生历史所有权。
   自定义 system prompt 追加到 Claude Code 默认提示词。宿主不读取、保存或打印认证材料。
   子进程 stderr 不进入客户端或持久日志；可通过 `claude auth status` 单独检查登录。
2. 模型和推理等级取自已安装 CLI 的 `initialize.models`，命令取自 `initialize.commands`。
   发现过程不发送用户提示词，并禁用探测会话的历史保存。不同于 Paseo 的静态模型清单，
   本实现以本机运行时的目录为准，支持运行时提供的自定义模型。
3. 使用既有 `AgentClient` / `AgentSession`，实现创建、恢复、发送、流式输出、审批、中断和关闭。
   Session UUID 在创建时固定；后续配置变化重新启动并恢复同一原生会话。
   默认模式为 `default`，显式支持 `plan`、`acceptEdits`、`auto`、`bypassPermissions`。
   不设置全局自动放行；仅选择 bypass 模式时向 CLI 传递 bypass 启动能力。
4. `can_use_tool` 转为单次权限交互；`AskUserQuestion` 映射 question UI，并将回答标题归一化为
   Claude 原始问题文本。未知控制请求失败关闭；不接受隐式 session 级权限扩张。
5. JSONL 帧、消息队列、历史读取和展示内容有界。流式文本与完整 assistant 内容使用一致的
   原生键，工具结果由 user/tool_result 补全展示项。子 Agent 内容不混入父 Agent 的回复。
   中断得到控制回复后回收进程组，下一轮恢复历史，避免旧结果完成新 turn。
6. 原生会话发现、导入、历史展示与刷新只读 `CLAUDE_CONFIG_DIR/projects`（默认 `~/.claude/projects`），
   校验 UUID、原生 cwd 和读取预算。历史不完整或超过预算时返回错误，不静默截断为完整历史。

## 边界与初版限制

本次新增的是独立 server 的 adapter；旧 daemon 的 `ait-domain::ProviderKind` 不参与这个运行路径。
不改变核心 Message 树、Session ref 或 Run 终止屏障。Timeline 仍是原生历史的展示投影。

初次接入时尚未支持运行中追加输入、rewind、子 Agent、账户额度、fast mode、附件输入及
providerOptions/MCP 的宿主级覆盖。后续 ADR-052 已补齐这些能力，并明确区分 stdin 写入回执
和推理确认；这些初版限制不代表当前实现。未知宿主配置仍明确拒绝，不静默丢弃。

支持范围、测试和覆盖率见[实施报告](../reports/provider-parity.md)。
