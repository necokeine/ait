## NEC-166：按实体与操作拆分本地 HTTP API

- 状态：Accepted
- 日期：2026-09-04
- 替代：NEC-152 中统一的 `POST /v1/commands` HTTP 入口
- 来源：NEC-166 产品反馈

## 决策

HTTP 传输层不再公开带 `type` 判别字段的统一 command 入口。每个用例都有独立的
`/v1/<entity>/<operation>` 路由，请求体只包含该操作所需字段。实体名和操作名使用单数、
小写 kebab-case。当前路由为：

| Method | Path | Application command |
| --- | --- | --- |
| `POST` | `/v1/project/register` | `RegisterProject` |
| `POST` | `/v1/project/set-default-agent` | `SetProjectDefaultAgent` |
| `POST` | `/v1/project/export` | `ExportProject` |
| `POST` | `/v1/project/import` | `ImportProject` |
| `POST` | `/v1/agent/register` | `RegisterAgent` |
| `POST` | `/v1/session/create` | `CreateSession` |
| `POST` | `/v1/session/send-message` | `SendMessage` |
| `POST` | `/v1/session/fork` | `ForkSession` |
| `POST` | `/v1/run/get` | `GetRun` |
| `POST` | `/v1/run/cancel` | `CancelRun` |
| `POST` | `/v1/cron/create` | `CreateCron` |
| `POST` | `/v1/cron/set-enabled` | `SetCronEnabled` |
| `POST` | `/v1/cron/trigger` | `TriggerCron` |
| `GET` | `/v1/workspace/snapshot` | `Snapshot` |
| `GET` | `/v1/settings` | `GetSettings` |
| `POST` | `/v1/settings/save` | `SaveSettings` |
| `POST` | `/v1/settings/reset` | `ResetSettings` |
| `GET` | `/v1/event/list` | durable event SSE replay |
| `GET` | `/v1/metric/list` | in-process metric snapshot |

例如注册 Project：

```bash
curl -X POST http://127.0.0.1:7314/v1/project/register \
  -H 'content-type: application/json' \
  -d '{"id":"project-1","name":"Demo","workdir":"/absolute/workdir"}'
```

请求体中不再需要 `{"type":"register_project", ...}`。所有业务响应继续使用版本化的
`Response` envelope，durable event 与 SQLite 事务语义不变。

## 边界

`ait-contracts::Command` 和 `LocalControlService::execute` 仍是进程内、传输无关的应用层
分派契约，不是公开 HTTP endpoint。HTTP adapter 负责把每个独立请求 DTO 转换成对应
command，因此领域规则和事务逻辑不会在路由中复制。CLI 仍可接受 command JSON 作为通用
输入格式，但会移除 `type` 并请求对应的实体操作路由。

旧的 `/v1/commands`、`/v1/events` 和 `/v1/metrics` 不保留兼容别名。当前 API 尚未公开
发布，保留旧入口会延续两套边界并使调用方继续依赖统一 command。客户端必须迁移到上表
路由。

## 可观测性

为避免继续暴露“单一 command 入口”的概念，HTTP 指标改名为
`api_operations_total`、`api_operation_duration_ms_total`、`api_operation_errors_total`，
完成日志事件改为 `operation.completed`；关联 ID 与 operation 名仍按原语义记录。
