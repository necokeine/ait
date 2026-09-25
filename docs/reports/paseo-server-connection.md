# Paseo desktop 与 Rust server 连接实测

本报告记录接口适配前的基线。后续前端适配和最新 SDK 联调结果见
[前端 Rust 适配报告](paseo-client-rust-adapter.md)。

2026-09-25 实测结论：当前导入的 `apps/paseo` **不能直接连接** `bins/server`。
Rust server 能正常启动并处理基础请求；桌面构建环境、daemon 启动方式和线上消息协议
尚未完成适配。只修改端口或 WebSocket 路径不足以接通。

## 启动验证

| 操作 | 结果 |
| --- | --- |
| `npm --prefix apps/paseo run dev` | 退出码 1，缺少根目录 `scripts/dev-home.sh` |
| `npm --prefix apps/paseo run build:main` | 退出码 127，`tsc: command not found`；导入目录没有安装依赖 |
| `CARGO_TARGET_DIR=/tmp/ait-agent-interface-target cargo build -p server-bin --bin server --offline -j2` | 成功 |

静态检查另确认 `apps/paseo/tsconfig.json` 引用的根 `tsconfig.base.json` 不存在，
client、protocol 等共享 npm 包尚未导入。桌面内置 daemon 管理仍调用
`@getpaseo/server/daemon-control`，并启动该包的 Node supervisor；当前没有启动 Rust
`server` 的实现，见 [daemon-manager.ts](../../apps/paseo/src/daemon/daemon-manager.ts)
和 [runtime-paths.ts](../../apps/paseo/src/daemon/runtime-paths.ts)。

## 隔离连接探测

启动刚编译的真实 Rust binary，使用 `127.0.0.1:0`、临时数据目录和内存中生成的随机
token。客户端探测使用本地上游 `2c8e8a826810337492cc5a38bb0bbd705b6fb632` 的
`buildDaemonWebSocketUrl`、`createWebSocketTransportFactory`，以及从 AST 提取并执行的
`DaemonClient.sendHelloMessage` 和默认 capabilities。使用现有 Playwright 内置的 `ws`
发起实际网络连接，不需要安装新的依赖。

这是传输与消息兼容性探测，**没有启动 Electron 界面，也没有运行完整 DaemonClient**。
HTTP Authorization 和 Origin 的部分组合是为定位阻塞而注入的诊断条件。

| 探测 | 实际结果 |
| --- | --- |
| `/healthz`、`/readyz`、已鉴权 `/v1/server/info` | 均为 HTTP 200 |
| Paseo 默认 `/ws`，不带凭据 | HTTP 401，先被鉴权中间件拒绝 |
| Paseo 默认 `/ws`，带正确 Bearer header | HTTP 404 |
| 改为 `/v1/ws`，仅使用 Paseo 浏览器密码 subprotocol | HTTP 401；server 要求 Authorization header |
| `/v1/ws`，正确 Bearer header + `Origin: http://localhost:8082` | HTTP 403；server 不接受 Metro 的跨端口 Origin |
| `/v1/ws`，正确 Bearer header、无 Origin，发送实际 Paseo hello | HTTP 101 后返回 `invalid_message` |
| Rust 原生 hello + ping、server info、project list、workspace list、daemon status | 握手与 5 个 RPC 全部成功 |
| Rust 原生握手后发送 Paseo `session` 业务消息 envelope | 返回 `invalid_message` |

本次 server 公布 209 个 capability，其中 164 个已实现。该数量不表示原版 Paseo
客户端能够直接使用这些方法。基础查询在空的隔离数据目录中执行，project/workspace
列表为空属于预期结果。

## 必须适配的接口层

| 层次 | Paseo 客户端 | 当前 Rust server |
| --- | --- | --- |
| 路径 | `/ws` | `/v1/ws` |
| 桌面 TCP 鉴权 | renderer WebSocket，通过 `paseo.bearer.*` subprotocol 携带密码 | HTTP Authorization Bearer |
| hello | `clientId`、`protocolVersion`、capabilities 对象 | `client_id`、`protocol` 版本区间、capabilities 名称数组 |
| server info | `session → status → payload.status=server_info` | 顶层 `type=server_info`，包含 `info` |
| 业务消息 | `{type:"session", message:{type, requestId, ...}}` | `{type:"request", request_id, method, params}` |

桌面 TCP 路径使用 renderer WebSocket，见
[test-daemon-connection.ts](../../apps/app/src/utils/test-daemon-connection.ts)。Rust 路由、
来源检查和握手分别见 [lib.rs](../../crates/server-api/src/lib.rs)、
[auth.rs](../../crates/server-api/src/auth.rs) 和
[connection.rs](../../crates/server-api/src/connection.rs)。

下一步需要补齐前端 workspace 的构建依赖，接入 Rust binary 的进程管理，并在明确的
客户端或服务端适配层处理鉴权、握手、请求、响应和订阅事件。原生协议的对照成功不能
作为桌面连接成功的证明。

## 验证范围与清理

[探测结果](paseo-server-connection-validation.json)保存实际状态码和脱敏响应。
本机临时探测脚本为 `/tmp/ait-paseo-connection-probe/probe.cjs`；运行命令为
`node /tmp/ait-paseo-connection-probe/probe.cjs`。本机端口监听需要沙箱外权限。
首次探测发现无凭据的 `/ws` 先返回 401，修正探测预期后完整执行通过；探测断言通过
代表确认上述兼容性失败和原生协议成功，不代表桌面联调通过。

测试 server 已收到 SIGTERM 并以 0 退出，临时数据目录已删除；token 未写入报告或日志。
部分 WebSocket 由探测器取得所需响应后主动终止，结果中的 1006 关闭码不能据此判定
server 异常。没有修改客户端或 Rust 实现，也没有接触已有项目数据。

Test coverage：本次新增的是运行期连接探测，未测量行覆盖率；没有进行 GUI、Agent
执行、语音、听写或完整业务回归。Rust 仅重新编译 binary；格式、Clippy 和工作区测试
的既有导入验证结果见 [导入报告](paseo-client-import.md)，本次未重复运行。
