# 独立 server：使用与协议

`server` 与现有 daemon 并存，当前支持本机服务、WebSocket 与独立 Git 项目的打开、查询和关闭。
内部代码全部来自新建的八个 `server-*` package；尚未实现 Agent、Session、Run 或 Provider。
项目必须使用独立 clone，不能与旧 daemon 共管同一目录或共享 Git worktree。

## 启动

```sh
export AIT_SERVER_TOKEN="$(openssl rand -hex 32)"
cargo run -p server-bin --bin server -- --listen 127.0.0.1:7316
```

凭据只从 `AIT_SERVER_TOKEN` 读取，要求 32–256 字节可见 ASCII、无空格；请使用随机值。
不支持命令行 token、URL token、自动加载 `.env` 或配置文件中的 token。
当前适用于能够设置 Authorization header 的本机程序客户端；浏览器原生 WebSocket API 的
登录流程尚未实现。

默认目录为 `~/.ait-server`。配置优先级为命令行 > 环境变量 > TOML > 默认值。
目录只由 `--data-dir` / `AIT_SERVER_DATA_DIR` / `HOME` 决定，不能由 TOML 反向修改。
可用参数：`--data-dir`、`--listen`、`--config`、`--log-level`、`--help`、`--version`。
对应环境变量为 `AIT_SERVER_DATA_DIR`、`AIT_SERVER_LISTEN`、`AIT_SERVER_LOG_LEVEL`。

默认读取 `<data-dir>/config.toml`（不存在则跳过）；显式 `--config` 指定的文件必须存在。
仅支持如下非秘密字段，其他字段或错误语法会拒绝启动：

```toml
listen = "127.0.0.1:7316"
log_level = "info"
```

允许 IPv4/IPv6 loopback；拒绝 `0.0.0.0` 和其他远程地址。端口 `0` 可用于隔离测试，日志与
`server.info` 返回实际端口。日志只记录 HTTP method、状态、耗时及启动地址，不记录
Authorization、请求内容和 URL query。配置解析错误不回显 TOML 内容。

启动先绑定端口，再初始化新目录和 `catalog.sqlite3`。`instance.lock` 和 `server-id` 分别用于实例锁与身份：
前者用 OS 文件锁排他持有到正常关闭完成，后者原子写入并在重启后保留。每次启动生成新的
`instance_id`。损坏的身份文件会使启动失败，不能默默重建。锁文件不会被删除；进程结束后
由 OS 释放锁。Unix 上新建目录使用 0700 权限。

## HTTP 与升级

| Endpoint | 凭据 | 行为 |
| --- | --- | --- |
| `GET /healthz` | 无 | 最小存活状态 |
| `GET /readyz` | 无 | ready 时 200，draining 时 503 |
| `GET /v1/server/info` | Bearer | 稳定/实例身份、实际地址、版本、capability、预算 |
| `GET /v1/ws` | Bearer | 校验后升级 WebSocket；draining 拒绝新连接 |

```sh
curl http://127.0.0.1:7316/healthz
curl -H "Authorization: Bearer $AIT_SERVER_TOKEN" http://127.0.0.1:7316/v1/server/info
```

所有路由校验 Host：仅接受实际监听 authority 或 `localhost:<同端口>`。
Origin 可以缺省（本机原生客户端）；提供时必须是相同允许 authority 的 HTTP origin。
重复的 Host/Origin/Authorization header 被拒绝，不开放 CORS。所有 query 被拒绝。

## WebSocket v1.0

升级后 10 秒内，客户端必须发送 JSON Text hello：

```json
{
  "type": "hello",
  "client_id": "local-client",
  "protocol": { "major": 1, "min_minor": 0, "max_minor": 0 },
  "capabilities": ["server.info", "connection.ping", "server.status.subscribe"],
  "required_capabilities": []
}
```

major 必须为 1，minor 区间必须包含 0。未知 optional capability 被忽略，未知 required
capability 拒绝握手。服务端返回 `type=server_info`，包含 `info`、新的 `connection_id` 与
`negotiated_capabilities`。能力必须协商后才能调用。`client_id` 仅用于诊断，不用于身份、
接管、去重或订阅共享；即使重复，两个物理连接也拥有不同的 connection ID。

RPC 格式：

```json
{"type":"request","request_id":"r1","method":"connection.ping","params":{"nonce":"n1"}}
```

```json
{"type":"response","request_id":"r1","result":{"nonce":"n1"}}
```

| Method | Params | Result |
| --- | --- | --- |
| `server.info` | 空 | 与 HTTP info 相同 |
| `connection.ping` | `nonce`，1–128 字节、无控制字符 | 回显 nonce |
| `server.status.subscribe` | 空 | 新的 `subscription_id`；随后发送 ready 状态 |
| `server.status.unsubscribe` | `subscription_id` | `unsubscribed: true`；只能取消本连接订阅 |

状态通知形如 `{"type":"status","subscription_id":"…","lifecycle":"ready"}`。
每连接最多 16 个订阅，断开即清理，重连需要重新订阅。退出时 best-effort 发出 `draining`。
这是临时连接通知，没有持久序号/回放语义；业务 outbox 与 cursor 留到后续 M1 切片。

错误形如 `{"type":"error","request_id":"r1","code":"method_not_found","message":"Unknown method","retryable":false}`。
尚未实现的业务 method 返回 `method_not_found`，不返回空成功。有效请求的错误保留 request ID。
不合法 envelope、重复 hello、二进制消息与握手失败会关闭连接；未知 method 或错误参数可修正后
继续使用当前连接。请求按连接顺序处理，已响应的 request ID 可以再次使用；它不是幂等键。

## 项目操作（M1 首个切片）

在 hello 的 `capabilities` 或 `required_capabilities` 中加入所需的
`project.open`、`project.list`、`project.get`、`project.close`。

| Method | Params | Result |
| --- | --- | --- |
| `project.open` | `path`：绝对路径；`idempotency_key` | 稳定 `operation_id`、`project_id` |
| `project.list` | `{}` 或 `after`、`limit`（1–50，默认 20） | `projects`、`next_after` |
| `project.get` | `project_id` | 项目摘要及本进程的 `owner_epoch` |
| `project.close` | `project_id`、`owner_epoch`、`idempotency_key` | 稳定 `operation_id`、`project_id` |

```json
{"type":"request","request_id":"open-1","method":"project.open","params":{"path":"/absolute/path/to/independent-clone","idempotency_key":"open-project-1"}}
```

摘要包含规范路径、初始名称、冻结的 `base_commit`、`root_message_id`、`created_at`（Unix 毫秒）。
`owner_epoch` 非空表示本进程持有项目；null 表示本进程没有持有，不能推断其他进程的状态。
get/list 读取可重建 catalog，不隐式接管项目。`next_after` 作为下一页 `after`；完整末页后
可能再返回一个空页。

第一次打开要求该目录本身是具备 HEAD 的独立 Git 根；不自动 init 或创建 commit。
所有 linked worktree、带额外 worktree 的主检出、旧 `.ait` 项目及其内部目录均被拒绝。
初始根 system Message 快照来自本目录 `AGENTS.md`（最多 128 KiB，缺省为空，拒绝 symlink）；
以后修改指令、目录名或 HEAD 都不会改写已保存的根 Message 和 Git 基线。

新增状态：

```text
<data-dir>/catalog.sqlite3
<project>/.ait-server/project.sqlite3
<project>/.ait-server/project.lock
HOME/.ait-server-project-locks/<project-id>.lock
HOME/.ait-server-project-locks/<project-id>.epoch
```

server 会向项目 `.git/info/exclude` 追加 `/.ait-server/` 并验证忽略结果。已有 tracked
runtime 文件或仓库规则覆盖排除时拒绝准入。项目数据库、sidecar 和锁路径不能是 symlink。
失败后可能保留 runtime 目录、锁、排除规则或已提交的数据库，重试会继续处理，不自动清理。

`idempotency_key` 为 1–128 字节无空白 ASCII，在一个 catalog 内按 method 去重。连接断开
或结果不明时使用原 key 重试；不同规范参数复用同 key 会返回 `idempotency_conflict`。
完成回执只说明该操作已经提交，当前是否打开必须用 get/list 查询。

- 同 key 重放 open 不会重新打开已关闭项目；重新打开需新 key。
- close 携带 get/list 返回的当前 epoch；新操作使用过期 epoch 返回 `stale_owner`。
- 重放已完成 close 会直接返回旧回执，不会关闭后来重新打开的项目。
- 重启后 catalog 项目默认未打开；未完成的打开意图通过原 key 显式重试恢复。
- 不同 data-dir 的实例仍共用 HOME 下的 Project ID 锁；同一用户的实例必须保持一致 HOME。
- `.epoch` 保存跨副本的本机代次上限，损坏时拒绝接管；保留它，不要当作临时文件清理。
- 同时只接纳一个短项目操作，争用返回可重试的 `resource_exhausted`；客户端做退避重试。

常见业务错误：`unsupported_workspace`、`legacy_project`、`project_busy`、`unsupported_format`、
`identity_conflict`、`project_not_found`、`project_not_open`、`stale_owner`、`project_io`。
错误只提供安全信息，既不输出数据库诊断，也不回显凭据或指令内容。

## 预算和停止

- 输入 JSON message 和单帧均上限 1 MiB；包括分片累计大小。
- 最多 64 个同时升级的连接，超限 HTTP 429；待 hello 的连接同样占名额。
- 每连接待发送队列最多 256 条，消息内容合计 4 MiB，正在写入的内容仍占字节预算。
- 队列满立即终止慢连接；一次写入最多等 5 秒，最后排空/Close 最多 1 秒。
- Unix 支持 SIGINT/SIGTERM；Windows 使用 Ctrl-C。停止先关闭接纳，再排空 HTTP 和已跟踪
  WebSocket 和已接纳的项目操作；排空预算为 15 秒。超时记录关闭错误，非零退出。

客户端断开不会撤销已接纳的项目事务；服务停止会等待这些任务。关闭期限超时时返回错误，
仍在执行的阻塞任务会继续持有 data-dir 锁，直到完成或进程结束。Tokio 的阻塞工作不能强行取消，
因此此时进程实际退出可能晚于 15 秒；当前没有硬终止阻塞线程的机制。后续 Run 生命周期仍独立于连接。
Linux/Windows 的平台验收以 CI/后续实测为准；本次本地报告记录具体已验证平台。

## 开发验证

```sh
cargo test -p server-protocol -p server-api -p server-bin -p server-domain -p server-ports -p server-application -p server-storage -p server-workspace
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo llvm-cov --workspace --html
```

`server-bin/tests/dependencies.rs` 在 workspace 测试中读取 `cargo metadata --no-deps --locked`，
检查所有新 package 的声明依赖（包括 optional、平台条件、dev/build 和重命名依赖），拒绝旧
内部组件及违反 ADR-022 的依赖方向。未来新增 crate 必须同时更新边界表和本用例。

设计与后续工作见 [ADR-022](../decisions/adr-022-independent-server.md) 和
[实施计划](../plans/independent-server.md)。
