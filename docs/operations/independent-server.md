# 独立 server：M0 使用与协议

`server` 与现有 daemon 并存。当前实现进程生命周期、服务信息与 WebSocket 连接能力；
不创建 Project、Session、Run，不调用 Provider 或旧 `ait-worker`，也不打开旧数据库。
内部代码只有本次新建的 `server-bin`、`server-api` 和 `server-protocol`。

## 启动

```sh
export AIT_SERVER_TOKEN="$(openssl rand -hex 32)"
cargo run -p server-bin --bin server -- --listen 127.0.0.1:7316
```

凭据只从 `AIT_SERVER_TOKEN` 读取，要求 32–256 字节可见 ASCII、无空格；请使用随机值。
不支持命令行 token、URL token、自动加载 `.env` 或配置文件中的 token。
M0 适用于能够设置 Authorization header 的本机程序客户端；浏览器原生 WebSocket API 的
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

启动先绑定端口，再初始化新目录。目录内只有 `instance.lock` 和 `server-id` 是 M0 状态：
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
这是临时连接通知，没有持久序号/回放语义；业务 outbox 与 cursor 留到 M1。

错误形如 `{"type":"error","request_id":"r1","code":"method_not_found"}`。
尚未实现的业务 method 返回 `method_not_found`，不返回空成功。有效请求的错误保留 request ID。
不合法 envelope、重复 hello、二进制消息与握手失败会关闭连接；未知 method 或错误参数可修正后
继续使用当前连接。请求按连接顺序处理，已响应的 request ID 可以再次使用；它不是幂等键。

## 预算和停止

- 输入 JSON message 和单帧均上限 1 MiB；包括分片累计大小。
- 最多 64 个同时升级的连接，超限 HTTP 429；待 hello 的连接同样占名额。
- 每连接待发送队列最多 256 条，消息内容合计 4 MiB，正在写入的内容仍占字节预算。
- 队列满立即终止慢连接；一次写入最多等 5 秒，最后排空/Close 最多 1 秒。
- Unix 支持 SIGINT/SIGTERM；Windows 使用 Ctrl-C。停止先关闭接纳，再排空 HTTP 和已跟踪
  WebSocket；总等待最多 15 秒，超时返回非零并退出进程。

M0 没有业务任务，客户端断开只清理连接资源。后续 Run 生命周期不能用此连接生命周期代替。
Linux/Windows 的平台验收以 CI/后续实测为准；本次本地报告记录具体已验证平台。

## 开发验证

```sh
cargo test -p server-protocol -p server-api -p server-bin
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
