# ADR-009：Session 独占执行与 AgentProvider 配置层

- 状态：Accepted
- 日期：2026-09-07
- 来源：用户要求删除发送消息的乐观锁、活动 Run 拒绝新输入，以及共享 Provider / 命名与匿名 Agent 配置。
- 修订：ADR-001 v4 中 Provider 非领域对象、Session 外部 version CAS、活动 Session 接受新用户输入的条款；替代 NEC-174 Run reasoning effort 覆盖值设计。

## Session 与 Run

daemon 为 Session 的发送、重绑与配置修改取得立即失败的独占许可。许可从请求进入应用层开始，跨越状态提交和真实 Agent 调用，直到命令完成才释放。同一 Session 的竞争请求直接返回 `SESSION_BUSY`，不会排队等待后再偷偷发送。

持久化 `active_run_id` 是跨请求和重启后的占用事实。新 user Message、Session 指针和 Run 在同一提交内落盘；有 active Run 时，daemon 在 Git 检查和写入之前拒绝新的用户输入。Manual 的 queued 和等待审批状态继续占用 Session，可查询和取消。Run 内部的工具结果、重试、恢复与终止屏障仍保留，外部人类输入不再加入活动 Run 队列。不同 Session 使用不同许可，可以独立运行。

`SendMessage` 只接受 `session_id`、`text`。`SetSessionAgent` 只接受 Session 和 Agent ID。移除两者的 `expected_version`，并从发送与分支操作移除 `reasoning_effort`。HTTP 对这些旧字段返回 422。Session `version` 仅作为变化序号和内部存储一致性依据；底层 MessageStore/SessionStore 的指针 CAS、ControlStore 的全局事务 revision、设置编辑的 revision 不承担用户输入准入，不在此次删除范围内。

取消先持久化终态并通知执行中的调用退出，独占许可在调用退出后释放；迟到的调用结果不得追加输出。执行入口仍遵循 ADR-008：真实调用在 `try_execute` 内完成，新执行返回终态；查询、Manual 和审批等待可返回中间态。

## 配置关系

```text
AgentProvider { id, name, kind, url?, models[] }
ProviderModel { id, name, reasoning_efforts[] }
Agent { id, name, config, owner_session_id?, revision, enabled }
Agent.config { provider_id, model, reasoning_effort? }
Project.default_agent_id -> named Agent
Session.agent_id -> named Agent | Session-owned anonymous Agent
Run -> fixed Agent id + revision + config + provider connection snapshot
```

Provider kind 选择 Codex、OpenAI、DeepSeek 等适配器。URL、认证和模型能力被多个 Agent 共享。领域只保存纯数据，应用层通过端口协调，Rig、HTTP 与操作系统凭证库留在 `agent-adapters`。

命名 Agent 是可复用预设；Project 默认配置与 Cron 必须选择命名 Agent。Session 也可以绑定命名预设。Session 改模型或推理强度时，若当前为命名预设则派生一个 `name=""`、`owner_session_id=session.id` 的匿名 Agent；后续调整更新同一个匿名 Agent 并增加 revision。新建或分支 Session 若选择了另一 Session 的匿名 Agent，则复制配置并赋予新的 Agent 身份。配置修改不修改 Message 历史，也不影响其他 Session 的匿名配置。

在设置中修改命名预设会更新该预设的后续使用。每个 Run 在创建事务内固定完整配置与 Provider 连接，并在私有状态中固定凭证引用；执行期间不重新解析可变 Agent。这样修改预设、URL 或轮换密钥不会改变已开始的 Run。

模型与 reasoning effort 必须存在于 Provider catalog 中。空 effort 表示供应商默认值。模型 ID 和等级均为字符串，避免把所有供应商限制在 Codex 的固定枚举。模型发现使用实际 API 的 `/models`；该接口或 adapter 不提供等级时，新模型的等级为空，已有模型保留手工声明的等级，不从名称猜测能力。若 adapter 提供非空等级目录，则该能力事实优先于旧保存值；DeepSeek 的 adapter-owned 目录由 [NEC-218 ADR-001](NEC-218/adr-001-deepseek-reasoning-efforts.md) 定义。发现中若连接已变化，拒绝把旧响应写入新连接。模型下架后旧配置仍可查询，但新执行必须重新通过能力校验。

OpenAI 的调用使用 Rig Responses API 的 `reasoning.effort`。DeepSeek Chat
Completions 的非 `off` 等级显式发送 `thinking.type=enabled` 与
`reasoning_effort`；参考 [DeepSeek Harness 的 adapter-owned effort
约定](https://github.com/deepseek-ai/deepseek-harness/blob/c389f96bf3a9b6807cb71ed6bdad5849be0df6d8/packages/llm/llm-deepseek/README.md)，
`off` 转换为 `thinking.type=disabled` 且不得作为 `reasoning_effort=off`
发送。空值不发送任何控制字段，保留供应商默认。参考：[DeepSeek thinking mode](https://api-docs.deepseek.com/guides/thinking_mode/)。当前远程 LLM 执行投影文本历史并进行一次调用；Codex 的工作区工具循环仍由其 adapter 负责。

## 凭证

Provider 保存接口的 `secret` 为只写输入，Debug 脱敏。实际值写入操作系统凭证库，SQLite 只保存不透明引用。快照和事件仅返回 `has_secret`。归档不包含 secret、凭证引用或该标志；导入后远程 Provider 需要重新配置凭证。配置写入失败时回收未使用的新凭证。已被历史 Run 固定的旧凭证引用继续保留，以免轮换破坏固定配置。

Codex 沿用主机登录，不接受 API secret。远程 URL 必须为不含用户名、密码、查询参数或 fragment 的 HTTP(S) URL。凭证存储不可用时明确失败，不回退成明文配置。

## API 与桌面端

桌面入口与发现/保存分离流程现由 [ADR-010](adr-010-provider-discovery-and-agents-page.md) 修订；下文保留最初实现记录。

| 操作 | 请求字段 |
| --- | --- |
| `POST /v1/agent-provider/save` | `provider: {id,name,kind,url,models}`, `secret?` |
| `POST /v1/agent-provider/refresh-models` | `provider_id` |
| `POST /v1/agent/register` | `id`, `name`, `config` |
| `POST /v1/agent/update` | `id`, `name`, `config` |
| `POST /v1/session/set-config` | `session_id`, `config` |
| `POST /v1/session/set-agent` | `session_id`, `agent_id` |
| `POST /v1/session/send-message` | `session_id`, `text` |

桌面 Settings 的 Models 管理 Provider、URL、密钥、模型和等级，并提供模型发现；Agents 管理命名预设。Composer 的 Provider、Model、Reasoning 变更立即保存 Session 配置，发送按钮在保存期间禁用。Project 和新 Session 的共享预设选择器只列命名 Agent。

原来的 `models.default/provider/endpoint/credential_ref` 全局设置没有驱动实际执行，现在从 settings schema 删除，避免第二套配置来源。设置 schema revision 升为 2；其余偏好保留。

## 数据升级与验证

旧 SQLite JSON 快照在读取时升级：按原 Agent mode 建立 Provider 目录，保持 Agent ID、revision、Session 绑定和全部 Message 不变；旧 Run 上的 effort 移入其固定 config。下一次原子提交保存新结构。旧 Agent 没有持久化 effort 时使用供应商默认，不从某个历史 Run 推断全局默认。

Project 导出格式升至 3，加入无凭证 Provider 目录。格式 2 仍能导入，会生成独立 Provider ID，避免意外复用本机同名连接的凭证。导入校验匿名 Agent 所有者、命名默认绑定、配置与 Provider 引用及身份冲突；供应商下架模型不妨碍历史归档，重新执行时才校验当前能力。

自动验证覆盖竞争发送无副作用、不同 Session 并行、取消、配置隔离和复用、Run 固定配置、能力校验、模型发现、凭证脱敏、重启与归档迁移，以及 HTTP 旧字段拒绝。真实供应商付费调用和系统钥匙串交互不作为自动测试前置条件。
