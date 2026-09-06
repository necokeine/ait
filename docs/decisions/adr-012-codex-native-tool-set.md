# ADR-012：Codex 原生 Tool set 与分层提示词

- 状态：Proposed（NEC-191 实现，待评审）
- 日期：2026-09-07
- 依赖：ADR-001 v4、ADR-003 Agent Adapters、ADR-009、ADR-011

## 决策

1. Codex Provider 使用 `ait-tools::codex::CodexToolSet`，版本为
   `ait-codex-native-v1`。它表示 codex-core 原生工具执行配置，不是另一份
   API function catalog，也不能转换为 `ToolSet`。Schema、工具选择及执行器
   全部由本机 `codex app-server` 的 core 提供，随模型、平台、权限和配置变化。
   这样可以保留 free-form `apply_patch`、命令会话及 hosted tools 的原始语义。
2. 应用仅在固定的 `run.provider.kind == Codex` 时调用原生执行器。
   OpenAI/DeepSeek API 继续使用 ADR-011 的默认目录与精确模型覆盖；即使
   API 模型名称包含 `codex`，也不会启用本配置。其他 Provider 不回退到 Codex。
3. 按 Codex app-server 客户端的分层方式构建提示词：core 保留基础模型提示词；
   Ait 的宿主指导与不可变 Project system Message 快照依次进入
   `developerInstructions`；非 system 历史和当前用户输入进入 `turn/start.input`。
   不发送 `baseInstructions`，不把原生工具复制为 `dynamicTools`，不改写历史。
   core 继续负责权限、环境和本机项目指令的注入。
4. `thread/start` 和 `thread/resume` 都传入同一套 developer 层，并明确传递
   本次 cwd、模型、sandbox 和 approval policy，避免恢复线程继承旧权限。
   `WorkspaceAgentInvocation.project_instructions` 和
   `AgentRunRequest.project_instructions` 仅传递本次请求投影，不新增持久化字段。
5. core 的工具生命周期沿现有 `ItemStarted` / `ItemCompleted` 原样透传，
   命令/文件/权限审批继续使用原有 `ApprovalHandler`。取消或关闭消费流时清理
   此次创建的 app-server 子进程。本文不改变 Message/ToolResult 与 Run 的领域
   边界，也不声称已将所有 core 内部工具调用投影为 Ait 持久化 ToolExecution。

## 上游依据

- [官方 Codex app-server 文档](https://developers.openai.com/codex/app-server)：
  客户端握手、新建/恢复线程、原生 item 生命周期。
- 检查源码固定为 OpenAI Codex commit
  `ad931a45b201e3877d6ba542ba5dbbd85e7e31b4`：
  [core 工具组合与执行器](https://github.com/openai/codex/blob/ad931a45b201e3877d6ba542ba5dbbd85e7e31b4/codex-rs/core/src/tools/spec_plan.rs)、
  [app-server 线程配置](https://github.com/openai/codex/blob/ad931a45b201e3877d6ba542ba5dbbd85e7e31b4/codex-rs/app-server/src/request_processors/thread_processor.rs)、
  [developer 消息层](https://github.com/openai/codex/blob/ad931a45b201e3877d6ba542ba5dbbd85e7e31b4/codex-rs/core/src/context/developer_instructions.rs)。
- 协议字段另外用本机 `codex-cli 0.153.4` 的
  `app-server generate-json-schema` 核验。源码 revision 是参考记录，不是运行时
  二进制 pin。未复制或声称还原闭源 Desktop 的完整私有提示词；参考的是其
  app-server/core 分层边界。

## 验证与范围

离线测试覆盖 Provider 路由、同名 API 模型隔离、Project/system 与 user 分离、
原始快照不变，以及 start/resume 的协议参数。已有 API HTTP fixtures 继续验证
默认 28 个工具和准确的 system 消息。

显式启用的 `codex_python` 测试使用真实 Codex 两轮执行：第一轮用原生
`apply_patch` 创建 `hello.py`，原生命令执行 Python；中间检查成功的工具事件、
对应 start/completed ID、命令 exit code 与真实磁盘文件；第二轮恢复同一线程，
读取、用补丁重构为 `main()`、再次运行验证。测试宿主也在两轮之间和最终独立运行
Python，必须精确输出 `Hello, world!\n`。生成精简 `verification.json`，不导出
登录凭证或完整会话内容。此 smoke 验证编辑、命令、读取及恢复路径，不代表
所有可选 Web、图片、MCP 或协作工具都在当前机器启用。

```bash
cargo test -p ait-agent-adapters --test codex_python -- --ignored --exact codex_native_tools_create_and_verify_python_hello_world --nocapture
```

需要本机已经登录 Codex、安装 Python 3 和 Git。模型默认沿用 Ait 的
`gpt-5.6-sol`，可用 `AIT_CODEX_SMOKE_MODEL` 显式选择有权限的模型。
每轮最多 240 秒，测试默认忽略以避免普通 CI 使用真实账号。
