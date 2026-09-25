# Paseo 前端 Rust 接口适配与剩余缺口

2026-09-25：前端 TCP 连接已从原版 Paseo wire 格式适配到 Rust server。真实 Paseo SDK
通过修改后的 renderer transport、桌面主进程 transport manager 连接隔离 Rust binary，
基础 RPC、临时项目/Workspace、文件列表和标签更新事件通过。没有启动完整 Electron UI。

## 前端修改

- 主机探测和长期连接共用 `/v1/ws`、Rust hello 与能力协商。
- 桌面主进程支持带 Bearer header 的 loopback TCP；token 独立于 URL/subprotocol。
- 全部 205 个上游消息名称映射到 202 个规范方法；转换请求、响应、错误、事件、旧状态
  响应及 liveness ping。保留业务字段，包括 `messageId` 和 permission requestId。
- 四条功能连接解决单条 hello 的 64 项能力限制；关联响应、订阅释放和 binary 数据保留
  实际连接归属，失败时整体关闭。详见 [ADR-044](../decisions/adr-044-paseo-client-rust-transport.md)。
- 修复 Electron bridge 把 `ws` 的文本 Buffer 误标记为二进制的问题。

入口：[主机探测](../../apps/app/src/utils/test-daemon-connection.ts)、
[运行时](../../apps/app/src/runtime/host-runtime.ts)、
[协议适配器](../../apps/app/src/runtime/rust-server/transport.ts)、
[主进程 transport](../../apps/paseo/src/daemon/local-transport.ts)。

## Rust 侧优先缺口

| 优先级 | 行为 | 实测或来源 | 影响 |
| --- | --- | --- | --- |
| P0 | `agent.message.send.request` 接受 SDK 消息身份 | SDK 自动携带 `messageId`；Rust `only()` 仅允许 `agentId`/`text`，返回 `unsupported_capability` | 原始 SDK 的发送消息调用无法成功；需要实现消息身份/幂等语义，不能在前端丢弃该字段 |
| P0 | `agent.list.request` 的 subscribe/sync | `observeAgents()` 返回 `unsupported_capability` | Agent 列表实时观察未接通 |
| P0 | `workspace.list.request` 的 subscribe/sync | `observeWorkspaces()` 返回 `unsupported_capability` | Workspace 列表实时观察未接通 |
| P1 | 完整 Session 事件生产者 | `agent_permission_request` 事件订阅失败；daemon config 订阅成功 | 当前只支持 Provider snapshot、Agent attention、server info、daemon config 四类 |
| P1 | 业务完整性 | 已有服务报告和参数校验：组合 Workspace+Agent/worktree、resume overrides、富消息附件、完整 Timeline 增量等仍有限制 | 不能把方法已注册当作业务与 Paseo 完全一致 |
| P2 | Schedule/Plugin/Hub/Browser 等占位 | `scheduleList()` 立即得到 `not_implemented`，没有伪成功或等待超时 | 可实现方法以生产 `implemented_capabilities` 为准 |
| P2 | 浏览器登录与远程连接 | Rust 只接受本机来源和 Authorization header；本次仅通过 Electron 主进程或原生 header transport 连接 | 普通浏览器、Rust relay/SSH/IPC 仍没有对应接入 |
| P2 | Session ping 的服务端时间戳 | Rust `connection.ping` 只回显 nonce | SDK liveness ping 已适配，要求服务端收发时间的 Session ping 明确拒绝 |

发送消息的具体入口见
[agent_execution.rs](../../crates/server-provider/src/rpc/agent_execution.rs) 的 `send()` 和 `only()`；
目录订阅分别见 [agent_runtime.rs](../../crates/server-provider/src/rpc/agent_runtime.rs) 和
[directory.rs](../../crates/server-metadata/src/rpc/directory.rs)。服务端返回的
`unsupported_capability` 文案是通用的 “Capability was not negotiated”；在上述测试中
方法已经协商成功，实际拒绝的是处理器尚未支持的参数或事件种类。

生产能力统计及全部 45 个占位仍见[接口清单](server-interface-gaps.md)。本次未修改 Rust
实现，也没有开启 Provider 执行或使用已有项目数据。

## 验证与复现

[验证脚本](../../scripts/validate-paseo-rust-client.cjs)在临时目录构建固定上游 SDK，
使用上游原始 `WSOutboundMessageSchema` 检查返回消息；未迁入的 AOT validator 由同源 Zod
schema 执行，未跳过消息校验。真实网络测试执行两侧 transport 和 RPC bridge 的代码，
IPC 由进程内测试回调连接，因此不等于 Electron preload/UI 端到端验收。

可通过 `PASEO_TEST_DEPS` 指向已安装的独立测试依赖目录，避免为这次接口测试迁入全部 Expo
依赖。脚本需要 esbuild、Vitest、Zod ^4.4.3、tweetnacl、base64-js、semver 和 ws；已有
`apps/desktop` 的 esbuild/Playwright 也可复用。本次使用 Vitest 4.1.11、Zod 4.6.5；
依赖来源和范围在脚本开头说明。

```sh
python3 scripts/check-paseo-client-methods.py
CARGO_TARGET_DIR=/tmp/ait-agent-interface-target cargo build -p server-bin --bin server --offline -j2
PASEO_SOURCE_ROOT=/path/to/paseo \
PASEO_TEST_DEPS=/path/to/test/node_modules \
AIT_SERVER_BIN=/tmp/ait-agent-interface-target/debug/server \
PASEO_VALIDATION_REPORT=/tmp/paseo-client-validation.json \
node scripts/validate-paseo-rust-client.cjs
```

本次已验证 20 项成功行为：连接、目录查询、daemon/config、Provider 查询、语音模式关闭、
liveness ping、标签快照/订阅/释放、临时项目注册与 Workspace 打开、非空 Workspace 列表、
文件和终端列表、标签赋值与实时更新事件。另有 5 项预期缺口断言，明确检查错误码；
这些断言通过不表示对应功能已经支持。全部接收消息的 schema 错误为 0。

4 个文件中的 36 项前端单元测试全部通过；适配器核心和桌面主进程 transport 的严格
TypeScript 检查通过。Rust format、Clippy 和完整工作区回归通过：99 个测试目标，
1,004 项通过、0 失败、5 项忽略。前端格式、方法清单和 Rust 回归的最终结果见
[验证制品](paseo-client-rust-adapter-validation.json)。测试 token 仅保存在内存中，
server 结束后删除临时数据目录。首次工作区测试在沙箱中出现 socket/进程权限错误，
随后按同样参数在允许本机进程与网络的环境重跑。

## Test coverage

本轮没有修改 Rust 行为，Rust 行覆盖率不单独测量。前端行覆盖率未测量；针对性测试
覆盖了握手、能力预算、并发关联、订阅所有权、错误、权限请求、听写路由、二进制路由、
超时和连接清理。未验证完整 GUI、真实 Agent 执行、语音识别/合成和所有业务参数组合。

完整桌面启动依然需要补齐上游 npm workspace 和根级构建脚本，并将内置 daemon 管理
改接 Rust binary。现阶段可手动启动 Rust server，在桌面的 direct TCP 主机配置中使用
其地址和 Bearer token（现有 password 字段）；完整产品流程仍待构建整合。
