# ADR-007：agent-adapters 中的 Rig LLMClient

- 状态：Accepted
- 日期：2026-09-07
- 依赖：ADR-001 v4、NEC-151 ADR-002/003、NEC-154 ADR-002

## 背景

需要在 `agent-adapters` 中通过 API key 配置构建 Rig client，并提供远端模型列表查询和单次模型调用。NEC-151 ADR-003 原本只把完整 harness 放在该 crate，需要明确这个新增的 SDK 边界。

## 决策

1. `agent-adapters::LLMClient` 封装 Rig 的 OpenAI / DeepSeek client；它是 adapter 级 SDK 入口，不实现完整 `AgentAdapter` harness 协议，也不替代 `providers` 中的流式 `ProviderAdapter` contract。
2. `LLMClientConfig` 显式接收 provider、内存 API key、可选 API 根地址和请求超时。默认地址分别为 OpenAI `/v1` 根地址和 DeepSeek 根地址；自定义地址保留调用方给出的版本前缀。
3. `list_models` 使用 Rig 原生模型查询；`completion_request` 构建 Rig 请求，`complete` 单次返回结构化内容与 usage，`prompt` 提供文本便捷接口。OpenAI 使用 Responses API，DeepSeek 使用 Chat Completions API。
4. Rig 的请求/响应类型仅属于 adapter 接口，不进入 domain、ports、存储 schema 或本地 HTTP/IPC contract。`domain` 的依赖和所有领域不变量不变。
5. SDK 不拥有历史、Session 指针或 Run。ToolCall 只作为响应数据返回，不在该 client 内执行工具；一次调用返回不代表 Run 通过终止屏障。请求不自动重试，重试由调用方协调。
6. API key 不序列化、不记录；配置与 client 的 Debug 隐去敏感字段。外部错误仅保留稳定分类与 HTTP 状态，不传播可能回显凭证、prompt 或地址的 SDK 错误正文。持久化配置仍只保存 credential reference。
7. 通过本地 HTTP fixtures 验证两家 SDK 的鉴权、模型列表、请求格式、结构化返回、错误分类、超时和无隐式重试。默认测试不使用真实账号或计费 API。

## 后果

Rig 及其 HTTP 依赖局限于 adapter crate。`providers` 继续承担现有统一流式协议；若未来 runtime 需要使用 Rig，应另行通过该协议适配并运行其 contract tests，而不是把 Rig 的 Agent 循环接管为本项目 Run。
