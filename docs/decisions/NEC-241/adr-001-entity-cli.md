# ADR-001：按实体分组的类型化 CLI

- 状态：Accepted
- 日期：2026-09-11
- 关联：NEC-241
- 依赖：ADR-001 v4、NEC-152、NEC-166、ADR-009、NEC-208、NEC-234

## 背景

HTTP 已按实体/操作拆分，但 CLI 的多数写操作仍让用户手写内部 Command DTO。
这导致帮助不完整、常规标量错误推迟到 JSON 反序列化，也容易把 Provider 凭据放入 shell 历史。
这是外部 CLI 边界变更；application Command 与领域语义不需要改变。

## 决策

删除 `CliCommand::Command`、`parse_command` 和通用 tagged-command CLI 入口，不保留隐藏别名。
CLI 只做参数校验、输入读取、DTO 构造和现有 HTTP 调用。所有操作可由每一级 `--help` 发现。

| Application Command | CLI |
| --- | --- |
| `RegisterProject` | `project register --id --name --workdir [--repo-url]` |
| `SetProjectDefaultAgent` | `project set-default-agent --project-id --agent-id` |
| `RegisterAgent` | `agent create --id --name --provider-id --model [--reasoning-effort]` |
| `UpdateAgent` | `agent update --id --name --provider-id --model [--reasoning-effort]` |
| `SaveAgentProvider` | `agent-provider save --id --name --kind [--url] [--input] [--secret-stdin]` |
| `DiscoverProviderModels` | `agent-provider discover-models`（同 save 的输入 flags，无保存副作用） |
| `RefreshProviderModels` | `agent-provider refresh-models --provider-id` |
| `SetSessionConfig` | `session set-config --session-id --provider-id --model [--reasoning-effort]` |
| `CreateSession` | `session create --id --project-id --agent-id [--at-message-id]` |
| `SetSessionAgent` | `session set-agent --session-id --agent-id` |
| `RenameSession` | `session rename --session-id --name` |
| `SetSessionTitle` | `session set-title --session-id --title` |
| `SendMessage` | `session send --session-id` 加一个文本输入 |
| `ForkSession` | `session fork --id --project-id --agent-id --at-message-id` 加一个文本输入 |
| `DeriveSession` | `session derive --id --project-id --source-session-id --agent-id --at-message-id` 加一个文本输入 |
| `GetRun` | `run get --run-id` |
| `CancelRun` | `run cancel --run-id` |
| `ResolveNativeApproval` | `run approval approve/deny/cancel --run-id --approval-id`，approve 必须指定 `--scope` |
| `CreateCron` | `cron create --id --name --project-id --base-message-id --agent-id --schedule --timezone` |
| `SetCronEnabled` | `cron enable/disable --cron-id` |
| `TriggerCron` | `cron trigger --cron-id --scheduled-at` |
| `ExportProject` | `project export --project-id --output` |
| `ImportProject` | `project import --input --workdir` |
| `GetSettings` | `settings get` |
| `SaveSettings` | `settings set --expected-revision --input` |
| `ResetSettings` | `settings reset` |
| `ListProjects` | `project list` |
| `ListAgents` | `agent list` |
| `ListAgentProviders` | `agent-provider list` |
| `ListSessions` | `session list --project-id` |
| `ListMessages` | `message list --project-id` |
| `ListRuns` | `run list --project-id` |
| `ListCrons` | `cron list` |

durable SSE 使用 `event list --after <u64 cursor>`。原有 `events`、`export`、`import`
快捷入口继续公开，分别复用实体入口的映射与输出逻辑，不接收 Command JSON。

### 输入边界

- clap 校验必填参数、互斥输入、Provider kind（Mock 仅在显式开发 feature 的 debug 构建可用）、approval scope、布尔开关、u64 revision/cursor、
  i64 Unix 毫秒时间戳，以及不含控制字符的非空 ID。ID 保持不透明字符串，允许空格，不强制 UUID。
  路径是 `PathBuf`，由 shell 引用为一个参数；目录存在性和 Project/Git 授权仍属于 application。
- Model ID 和 reasoning effort 是动态 Provider 目录值，CLI 只校验非空；组合合法性仍由 daemon
  判断，避免复制 adapter 能力目录。Cron schedule/timezone 的语义也由现有业务校验负责。
- Send/Fork/Derive 使用互斥的 `--text`、`--text-file <file|->`、`--text-stdin`。
  文件/stdin 必须是 UTF-8，完整保留换行、反斜杠和中文，不 trim 消息。
- `agent-provider save/discover-models --input <file|->` 只接受模型数组（id/name/reasoning_efforts）。
  `settings set --input` 只接受完整 values 文档，revision 必须通过单独的 typed flag 传入。
  `project import --input` 只接受 Project archive。不能用这些入口输入 tagged transport Command。
- Provider secret 仅用 `--secret-stdin`，不提供 secret 值参数或环境变量入口。要求 stdin 重定向，
  拒绝会回显的终端输入；去掉一个末尾 LF/CRLF，拒绝空值。省略时沿用现有保存语义。
  模型数组不能同时占用同一个 stdin，模型文件可与 secret stdin 一起使用。
  终端属性由真实 CLI 入口检测并和 reader 一起传入，解析层不读取进程全局 stdin；测试可明确注入终端或重定向来源。
- 读取/解析错误不打印输入内容；JSON 错误只给行列。携带 secret 的请求若传输、HTTP 或响应解码失败，只输出通用诊断，
  不显示可能引用远端字段值的底层错误。HTTP 请求不记录 body，ProviderSecret 的
  Debug 已有脱敏。CLI 额外对响应中的已知 secret 做字符串级脱敏并重新序列化，保护上游意外回显，
  同时保持合法 JSON。凭据仍由现有 gateway 保存，不进入 Agent、SQLite 状态、事件或归档。

### 行为与权限

保留 JSON 成功/业务错误信封：成功退出 0；业务拒绝退出 2；clap 参数错误退出 2；本地输入、
文件和传输错误退出 1。Export 成功只写文件；失败不覆盖目标文件。SSE 保持文本输出，默认有限回放
256 条，不新增持续订阅。`--endpoint` 成为真正全局参数，可放在任何子命令层级。

Send/Fork/Derive 复用原同步 HTTP 路由；操作成功返回 Run 不代表 Run completed。
帮助与流程要求读取 `status` 和 `error`。网络断开后先查询实体状态再决定是否重发；
不增加自动重试，不改 Session busy、Git dirty、settings revision CAS 或 Cron occurrence 去重规则。

原生审批的 grant scope 必须显式选择：命令/文件请求支持 one-shot/session，权限档案请求支持
turn/session；session grant 受管理员策略限制。CLI 固定枚举对应领域 `OneShot/Turn/Session`，
实际 scope、Run 权限快照和授权目标约束仍由 daemon 验证。deny/cancel 不接受 scope。

默认 settings 为 `permissions.sandbox=read_only`、`permissions.approval=on_request`。
代码写入前先 `settings get`，保留完整 values 和最新 revision，将 sandbox 改为 `workspace_write`，
再 `settings set --expected-revision <revision> --input <values文件>`，之后才能发起新的 Run。
权限在 Run 准入时固定，受管理员上限约束；`strict` 是 read_only 别名，旧 approval=always
会使 Codex 准入失败。普通 API Provider 当前只生成文本，权限上限不代表新增了文件/进程工具。

## 验证与后果

- 33 个原 Command variant 均有显式 argv→DTO 期望值测试。测试通过 syn 读取真正的 enum 语法，
  将 variant 集合与覆盖集合比较，新增 variant 必须增加实际 CLI 调用案例。
- 递归验证每一级帮助，覆盖缺参、非法 enum/时间戳/游标/revision、全局 endpoint、stdin、中文多行、
  含空格路径、模型 JSON 诊断及 secret 不出现在响应、事件、数据库文件中。
- NEC-166 路由表与真实 router 的 Method/Path 集合自动比对，全部 CLI 映射必须引用表内 endpoint。
- WF-01～WF-11 的文档和 CLI 进程调用全部迁移。WF-10 显式配置 workspace_write；WF-11 只通过
  secret stdin 传凭据，并实际创建、读取含空格路径的多行 prompt 文件。真实 Provider 测试继续 opt-in，普通 workspace 测试不代表真实模型验收。
- 不修改 domain/application/HTTP 的领域不变量与持久化行为；domain 依赖保持纯净，依赖继续向内。
- 旧通用 CLI 脚本必须迁移；复杂输入仍是实体文档，需要随对应契约演进。现有终端 alias 函数可继续使用。
