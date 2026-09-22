# ADR-026：规范化 Paseo WebSocket 接口并按能力分期接入

- 状态：Accepted。
- 日期：2026-09-22。
- 来源：`getpaseo/paseo@2c8e8a826810337492cc5a38bb0bbd705b6fb632`。
- 范围：新 `server` binary 及其全新 `server-*` crate；旧 daemon 与旧 Ait crate 不变。

## 背景

Paseo server 的入站消息名称同时包含 dotted name、下划线 legacy name 和 slash name，例如
`project.icon.get.request`、`read_project_config_request` 与 `schedule/run-once`。直接复制这些名字
会把历史命名差异固化为新协议，也会使同一行为出现多个公开入口。

新 server 已有带 hello、版本范围、capability 协商和统一 response/error envelope 的独立
WebSocket transport。接口移植需要保留这条连接边界，同时让业务 payload 和可观察行为尽量
对齐固定 Paseo 快照。

## 决策

`server-protocol::methods` 保存用户给出的全部 191 个 Paseo 入站名称及其规范名称、功能分组和
消息方向。规范名称统一为小写 dotted name：请求以 `.request` 结束；客户端事件与服务端发起
工作的响应分别使用稳定的事件名和 `.response`。例如：

| Paseo 名称 | 新 server 规范名称 |
| --- | --- |
| `read_project_config_request` | `project.config.read.request` |
| `write_project_config_request` | `project.config.write.request` |
| `fetch_workspaces_request` | `workspace.list.request` |
| `open_project_request` | `workspace.open.request` |
| `schedule/run-once` | `schedule.run_once.request` |

原名称仅作审计和测试生成依据，不作为 wire alias。若多个 Paseo 名称表达同一操作，它们映射到
一个规范名称；当前明确合并 `agent.create`、`project.icon.get` 和 `workspace.script.start` 三组。
catalog 测试固定条目总数、名称唯一性、格式和允许的合并集合，防止后续静默改变协议。

只有完成 DTO、application use case、port/adapter、生产组装及 WebSocket 验证的方法才能加入
对应模块的 `CAPABILITIES`。catalog 中登记但未实现的方法不参与 hello 协商，调用返回
`method_not_found`；客户端没有协商已实现方法时返回 `unsupported_capability`。这样客户端不会
把路线图误认为当前能力。

WebSocket 继续使用新 server 的统一 envelope：客户端发送
`{type:"request",request_id,method,params}`，服务端返回 correlated response/error。Paseo 的
method-specific payload 被移植进 `params`/`result`，不复制其顶层 discriminated union。这个差异
保留现有认证、大小限制、背压、drain 和 capability 协商语义。

Project/Workspace 方法通过 `server-application::directory::Directory` 协调纯 port；本地 Git、
文件、`paseo.json` 和图标处理由 `server-workspace` adapter 实现。API 不直接依赖 port 或存储
crate。Project/Workspace record 继续遵循 ADR-025 的 Paseo 结构，旧 `Projects` 租约切片只作为
过渡能力并存，不能用于实现新的目录方法。

## 第一阶段能力

第一阶段生产组装公开 15 个规范方法：

- Project：`project.add.request`、`project.create_directory.request`、`project.list.request`、
  `project.rename.request`、`project.remove.request`、`project.config.read.request`、
  `project.config.write.request`、`project.icon.set.request`、`project.icon.get.request`。
- Workspace：`workspace.open.request`、`workspace.create.request`、`workspace.list.request`、
  `workspace.archive.request`、`workspace.title.set.request`、`workspace.pin.set.request`。

目录选择保留用户所选根，即使它位于 Git repository 内部。`workspace.open` 依次复用最早的
active workspace、恢复最早且 Project 仍 active 的 archived workspace，最后才创建记录；
`workspace.create` 对 directory source 总是新建。Project config 采用 revision compare-and-swap
和同目录原子替换。Project icon 只允许 automatic 或客户端 upload，禁止服务端抓取任意 URL。

## 第二阶段能力

第二阶段生产组装公开 9 个 daemon 方法：`daemon.get_status.request`、
`daemon.get_pairing_offer.request`、`daemon.config.reload.request`、`daemon.update.request`、
`diagnostics.request`、`daemon.config.get.request`、`daemon.config.set.request`、
`server.restart.request` 和 `server.shutdown.request`。

配置使用 `<data-dir>/config.json`、同目录原子替换和内存发布顺序；patch 只采纳 Paseo mutable
config 的可写字段，未知 passthrough 字段不成为隐式设置入口。reload 对外部修改做 live/restart
路径分类。restart 在 standalone Rust 进程内释放旧实例并重新组装服务；shutdown 完成相关响应后
走同一 drain 边界。self-update 保留 Paseo 结果形状，但 standalone 安装没有包管理器 adapter，
因此明确返回失败，不触发重启。

## 后果与后续

后续接口按功能组继续移植，并复用同一规范化规则和 capability 准入门槛。涉及 worktree、Agent、
terminal、provider、forge、schedule、plugin、hub、voice、push 或 browser 的方法，在各自全新
crate 边界和生命周期完成前保持未发布。

当前 Paseo 对齐差异、每个第一阶段方法的状态和验证结果记录在
[WebSocket 接口第一阶段报告](../reports/paseo-websocket-surface-phase-1.md)；daemon/config 的行为、
测试和安装边界记录在
[WebSocket 接口第二阶段报告](../reports/paseo-websocket-surface-phase-2.md)。
