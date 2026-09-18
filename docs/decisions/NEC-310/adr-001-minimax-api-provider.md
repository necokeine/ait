# ADR-001：MiniMax API Provider

- 状态：Accepted
- 日期：2026-09-17
- 来源：NEC-310
- 修订：ADR-007 的 Rig client 范围、ADR-009 的 Provider kind 目录与 ADR-010 的远端模型发现入口。

## 决策

1. `ProviderKind` 增加稳定线格式 `minimax`。MiniMax 与 OpenAI、DeepSeek、Gemini 一样是用户创建、凭据由操作系统凭据库存储的远端 API Provider；不注册内置 MiniMax 连接或默认模型。
2. `agent-adapters::LLMClient` 使用 MiniMax 官方 OpenAI-compatible Chat Completions 协议，不复用 Ait 的 OpenAI Responses API 方言。国际默认 API 根为 `https://api.minimax.io/v1`；中国区用户可把自定义 URL 设置为 `https://api.minimaxi.com/v1`。
3. 模型发现使用带 Bearer 鉴权的 `GET /models`，保存前继续经过现有两段式预览与模型选择流程。adapter 不从模型名称猜测 reasoning effort，MiniMax 目录默认返回空等级；显式发送 MiniMax reasoning effort 在网络调用前拒绝。
4. MiniMax 复用公共 API Provider 的 RunCoordinator、HostTools、权限快照、独立 worker、持久化工具循环与错误脱敏。CLI、daemon/worker IPC 和 Desktop 都接受 `minimax`。
5. MiniMax 的 reasoning 内容保留在兼容接口返回的 assistant `content` 中，包括与 tool calls 同轮返回的 `<think>` 内容。Ait 持久化完整 assistant 文本与 tool calls，并在后续请求中原样重放，以维持多轮工具调用的 reasoning 连续性；不启用会改变返回形状的 `reasoning_split`。

## 安全与兼容性

- API key 不进入 Provider、Run、Message、事件或日志；SDK 错误正文继续被稳定分类替代。
- 已有 Codex、OpenAI、DeepSeek、Gemini 数据格式不变。新增枚举值只扩展新记录的可选范围。
- MiniMax 与其他普通 API Provider 一样只支持 `on_request` 审批模式，并受 Run sandbox 快照和管理员上限约束。

## 验证

- adapter 离线 HTTP fixture 覆盖 Bearer API key、`GET /models`、`POST /chat/completions`、system/history、工具声明、结构化响应、错误分类、超时与无重试。
- 真实 worker 子进程 fixture 覆盖 MiniMax 三轮写入、读取、搜索工具循环，以及 `<think>` 内容重放、ToolResult 顺序、Message、Run usage、SQLite receipt 和终态持久化。
- application、CLI 与 Desktop 测试覆盖 MiniMax Provider 选择、凭据保存、权限和 native/API harness 分流。

## 参考

- [MiniMax OpenAI-compatible API](https://platform.minimax.io/docs/api-reference/text-openai-api)
- [MiniMax models.list](https://platform.minimax.io/docs/api-reference/models/openai/list-models)
