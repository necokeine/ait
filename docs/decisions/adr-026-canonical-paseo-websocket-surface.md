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
teardown script、Agent/terminal 清理和 Paseo metadata 留待对应服务接入，不能以空成功伪装。

## 第五阶段能力

第五阶段生产组装公开 5 个 Workspace automation 方法：`workspace.setup.status.request`、
`workspace.setup.run.request`、`workspace.script.list.request`、`workspace.script.start.request` 和
`workspace.script.stop.request`。历史名称 `workspace_setup_status_request` 与
`start_workspace_script_request` 只保留在 catalog；后者与当前 script start 名称合并为一个 capability。

`server-ports::workspace_automation::WorkspaceAutomationRuntime` 隔离 `paseo.json`、shell 与子进程；
`server-workspace::LocalWorkspaceAutomation` 保存进程内 setup/script 快照。setup 在 worktree registry
提交后异步启动，或在 change-request Workspace 显式批准后清除 durable `untrustedSource` 再启动。
命令顺序执行并注入 Paseo workspace 环境；输出有界，状态通过 setup status 轮询。script list 解析并
排序有效配置；start/stop 操作真实子进程并按 Workspace/script identity 防止重复启动。

PTY terminal history/input、service proxy/health、实时 setup/script event、自动 terminal、archive teardown
仍需要后续 Terminal、订阅与 proxy 边界。这些字段按 Paseo shape 返回 null/省略或使用逻辑 terminal ID，
并在阶段报告逐项列出，不能声称已具备对应能力。

## 第六阶段能力

第六阶段生产组装公开 9 个 Agent runtime 目录与元数据生命周期方法：
`agent.list.request`、`agent.history.get.request`、`agent.get.request`、`agent.update.request`、
`agent.archive.request`、`agent.delete.request`、`agent.detach.request`、
`agent.attention.clear.request` 和 `agent.items.close.request`。历史下划线名称只保留在 catalog，
不作为 wire alias。

`server-domain::agent_runtime::PersistedAgentRuntimeRecord` 独立复制 Paseo `StoredAgentRecord` 的 durable
shape，与 ADR-024 的 Agent preset/revision 类型分开。`server-ports::agent_runtime::AgentRuntimeRegistry`
隔离存储；`server-storage::FileBackedAgentRuntimeRegistry` 通过 `<data-dir>/agents/agents.json` 的原子 JSON
数组保存 snapshot。`server-application::agent_runtime::AgentRuntimeDirectory` 组合 Agent、Workspace 与
Project registry，负责 placement、过滤、排序、分页、ID/prefix/title 查找以及更新、attention、detach、
archive cascade 和 delete。

Provider runtime 尚未建立，所以 stored snapshot 统一投影为 `providerUnavailable:true`，不公开 persistence
resume handle，也没有 active turn、dynamic mode 或 pending permission。create/resume/import/send/wait/cancel、
provider execution、timeline 和 config apply 等方法继续保持未发布。list 的 subscribe/sync、close-items 的
非空 terminal 集合返回 `unsupported_capability`，避免把尚未建立的事件或 Terminal 生命周期伪装为成功。

## 第七阶段能力

第七阶段生产组装公开 4 个 Workspace 状态方法：`workspace.clear_attention.request`、
`workspace.mark_unread.request`、`workspace.recovery.inspect.request` 和
`workspace.recovery.restore.request`。

`server-application::workspace_state::WorkspaceState` 组合共享的 Agent runtime、Workspace、Project
registry 与新的 `WorkspaceRecoveryRuntime` port。clear-attention 按 Workspace ID 所有权处理一个或多个
Workspace，保留 permission attention；mark-unread 只选择 active Workspace 中最新的 finished/read root
Agent，并用单调时间戳持久化 `finished` attention。

recovery inspect 复制 Paseo 的六个 unavailable reason，并区分现存目录的 `unarchive` 与已删除 managed
worktree 的 `restore`。本地 adapter 用保存的 main repository、worktree root、branch、base 和相对 cwd
恢复原路径；branch 已在别处 checkout 时明确拒绝，不创建后缀 branch。成功后取消 Workspace 与 Project
archive，并在 response 后发布 `workspace.update`。

attention 事件广播、live Provider pending-permission 状态、Project Git placement 重探测、merged change-request
latch、Paseo worktree metadata 以及 plugin recovery hook 仍等待对应的新事件、Provider、Forge 和 Plugin 边界；
第七阶段报告记录这些差异。

## 第八阶段能力

第八阶段生产组装公开 7 个 checkout 读取与观察方法：`checkout.status.get.request`、
`checkout.refresh.request`、`checkout.diff.get.request`、`checkout.diff.subscribe.request`、
`checkout.diff.unsubscribe.request`、`checkout.commits.list.request` 和
`checkout.commits.file_diff.request`。历史名称 `checkout_status_request`、
`subscribe_checkout_diff_request` 与 `unsubscribe_checkout_diff_request` 只保留在 catalog；服务端 diff
event 统一为 `checkout.diff.update`。

`server-ports::checkout::CheckoutRuntime` 隔离阻塞 Git；`server-workspace::LocalCheckout` 通过有时限、
有输出预算且不经过 shell 的 Git 子进程实现 status、merge-base diff、untracked diff、commit history 与
单文件 commit diff。commit list 保留全部 Workspace commit，再附加最多十条从 fork point 开始的 base
context，并标记 remote/base reachability 与文件状态。owned worktree 必须同时满足 managed root 的
`<repository-hash>/<slug>` 布局和 linked-worktree Git common-dir 事实。

diff subscription 属于物理 WebSocket connection；初始 snapshot response 写出后才启动 200 ms bounded
polling，同一 ID 替换旧任务，显式 unsubscribe、通用 release、断开和 drain 均取消任务。snapshot 以完整
响应 fingerprint 去重，变化时发布 `checkout.diff.update`。

Paseo 的 filesystem observer/workspace snapshot/debounce、Forge cache invalidation、worktree metadata base
ref、syntax highlighting、per-file diff budget 和丰富 remote/forge resolution 尚未移植。第八阶段报告记录
这些行为差异和真实 Git 测试范围。

## 第九阶段能力

第九阶段生产组装公开 13 个 Git 分支与修改方法：`checkout.branch.validate.request`、
`checkout.branch.suggestions.request`、`checkout.branch.switch.request`、`checkout.rename_branch.request`、
`checkout.commit.request`、`checkout.merge.request`、`checkout.merge_from_base.request`、
`checkout.pull.request`、`checkout.push.request`、`checkout.discard_changes.request`、
`checkout.stash.save.request`、`checkout.stash.pop.request` 和 `checkout.stash.list.request`。对应下划线 Paseo
名称只保留在 catalog，不作为 wire alias。

同一个全新 `CheckoutRuntime` port 扩展 typed branch resolution/suggestion、mutation、merge strategy 和 stash
entry；`LocalCheckout` 继续只以参数数组启动 Git、清理 `GIT_*`，并为写操作使用 120 秒 deadline。实现保留
Paseo 的 clean-tree preflight、origin-only tracking checkout、严格 rename slug、commit `addAll` 默认值、
merge 方向、linked base worktree、冲突 abort、most-ahead base、pull abort、push target、literal discard 和
`paseo-auto-stash:` 过滤语义。API 的共享 blocking semaphore 和 checkout mutex 对这些操作提供有界排队与
进程内串行化。

当前没有 Provider commit-message generator，空消息按 Paseo 最终失败语义拒绝；没有 mutation observer、
Workspace/status event 和 Forge cache invalidation，diff polling 只能在下一轮观察变化；configured push target
成功后不额外写本地 remote-tracking ref。第九阶段报告记录完整差异和真实 branch/merge/local-remote 测试。

## 第十阶段能力

第十阶段生产组装公开 10 个 Forge、PR 与检查状态方法：`forge.search.request`、
`github.search.request`、`checkout.pr.create.request`、`checkout.pr.merge.request`、
`checkout.pr.status.request`、`checkout.pr.timeline.request`、
`checkout.forge.set_auto_merge.request`、`checkout.forge.get_check_details.request`、
`checkout.github.set_auto_merge.request` 和 `checkout.github.get_check_details.request`。历史下划线名称只保留在
catalog；两个 `checkout.github.*` capability 保留 Paseo compatibility payload，但与对应 neutral Forge 方法
共用同一个新 port/adapter。

`server-ports::forge::ForgeRuntime` 隔离阻塞 Git 与 forge CLI；`server-workspace::LocalForge` 当前实现 GitHub
与已经由 `gh` 配置的 GitHub Enterprise。所有命令使用参数数组、关闭 stdin、30 秒读预算、120 秒写预算、
4 MiB stdout 与 64 KiB stderr 上限。search 合并 issue/PR 并按更新时间排序；status 投影 branch 对应 PR 与
check rollup；timeline 保留 review、general comment、inline thread 与 truncation；check details 读取 annotation
和 failed job；PR create 在调用 Forge 前真实 push 当前 branch。

Paseo 的 forge registry 还包含 GitLab、Gitea、Forgejo 和 Codeberg，并为 status、merge/auto-merge 维护更丰富的
forge-specific facts、cache、batch polling 与 mutation invalidation。当前 GitHub adapter 没有这些多 Forge 与
观察能力，也没有 Provider-backed PR 文本生成和 failed-job log tail；显式 title/body、GitHub 核心命令与 wire
shape 已对齐。第十阶段报告记录完整差异和受控 CLI/本地 remote 测试。

## 后果与后续

后续接口按功能组继续移植，并复用同一规范化规则和 capability 准入门槛。涉及 Agent 执行、terminal、
provider、剩余多 Forge、schedule、plugin、hub、voice、push 或 browser 的方法，在各自全新 crate 边界和
生命周期完成前保持未发布。

当前 Paseo 对齐差异、每个第一阶段方法的状态和验证结果记录在
[WebSocket 接口第一阶段报告](../reports/paseo-websocket-surface-phase-1.md)；daemon/config 的行为、
测试和安装边界记录在
[WebSocket 接口第二阶段报告](../reports/paseo-websocket-surface-phase-2.md)；Workspace 标签、事务与
订阅边界记录在
[WebSocket 接口第三阶段报告](../reports/paseo-websocket-surface-phase-3.md)；Worktree 生命周期、
真实 Git 验证与剩余差异记录在
[WebSocket 接口第四阶段报告](../reports/paseo-websocket-surface-phase-4.md)；Workspace setup/script
执行、测试和 Terminal/Proxy 差异记录在
[WebSocket 接口第五阶段报告](../reports/paseo-websocket-surface-phase-5.md)；Agent runtime 目录、
元数据生命周期与 Provider runtime 差异记录在
[WebSocket 接口第六阶段报告](../reports/paseo-websocket-surface-phase-6.md)；Workspace attention、
归档恢复与真实 Git 验证记录在
[WebSocket 接口第七阶段报告](../reports/paseo-websocket-surface-phase-7.md)；Git checkout 状态、Diff、
订阅和提交历史记录在
[WebSocket 接口第八阶段报告](../reports/paseo-websocket-surface-phase-8.md)；Git 分支、commit、merge、pull、
push、discard 与 stash 记录在
[WebSocket 接口第九阶段报告](../reports/paseo-websocket-surface-phase-9.md)；Forge search、PR lifecycle、timeline
与 check details 记录在
[WebSocket 接口第十阶段报告](../reports/paseo-websocket-surface-phase-10.md)。
