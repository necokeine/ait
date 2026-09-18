# ADR-001：Gemini API Provider

- 状态：Accepted
- 日期：2026-09-17
- 来源：NEC-309
- 修订：ADR-007 的 Rig client 范围、ADR-009 的 Provider kind 目录与 ADR-010 的远端模型发现入口。

## 决策

1. `ProviderKind` 增加稳定线格式 `gemini`。Gemini 与 OpenAI、DeepSeek 一样是用户创建、凭据由操作系统凭据库存储的远端 API Provider；不注册内置 Gemini 连接或默认模型。
2. `agent-adapters::LLMClient` 使用 Rig 0.42 的原生 Gemini GenerateContent client，不把 Gemini 伪装成 OpenAI-compatible API。默认 API 根为 `https://generativelanguage.googleapis.com`；自定义 URL 仍表示 API 根，Rig 负责追加 `/v1beta/models` 与 `:generateContent` 路径和 API key 鉴权。
3. 模型发现使用 Gemini `models.list`，保存前继续经过现有两段式预览与模型选择流程。adapter 不从模型名称猜测 reasoning effort，Gemini 目录因此默认返回空等级；显式发送 Gemini reasoning effort 在网络调用前拒绝。
4. Gemini 复用公共 API Provider 的 RunCoordinator、HostTools、权限快照、独立 worker、持久化工具循环与错误脱敏。CLI、daemon/worker IPC 和 Desktop 都接受 `gemini`。
5. Gemini REST 的 `functionCall.id` 可缺失。Ait 为持久化、执行与 ToolResult 关联生成内部 call ID，但序列化后续 Gemini 请求时只回传真实 provider ID；无 provider ID 时 `functionCall` / `functionResponse` 都不写伪造 ID，使用函数名完成 Gemini 协议关联。

## 安全与兼容性

- API key 不进入 Provider、Run、Message、事件或日志；SDK 错误正文继续被稳定分类替代。
- 已有 Codex、OpenAI、DeepSeek 数据格式不变。新增枚举值只扩展新记录的可选范围。
- Gemini 与其他普通 API Provider 一样只支持 `on_request` 审批模式，并受 Run sandbox 快照和管理员上限约束。

## 验证

- adapter 离线 HTTP fixture 覆盖 Gemini API key、模型列表、请求路径、system instruction、工具声明、结构化响应、错误分类、超时与无重试。
- 真实 worker 子进程 fixture 覆盖 Gemini 无 provider call ID 的三轮写入、读取、搜索工具循环，以及 Message、Run usage、SQLite receipt 和终态持久化。
- application、CLI 与 Desktop 测试覆盖 Gemini Provider 选择、凭据保存、权限和 native/API harness 分流。

## 参考

- [Gemini API reference](https://ai.google.dev/api)
- [Gemini models.list](https://ai.google.dev/api/models)
- [Gemini function calling](https://ai.google.dev/gemini-api/docs/function-calling)
