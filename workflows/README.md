# CLI 用户流程

这里按用户目标记录 AIT CLI 操作流程，作为后续校正命令设计和行为的验收依据。
每篇包含前置条件、实际命令、可观察结果、失败恢复方式及对应测试。
领域语义以 [ADR-001 v4](../docs/decisions/NEC-150/adr-001-core-domain-model-v4.md)
和 [文档索引](../docs/README.md) 中的后续修订为准；已知差距单独记录，不能把当前限制当成最终产品要求。

## 流程与自动化覆盖

| 编号 | 用户目标 | 自动化测试（WF-01～09 位于 `bins/cli/tests/workflows.rs`） |
| --- | --- | --- |
| [WF-01](01-register-project.md) | 接入工作目录、选择 Agent、创建 Session | `wf01_register_project_and_agent` |
| [WF-02](02-send-message.md) | 发送输入并查看 Agent 最终结果 | `wf02_send_message_and_inspect_agent_reply` |
| [WF-03](03-branch-and-rebind.md) | 从历史节点开分支、命名、切换 Agent | `wf03_branch_rename_and_rebind_session` |
| [WF-04](04-run-status-and-cancel.md) | 查看运行状态、取消活动 Run 并继续 | `wf04_observe_injected_provider_failure_and_continue`；活动取消另见 application 测试 |
| [WF-05](05-cron.md) | 保存、启停并触发一个定时 occurrence | `wf05_cron_occurrence_is_idempotent_and_independent` |
| [WF-06](06-events-and-restart.md) | 按游标续读事件并在重启后找回状态 | `wf06_replay_events_and_reopen_workspace` |
| [WF-07](07-export-import.md) | 导出 Project 并导入另一个本地工作空间 | `wf07_export_and_import_project_archive` |
| [WF-08](08-settings.md) | 修改设置、处理并发覆盖、恢复默认值 | `wf08_save_reset_and_recover_settings` |
| [WF-09](09-errors-and-scripting.md) | 在脚本中判断命令结果并处理输入错误 | `wf09_cli_diagnostics_do_not_mutate_workspace` |
| [WF-10](10-create-project-with-codex.md) | 空目录启动 daemon、接入项目、真实 Codex 生成 Rust Hello World 并提交 | `project_creation.rs::wf10_create_project_with_real_codex_and_commit`（手动启用） |
| [WF-11](11-deepseek-python-hello-world.md) | 从 .env 配置 DeepSeek 默认 Agent，生成并独立验证单文件 Python Hello World | `deepseek_workflow.rs::wf11_real_deepseek_python_hello_world`（手动启用） |
| [WF-13](13-api-provider-tool-loop.md) | OpenAI/DeepSeek 工具创建、读取与搜索文件，验证 Git/Message/Run 和重启查询 | `ait-application::api_tool_loop`（默认离线执行） |

```bash
cargo test -p ait-cli --test workflows
# 单独验证一个流程
cargo test -p ait-cli --test workflows wf03_
```

WF-01～09 的测试启动实际的 `ait-cli` 子进程，经随机 loopback 端口访问生产 HTTP router、application
service 和独立的临时 SQLite 文件；目录含空格、中文内容和换行也在覆盖范围内。
每个流程自行准备数据，通过 CLI 的退出码、stdout、文件和后续实体查询核对结果。
CLI 子进程有 20 秒测试超时，服务显式停止并等待退出，断言失败时也会取消服务。
测试不依赖已启动的 daemon、固定端口、用户数据、模型凭据或 jq。
现有 CI 的 `cargo test --workspace` 会自动包含这些测试。

这是 CLI 到持久化状态的验收；daemon 二进制的启动配置、真实 Codex 调用、worker 崩溃恢复、
长时间调度和附件字节搬迁不由这组测试验证。相关 daemon/adapter 测试仍保留原职责。
WF-10 单独覆盖真实 daemon 启动、Codex 生成、Cargo 运行和 Git 提交，
通过根目录的 [`./test_with_codex.sh`](../test_with_codex.sh) 一键构建并运行；
默认 CI 编译但跳过这项依赖模型凭据的测试。
WF-11 使用原生 DeepSeek Provider，由
[`./test_with_deepseek.sh`](../test_with_deepseek.sh) 读取本机 `.env` 并运行；
凭据解析和 Python 逻辑校验器测试默认执行，真实 DeepSeek 调用只在手动启用时执行。
`workflows.rs::wf11_stdin_commands_keep_credentials_out_of_diagnostics` 验证 CLI 管道输入和诊断脱敏。
运行 WF-11 的逻辑校验器测试需要 Python 3。

## 手工演练准备

需要 Rust stable、Git、Bash 或 Zsh；下面的手工示例使用 jq 提取响应 ID 和编辑完整 settings 文档；常规操作无需构造 JSON。
在仓库根目录的终端 A 执行：

```bash
cargo build -p ait-cli -p ait-daemon -p ait-worker
export AIT_REPO="$PWD"
export WF_ROOT="$(mktemp -d)"
export AIT_ENDPOINT="http://127.0.0.1:17314"
mkdir -p "$WF_ROOT/project"
set -o pipefail
ait() { "$AIT_REPO/target/debug/ait-cli" --endpoint "$AIT_ENDPOINT" "$@"; }
printf '演练目录：%s\n' "$WF_ROOT"
```

在终端 B 进入同一个仓库，将下面的 `演练目录` 替换成终端 A 打印的绝对路径，前台启动服务：

```bash
target/debug/ait-daemon --database '演练目录/ait.sqlite3' --listen 127.0.0.1:17314
```

看到监听地址后，在终端 A 执行 `ait project list`。若端口已被占用，给本次演练选择另一个端口，
同时修改 `AIT_ENDPOINT` 和 `--listen`。演练结束后在终端 B 按 Ctrl-C 停止本次服务。
数据库、响应文件、归档都放在 `$WF_ROOT` 下，位于 Project 的 Git 工作目录之外，避免让发送输入的 Git 检查失败。

先执行 WF-01，再按需执行其余流程；它们使用不同的 Session ID。重复一篇流程时应使用新的 ID，
或重新准备独立演练目录和数据库。不要对真实工作数据运行失败路径示例或设置重置。

## 公共输入输出约定

从 `ait --help`、`ait <实体> --help` 和 `ait <实体> <动作> --help` 发现全部参数。
`--endpoint` 是全局 flag，可以放在任意子命令层级。常用标量使用 flags；ID 是非空、不含控制字符的
不透明字符串（允许空格），路径按单个 shell 参数引用；固定枚举由 clap 校验。模型及 reasoning effort
来自动态 Provider 目录，具体组合由 daemon 校验，CLI 不复制目录或业务规则。

| 实体 | 动作 |
| --- | --- |
| `project` | `list`、`register`、`set-default-agent`、`export`、`import` |
| `agent-provider` | `list`、`save`、`discover-models`、`refresh-models` |
| `agent` | `list`、`create`、`update` |
| `session` | `list`、`create`、`set-agent`、`set-config`、`rename`、`set-title`、`send`、`fork`、`derive` |
| `message` | `list`（要求 `--project-id`） |
| `run` | `list`、`get`、`cancel`、`approval approve/deny/cancel` |
| `cron` | `list`、`create`、`enable`、`disable`、`trigger` |
| `settings` | `get`、`set`、`reset` |
| `event` | `list --after <cursor>`（durable SSE 回放） |

保留 `events`、`export`、`import` 顶层快捷入口，行为分别等同于 `event list`、`project export`、`project import`。
文本使用 `--text`、`--text-file <file|->`、`--text-stdin` 三选一，保留多行中文和反斜杠。
Provider secret 仅由 `--secret-stdin` 接收，不放 argv、shell history、JSON 模型文件或响应；见 WF-11。
实体 `--input <file|->` 只用于 Provider 模型数组、完整 settings values 或 Project archive，均不包含命令标签。
HTTP 映射见 [实体操作 API](../docs/decisions/NEC-166/entity-operation-http-api.md)。

实体操作和成功的 `project import` 输出一个 JSON 信封：

```json
{"api_version":1,"ok":true,"result":{"kind":"session","value":{"id":"示意，实际还有其他字段"}}}
```

上例只说明信封结构，不是完整 Session。业务拒绝的 `ok=false`，带有
`error.code/message/retryable`，不带 `result`。成功的 `export` 只写文件，stdout 为空；
`event list` 输出 SSE 文本。退出码细节见 WF-09。
发送后检查 `result.value.status` 和 `result.value.error`，不能用 `ok=true` 代替 Run 完成判断。
动态 Message/Run ID、Session version、settings revision 和 event cursor 都从返回值读取，不能手填猜测。

默认权限是 `read_only` / `on_request`。让 Codex 写代码前，按 [WF-08](08-settings.md#在代码写入前设置权限)
读取并保存完整 settings，将 sandbox 改为 `workspace_write`，再发送输入；[WF-10](10-create-project-with-codex.md) 已包含这一步。

## 后续校正清单

| 当前限制 | 用户期望与后续验收方向 |
| --- | --- |
| `create_session` 仍要求 `agent_id` | Project 默认 Agent 当前是建议值；省略 Agent 的体验需单独设计和测试 |
| 活动 Session 再次输入返回 `SESSION_BUSY` | ADR 要求进入现有 Run 队列；实现后需更新 WF-04 的当前行为断言并增加队列消费测试 |
| `event list` 单次最多默认回放 256 条，没有 CLI `--limit` 或持续订阅 | 用最后一个 `id` 续读；后续覆盖多页完整性、持续事件和错误帧的退出码 |
| Cron 配置和手动 occurrence 可用，daemon 没有持续到点调度循环 | 后续验证实际时钟触发、并发策略、misfire 和重启补偿；本目录不声称已支持 |
| 启用真实 Codex 和 AI 标题生成需要外部执行环境 | 手工流程使用 `builtin-codex`；WF-01～09 自动化通过 `WorkspaceAgent` port 注入 fake，WF-10 提供明确 opt-in 的真实执行测试 |
| API 工具首版为文件/搜索和有限命令；没有任意 shell 或自动 Git 提交 | WF-13 离线验证宿主工具循环，WF-11 显式启用真实 DeepSeek；修改保留为待审阅 Git diff |

修改流程时同步修改表中的测试，注明哪些行为是已实现契约、哪些是待校正差距。
添加新流程使用下一个 WF 编号，保持一篇 Markdown 对应一个用户目标；不要只记录命令清单。
