# ADR-011：默认 API Tool set 与 System Prompt

- 状态：Accepted（NEC-190 首阶段实现）
- 日期：2026-09-07
- 依赖：ADR-001 v4、ADR-007、ADR-009

## 背景

API 调用已有 Rust/Rig 客户端，但没有默认 System Prompt 或工具定义。
NEC-190 要求先构建 System Prompt，以 DeepSeek API 验证，工具范围参考
DeepSeek Harness；父任务 NEC-189 要求共享默认配置及按模型覆盖。

## 决策

1. `ait-tools` 保存不依赖 SDK 的 `ToolDefinition`、不可变 `ToolSet` 和
   `ToolSetRegistry`。默认版本为 `ait-default-v1`，按名称稳定排序；
   `(provider, model)` 精确命中覆盖项，否则回退共享默认。当前配置仅存在
   client 内存中，不扩充领域 Agent schema 或添加隐式持久化。
2. 默认范围固定为 DeepSeek Harness commit
   `d347e703908d0406b7a7ef80e3a0e594d86b2215` 的 Standard preset，补充
   Minimal 的 `str_replace_editor`。每个宿主 28 个函数；Shell 按操作系统
   选择 `bash` 或 `pwsh`。逐项映射、源码来源和 MIT 声明见
   `crates/tools/README.md`。可选插件不作为默认范围。
3. `agent-adapters::LLMClient::completion_request` 默认注入工具定义及 Ait
   System Prompt。带历史的组装顺序是 `[默认 system, 原有历史, 当前 user]`；
   user 内容原样保留，不混入 system，不做模板替换。Project 的根 System
   Message 保持不可变；新增内容仅属于本次 API 请求投影。
4. 工具 Schema 作为独立 `tools` 字段交给 Rig，支持 DeepSeek Chat Completions
   和 OpenAI Responses（系统指令映射为 `instructions`）。模型返回的
   ToolCall 仍作为数据交还宿主，符合 ADR-007 的单次调用边界。目录不代表
   授权或执行能力。DeepSeek 的 `content: null` 及省略 tool-call `index`
   由窄 HTTP 适配层归一化，以兼容 Rig 0.42 的响应解析器。
5. 首阶段不实现 Shell/文件/Web/子 Agent 等执行器或多轮运行器。现有
   `AgentProviderGateway::complete -> String` 无法处理 ToolCall，因此使用
   默认 system 加纯文本历史，不发送工具目录；`LLMClient::prompt` 同样如此。
   宿主接入工具时必须经过 ToolUse/ToolResult 持久化、权限和 Run 终止屏障，
   不能在 `LLMClient` 内隐式执行模型选定的命令。
6. 默认验证使用实际 Rig client 对接本地 HTTP fixtures，重点验证 DeepSeek
   的消息顺序、函数封装与调用 ID/推理内容回传，同时覆盖 OpenAI 请求格式。
   提供忽略执行的 DeepSeek 真网 smoke test，由操作者显式配置环境凭证及模型。

## 后果

调用方可直接构建可审查的 Prompt/Tool set，也可为某模型提供完整替代配置。
现有文本聊天开始使用默认身份和工作指导，但不会暴露尚未接入的工具。
后续工具执行阶段仍须实现桥接、权限、能力过滤及持久化，不能把 Schema
覆盖率当作可执行能力覆盖率。此次不改变 Message、Session、Run 的领域边界。
