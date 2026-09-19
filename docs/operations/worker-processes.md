# 受监督的 Run worker（NEC-248）

生产 daemon 使用私有协议 2.0 启动 `ait-worker --stdio --protocol-major 2`。
API Provider 经 `RunDispatcher` 注入；所有 Codex 请求通过同一个 `WorkerSupervisor` 的
原生 Thread writer、history、model catalog 和 title ports 进入 worker。HTTP/SSE、设置、
审批、SQLite 和 outbox 留在 daemon；worker 的正常依赖图不包含 storage-sqlite。
模型发现、历史查询和标题是独立辅助 scope，不创建伪 Run；daemon 不直接启动 app-server。

遵循 [ADR-017](../decisions/adr-017-unified-native-codex-worker.md)：新 Codex Session 使用固定
`<Project>/.ait/<session-id>` worktree；导入 Thread 保持原生 cwd。每 Run 临时 worktree、
变更回集、失败回滚及路径改写已经删除。Codex 的失败或取消不会回滚文件修改。
API Provider 的工具循环与 Cron Session 语义保留；Codex Cron 与原生 fork/steer 当前明确拒绝。

## 构建与启动

```sh
cargo build --locked -p ait-daemon -p ait-worker -p ait-cli
target/debug/ait-daemon --database ./ait.sqlite3 --listen 127.0.0.1:7314
```

两个可执行文件必须来自同一版本并放在同一目录。开发时可用 daemon 的
`--worker-binary /trusted/path/ait-worker` 指定位置。Desktop release workflow 同时编译、
stage 和打包二者；缺少 worker 时 staging 直接失败。不要手工将 worker 连接到终端；
stdout 的任何普通文本都会被判为协议污染。

macOS 的 Codex adapter 直接通过 `/bin/zsh -lic 'exec "$@"' -- codex app-server
--listen stdio://` 启动，模型发现、标题生成和 worker Run 共用此入口。zsh 加载
`.zprofile`、`.zshrc`，Codex 及其解释器/工具继承 shell 环境；daemon 不探测或转发 PATH。
可执行文件和额外参数作为独立 argv 传递，`exec` 让受管子进程直接成为 Codex。
shell 启动文件需保持 stdout 安静，以免污染 JSONL。开发版也使用此方式，其他平台直接启动。

## 提交与恢复

- v2 DTO 在 `ait-contracts/src/worker` 冻结独立字段；domain 与 SDK 类型只在进程内使用，
  `ait-ipc::mapping` 显式转换。每个 frame 是 u32 big-endian 长度和 UTF-8 JSON。
- `hello` 声明 minor 区间、支持/required capabilities、进程 PID 和 frame 上限；
  `hello_ack` 显式选择共同 minor、capability 交集和较小 frame 上限。每个 envelope 携带
  major/minor，协商后版本漂移会被拒绝；同 major 未知 optional field/capability 被忽略，
  未知 required capability 和消息 kind 继续失败。握手后每个 frame 还携带
  `scope_id + worker_instance_id + lease_epoch`；每方向 sequence 严格递增。
  RPC request ID 严格递增，ACK 必须同时匹配 request ID 和 operation ID。
- daemon 的 `ControlRunStore::commit_worker` 在同一 SQLite CAS 中提交 Message、工具状态、
  Run、Session 和 receipt。相同 operation ID/内容返回原 receipt；换内容、旧 lease、
  错误 Run 或非法转换被拒绝。工具 intent ACK 在执行前；已知 outcome ACK 在 ToolResult 前；
  terminal ACK 在 worker 的完成报告前。
- Codex Worker 创建或恢复持久 Thread 并持有 writer。daemon 先持久化输入 intent，再发送
  `Start`；发送前将状态置为 send-unknown。完成后通过 `thread/read` 元信息与
  `thread/turns/list(itemsView:full)` 取得权威历史，关闭 writer 后原子发布 Message 与 Run。
  clientUserMessageId 是归因字段，不是重放保证。历史结果采用有序 16 KiB 分块，总量不超过
  64 MiB；writer ownership proof 与展示字段分开传递。
- API worker 异常退出最多重新 claim 三个 epoch，始终使用原 Run ID 和原
  `RunCoordinator`。已确认 Message/ToolResult 不再生成；结果未知的工具走原 reconcile
  策略，禁止猜测成功或盲目重放副作用。终态已提交而 ACK 丢失时，以 SQLite 为准。
- Codex 恢复只读取和对账原 Thread。queued 且未发送的输入明确终结；发送结果未知的输入
  不会重发。`codex.auto_commit` 默认关闭，在准入时冻结；启用后仅对成功 Run 独立收尾。
  精确 commit plan 在更新 Git ref 前持久化，崩溃或 ACK 丢失后复用原 commit ID。
  Git 失败保留模型的 completed 状态，CLI `ait run retry-commit --run-id …` 和桌面按钮
  只重试 Git。起始脏目录或变化的 Git 基线跳过自动提交，无变化也跳过；Message 不携带提交结果。

连续文本 delta 在 worker 中按同一 item 合并（4 KiB 或下一事件观察到 40 ms 间隔时 flush，
完整事件前强制 flush），避免每个字符的 IPC 往返。进度回放仍使用 daemon durable cursor 和既有 SSE 接口。慢 UI 消费者不会成为 worker
的提交 ACK 接收者；丢失的临时 delta 由权威历史/最终 Message 校正。

## 限制与权限

| 边界 | 当前上限/行为 |
| --- | --- |
| frame | 1 MiB；协商可以缩小，先检查长度再分配 |
| 收发队列 / 在途 RPC | 每方向 16；API mutation 串行；Codex 最多 16 个 port 调用 |
| 单连接写入 | 1 秒 flush deadline；2 秒排队加 flush deadline |
| 握手 / heartbeat | 3 秒握手；500 ms 心跳，3 秒失联 |
| Run wall-clock / drain | 300 秒；取消后 2 秒；daemon shutdown 最多等待 5 秒 |
| worker 数量 | 单 supervisor 最多 16；同 Run 同时只有一个 |
| 工具并发 / 输出 | 最多 4；64 KiB；API 工具参数最多 16 KiB |
| API steps / tokens | 固定 RunBudget：128 steps、1,000,000 tokens；跨恢复累计 |
| Codex items / tokens | 同一 native turn 最多 128 个不同 item；上报 usage 超过 1,000,000 即取消 |
| receipt / context pages | 每 Run 最多 8192 个 API mutation receipt；每条读取一项，累计最多 8192 项 |

权限在准入时冻结，启动时重查管理员 `--max-sandbox`。`ait-sandbox` 使用 NEC-247 的
capability-relative、拒绝 symlink 的文件工具，阻止绝对路径逃逸和 `..` 遍历。受控
shell 仅允许既有白名单命令、无扩展/重定向/后台任务；full_access 也不自动增加
API shell 能力。审批只授权原 snapshot 已允许的动作，不能提高 sandbox。

金额上限通过 daemon 的可选 `--max-run-cost-micros` 启用（百万分之一账单币种单位）。
worker 在任何新 Provider 调用前要求可核验的定价/费用契约；当前 API 与 Codex 不提供
该契约，因此启用金额上限会拒绝这些调用，确保不以猜测价格放行。已确认的工具结果和
已发布 Codex 历史的 Git 恢复仍可结算。该选项同时停用自动 AI Session 标题生成，避免辅助
调用绕过费用保护；手工命名仍可用。未启用时保留原 `RunBudget.cost_budget`/usage 行为，
不承诺金额上限；后续需要价格契约才能在指定金额内继续使用这些 Provider。token 上限
基于 Provider 上报，已发出的请求可能超出剩余额度。

## 取消与平台

普通取消先持久化，再通过私有管道传播。heartbeat timeout、控制 EOF、非零退出、协议
污染与硬 deadline 都关闭管道并回收子进程；daemon 的 durable state 决定失败/恢复/终态。
SIGINT/SIGTERM 停止新交互准入、记录取消并 drain。已经发布的模型结果与精确 Git plan
在下次启动时独立对账，不重放输入。

Unix 使用独立 process group；daemon 在 leader 正常退出后仍清理组内后代，worker
发现父管道 EOF 也清理自身组。Windows 使用 process-wrap 的 kill-on-close Job Object。
这是可信可执行文件及其正常继承进程树的边界，不是容器或不同用户的 OS 安全边界；
同 UID 的恶意替换 worker、主动脱离 process group 的可执行文件不在保证范围内。
Windows Job Object 分支需要 Windows 主机验证；现有 Desktop 发布目标仍是 macOS ARM64
与 Linux x86_64。实现依据见 [process-wrap 文档](https://docs.rs/process-wrap/10.0.0/process_wrap/)。

## 凭证与诊断

daemon 在 bootstrap 时从凭证存储取得当前 Provider 的最小 grant，仅通过私有 stdin
发送；worker argv 不包含 Run 数据或密钥。环境先清空，只保留 OS 路径/用户目录/临时目录
白名单；不继承 OPENAI/DEEPSEEK/CODEX key、DATABASE_URL 或任意应用环境。
Codex 使用其既有用户登录存储，worker 不复制登录文件进入 journal 或 Project export。

grant 的 Debug 固定为 `[REDACTED]`，管道双方拒绝回传 grant；协议错误没有原始 JSON、
stderr 或 SDK 文本。共享、可审计的 fail-closed 分类覆盖 private/access key、
credential/auth、URI user-info、PEM、常见 credential marker 以及超限/畸形 JSON；worker
在形成 assistant Message 前拒绝，daemon IPC store adapter 与 application persistence
adapter 又分别在 Message 和 ToolExecution intent 写入前复核。API 因而拒绝包含这些形态
的工具参数；native operation
只做有界脱敏展示，不伪装成 AIT ToolUse。worker stdout 只写 frame，稳定日志走 stderr；
生产 supervisor 丢弃子进程 stderr，避免第三方意外日志进入 daemon 日志。
此处保护的是运行凭证和已识别敏感参数，不能识别用户自行放入普通正文的任意秘密。

## 验收入口

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cd apps/desktop
npm run typecheck
npm test
```

- `bins/worker/tests/process_providers.rs`：真实 worker + 拆分 SQLite + 离线 OpenAI/DeepSeek/Gemini/MiniMax HTTP；
  ToolUse → ToolResult → final，18 个 API ACK kill 边界、durable receipt 重放/冲突/旧 fence，
  各类敏感 ToolUse 对全局/Project DB/WAL、事件、checkpoint、export 的回归。
- `bins/worker/tests/process_codex.rs`：真实 Worker 的新建/续聊、发送前关闭、完整历史读取与进程回收。
- `crates/application/tests/native_execution.rs` 与 workspace-local commit tests：固定 cwd、
  新输入、Git-only 重试、精确 commit 身份、HEAD/index/分支竞态和自己的锁恢复。
- `bins/worker/tests/completes_run.rs`、`credential_process.rs`、`crates/ipc` 单元测试：真实
  stdio/父管道 EOF、双向跨 minor、错误版本/能力、超限/畸形/序列回退/污染/非零退出/超时、后代回收、
  慢消费者和 argv/env/stderr 脱敏。
- `bins/daemon/tests/codex_http.rs`：生产 HTTP → dispatcher → worker → fake Codex，4000 个
  delta 的 cursor replay、启动恢复期间可用的 readiness 和未发送输入不重放。
  macOS 还覆盖精简 GUI PATH 下，登录 shell 提供的含空格安装路径、PATH 解释器、
  daemon 模型发现和完整 worker Run。
  runtime、host tools 与原生审批的回归继续执行；旧 NEC-212 每 Run worktree 结算已移除。

## API 工具审批

私有协议 2.0 要求 `tool-grants-v1`、`tool-interactions-v1` 与 `native-codex-v1`。Approval、提问与计划审阅 RPC 等待期间心跳/控制面继续服务；决定、单次 grant 消费及交互答案由 daemon application 事务完成。未消费授权在 worker lease 变化时过期，交互按原 ToolExecution ID 恢复，Running 工具的未知结果不重放。等待期限计入总墙钟预算，见 [审批手册](api-tool-approvals.md)、[NEC-290 ADR](../decisions/NEC-290/adr-001-api-tool-approval-grants.md) 和 [NEC-313 ADR](../decisions/NEC-313/adr-001-aligned-api-agent-tools.md)。
