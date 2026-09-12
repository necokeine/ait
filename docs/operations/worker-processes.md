# 受监督的 Run worker（NEC-248）

生产 daemon 为每个 Run 启动 `ait-worker --stdio --protocol-major 1`。
交互式 Send/Fork/Derive 和启动恢复使用同一个 `WorkerSupervisor`：API Provider 经
`RunDispatcher` 注入，Codex 经该对象实现的 `WorkspaceAgent` 注入。HTTP/SSE、设置、
审批、SQLite 和 outbox 留在 daemon；worker 的正常依赖图不包含 storage-sqlite。
模型目录发现、Session 标题生成仍是 daemon 的辅助操作，不属于 Run 执行。

## 构建与启动

```sh
cargo build --locked -p ait-daemon -p ait-worker -p ait-cli
target/debug/ait-daemon --database ./ait.sqlite3 --listen 127.0.0.1:7314
```

两个可执行文件必须来自同一版本并放在同一目录。开发时可用 daemon 的
`--worker-binary /trusted/path/ait-worker` 指定位置。Desktop release workflow 同时编译、
stage 和打包二者；缺少 worker 时 staging 直接失败。不要手工将 worker 连接到终端；
stdout 的任何普通文本都会被判为协议污染。

## 提交与恢复

- v1 DTO 在 `ait-contracts/src/worker` 冻结独立字段；domain 与 SDK 类型只在进程内使用，
  `ait-ipc::mapping` 显式转换。每个 frame 是 u32 big-endian 长度和 UTF-8 JSON。
- 握手验证主版本、双方 required capabilities、进程 PID 和 frame 上限。握手后每个
  frame 都携带 `Run ID + worker_instance_id + lease_epoch`；每方向 sequence 严格递增。
  RPC request ID 严格递增，ACK 必须同时匹配 request ID 和 operation ID。
- daemon 的 `ControlRunStore::commit_worker` 在同一 SQLite CAS 中提交 Message、工具状态、
  Run、Session 和 receipt。相同 operation ID/内容返回原 receipt；换内容、旧 lease、
  错误 Run 或非法转换被拒绝。工具 intent ACK 在执行前；已知 outcome ACK 在 ToolResult 前；
  terminal ACK 在 worker 的完成报告前。
- Codex 的 result checkpoint 与 integration claim 在既有 `WorkspaceRunJournal` 中原子
  保存 worker fence 和 operation receipt。RPC 重试缓存只负责同连接合并；跨进程恢复
  依赖 SQLite journal。native approval 使用既有稳定审批 ID 和持久决定；progress 仍是
  有界展示投影，不是新的领域状态机。
- API worker 异常退出最多重新 claim 三个 epoch，始终使用原 Run ID 和原
  `RunCoordinator`。已确认 Message/ToolResult 不再生成；结果未知的工具走原 reconcile
  策略，禁止猜测成功或盲目重放副作用。终态已提交而 ACK 丢失时，以 SQLite 为准。
- Codex 在 checkpoint 之后丢失 worker，立即走 NEC-212 的 recovery claim 和 Git
  settlement，不再次调用 Codex。HEAD、index、Run ref 或 rollback material 不满足
  恢复条件时保留材料并中断，释放 Session；不要手工重发相同任务来掩盖不确定结果。
  checkpoint 之前的孤立提交不能凭空转换成已确认答复。

连续文本 delta 在 worker 中按同一 item 合并（4 KiB 或下一事件观察到 40 ms 间隔时 flush，
完整事件前强制 flush），避免每个字符的 IPC 往返。进度回放仍使用 daemon durable cursor 和既有 SSE 接口。慢 UI 消费者不会成为 worker
的提交 ACK 接收者；丢失的临时 delta 由 checkpoint/最终 Message 校正。

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
Codex checkpoint/Git 恢复仍可结算。该选项同时停用自动 AI Session 标题生成，避免辅助
调用绕过费用保护；手工命名仍可用。未启用时保留原 `RunBudget.cost_budget`/usage 行为，
不承诺金额上限；后续需要价格契约才能在指定金额内继续使用这些 Provider。token 上限
基于 Provider 上报，已发出的请求可能超出剩余额度。

## 取消与平台

普通取消先持久化，再通过私有管道传播。heartbeat timeout、控制 EOF、非零退出、协议
污染与硬 deadline 都关闭管道并回收子进程；daemon 的 durable state 决定失败/恢复/终态。
SIGINT/SIGTERM 停止新交互准入、记录取消并 drain，已经取得 integration gate 的 Git
结算仍由原规则裁决。中途退出的 daemon 下次启动会扫描并恢复原 Run。

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
stderr 或 SDK 文本。API 拒绝包含敏感字段/凭证命令或超限的工具参数；native operation
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

- `bins/worker/tests/process_providers.rs`：真实 worker + SQLite + 离线 OpenAI/DeepSeek HTTP；
  ToolUse → ToolResult → final，18 个 API ACK kill 边界、durable receipt 重放/冲突/旧 fence，
  credential echo 对 DB/WAL、事件、checkpoint、export 的回归。
- `bins/worker/tests/process_codex.rs`：9 个 native checkpoint/integration/finished kill 边界，
  一次 Provider 调用、唯一 Git commit/Message、原 Run ID 和 Session 释放。
- `bins/worker/tests/completes_run.rs`、`credential_process.rs`、`crates/ipc` 单元测试：真实
  stdio/父管道 EOF、错误版本/能力、超限/畸形/序列回退/污染/非零退出/超时、后代回收、
  慢消费者和 argv/env/stderr 脱敏。
- `bins/daemon/tests/codex_http.rs`：生产 HTTP → dispatcher → worker → fake Codex，4000 个
  delta 的 cursor replay、启动恢复期间可用的 readiness 和唯一执行。
  既有 runtime、host tools、审批及 NEC-212 的故障/取消/Git settlement 测试继续执行。
