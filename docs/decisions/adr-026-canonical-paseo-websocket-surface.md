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

## 第三阶段能力

第三阶段生产组装公开 5 个 Workspace 标签方法：`workspace.label.list.request`、
`workspace.label.assignment.set.request`、`workspace.label.update.request`、
`workspace.label.delete.inspect.request` 和 `workspace.label.delete.request`；同时公开所有后续
connection-owned 订阅共用的 `subscription.release.request`。

标签 definition 是 host-wide catalog，Workspace 记录只保存名称 assignment。名称先折叠空白并
trim，以不区分大小写的 key 比较；颜色限制为 Paseo 固定的十色 palette。重命名与改色是一个
原子编辑，名称冲突时两个字段都不落盘；重命名和删除同时重写 active/archived Workspace 的
assignment。删除检查与真正删除使用同一计数集合。

`<data-dir>/projects/workspace-labels.json` 与 `workspaces.json` 通过
`workspace-labels.transaction.json` 的 prepared/committed journal 协调。prepared 中断在重启时
回滚两份文件；committed marker 只用于清理，不把旧 after-image 覆盖到更新的 Workspace。无法
判断提交结果时冻结 registry 写入直到重启，并返回 `workspace_label_storage_uncertain`。

标签 list 可携带 generation/sequence cursor 并选择订阅。服务端先建立监听，再返回一致的
snapshot 或压缩 changes；响应发送完成后才放行 bootstrap 期间的 live update。一个连接可持有
多个服务端分配 ID 的标签订阅，断开或 `subscription.release.request` 会独立释放对应监听。

## 第四阶段能力

第四阶段生产组装公开 3 个 Worktree 方法：`workspace.worktree.list.request`、
`workspace.worktree.create.request` 和 `workspace.worktree.archive.request`。三个历史 Paseo 名称
`paseo_worktree_list_request`、`create_paseo_worktree_request`、
`paseo_worktree_archive_request` 只保留在 catalog，不作为 wire alias。

`server-ports::worktrees::ManagedWorktrees` 隔离阻塞 Git 与文件操作；
`server-workspace::LocalManagedWorktrees` 把 owned worktree 固定放在
`<data-dir>/worktrees/<repo-hash>/<slug>`。application 先完成 Git 创建，再选择或新建 Project、写入
Paseo-shaped Workspace record；后续 registry 失败会删除刚创建的 worktree。归档时 `workspace`
scope 只归档一个记录，最后一个 active 引用消失才删目录；`worktree` scope 归档该 checkout 下的
全部 active Workspace，并且必须先通过 managed-root ownership 检查。

创建支持 source cwd 位于 repository 子目录、branch-off/default branch、已有 branch/路径 collision
suffix、existing branch checkout、首 Agent prompt 的 provisional title 和未跟踪 `paseo.json` 种子
复制。创建响应之后发布统一 envelope 的 `workspace.update` upsert event。change-request checkout、
setup/teardown script、Agent/terminal 清理和 Paseo metadata 留待对应服务接入，不能以空成功伪装。

## 后果与后续

后续接口按功能组继续移植，并复用同一规范化规则和 capability 准入门槛。涉及 Agent、terminal、
provider、forge、schedule、plugin、hub、voice、push 或 browser 的方法，在各自全新
crate 边界和生命周期完成前保持未发布。

当前 Paseo 对齐差异、每个第一阶段方法的状态和验证结果记录在
[WebSocket 接口第一阶段报告](../reports/paseo-websocket-surface-phase-1.md)；daemon/config 的行为、
测试和安装边界记录在
[WebSocket 接口第二阶段报告](../reports/paseo-websocket-surface-phase-2.md)；Workspace 标签、事务与
订阅边界记录在
[WebSocket 接口第三阶段报告](../reports/paseo-websocket-surface-phase-3.md)；Worktree 生命周期、
真实 Git 验证与剩余差异记录在
[WebSocket 接口第四阶段报告](../reports/paseo-websocket-surface-phase-4.md)。
