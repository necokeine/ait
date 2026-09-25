# ADR-044：Paseo 前端适配 Rust server 协议

- 状态：Accepted；方法范围经 ADR-045 修订为 186 个上游名称 / 183 个规范方法，第四条连接为 28 项能力，下文计数保留原始决策记录。
- 日期：2026-09-25。
- 范围：`apps/app` 的 TCP 客户端连接层和 `apps/paseo` 的 Electron transport bridge。
- 基线：Paseo `2c8e8a826810337492cc5a38bb0bbd705b6fb632`、Rust WebSocket v1.0。

## 背景

导入的 Paseo 使用 `/ws`、camelCase hello、`session` 消息封装和浏览器密码 subprotocol。
独立 Rust server 按 ADR-026 使用 `/v1/ws`、Bearer header、版本区间与能力协商、统一
request/response/error。直接替换端口无法连接，见[首次实测](../reports/paseo-server-connection.md)。

## 决策

在前端的 `runtime/rust-server` 增加传输适配器，保留 UI 使用的 DaemonClient API。
主机探测和长期运行时的 direct TCP 连接统一使用 `buildRustClientConfig`，请求路径为
`/v1/ws`。205 个上游消息名称映射到 Rust catalog 的 202 个规范方法；独立脚本校验
名称及消息方向。响应恢复原始 SDK 类型和 requestId，Rust 错误转成 SDK 的 `rpc_error`。
连接级错误、未知消息和不可用能力不得伪装为成功。

Electron 的 Rust TCP 连接经现有 IPC bridge 交给主进程的 `ws` 客户端。主进程只允许
当前 Rust server 支持的显式 loopback endpoint，token 单独通过 IPC 参数传递，再写入
Authorization header；不写入 URL/subprotocol，也不转发 renderer Origin。原生移动端
使用可设置 header 的 transport。普通浏览器缺少 Rust 登录流程，连接时给出明确错误。

Rust hello 的 optional capabilities 每连接最多 64 项。适配器使用四条固定功能连接：
Agent/Provider/Skills/Voice/Session/Push、Daemon/Project/Workspace/Terminal/Editor、
Git/Files、其余服务。分别协商 61、57、43、47 项（含公共 ping/release）。各连接仍受
Rust 的并发和订阅预算约束；一个逻辑客户端占四个物理连接。只有四条连接握手完成且
server/instance identity 一致时，才向 SDK 发布已连接状态。任一连接失败时一起关闭，
由 SDK 重连整个逻辑客户端。

相关请求分配独立 wire ID，并恢复 SDK 的原始关联 ID。订阅记录实际物理连接归属，
release 返回所属连接。Voice/听写固定在同一连接；Terminal 和文件 binary opcode
分别路由到所属功能连接，不修改原始 bytes。请求映射最多保存 256 项，超时和断线清理；
不将连接内关联 ID 当作业务幂等键。Provider permission 的 requestId 保留在业务参数中。

能力旗标同时受 `implemented_capabilities` 和适配器的行为支持约束。对尚未实现的
目录同步、组合创建等行为保持不宣称支持；不删除 `messageId`、订阅参数或其他业务字段
来规避 Rust 的错误。原版 socket/pipe/SSH/relay 连接仍走原来的 Paseo 路径；本次未将它们
宣称为 Rust transport。

## 边界与后续

本次不改变 Rust API、鉴权策略、领域边界或服务端实现。前端与 server 的协议转换归
前端 adapter；领域 Session/Message/Run 语义仍遵循 ADR-001 v4。桌面内置 daemon 的进程
管理与完整 npm workspace 构建不在这次接口适配内，仍需整合。

四连接方案增加每个主机的连接开销；未来若 Rust 支持更大的能力集合或动态协商，可以
收敛连接数，但必须保留订阅与音频的物理连接所有权。Rust 剩余行为缺口和验收范围见
[实施报告](../reports/paseo-client-rust-adapter.md)。
