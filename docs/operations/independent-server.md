# 独立 server：使用与协议

> 当前 binary 已接通规范化后的 Paseo Project/Workspace、daemon/config、Workspace 标签、
> Worktree、Workspace setup/script、Workspace attention/recovery、Git checkout 读取/订阅/变更与 Agent runtime
> 目录/元数据生命周期 WebSocket 接口，详见
> [ADR-026](../decisions/adr-026-canonical-paseo-websocket-surface.md)。下文的
> 早期 `project.open/list/get/close` 已废除，Project/Workspace 统一由 `server-metadata` 管理，
> 见 [ADR-029](../decisions/adr-029-server-metadata.md)。Git、Forge/PR、文件/目录、Worktree、
> 恢复和 GitHub clone 由 `server-filesystem` 管理，见 [ADR-030](../decisions/adr-030-server-filesystem.md)。

`server` 与现有 daemon 并存，当前支持本机服务、WebSocket、独立 Git 项目、Paseo Agent runtime
snapshot、版本化 Agent 配置、Codex 原生纯文本执行、Session 事件与 Terminal。内部代码全部来自八个 `server-*` package；
领域 Session ref、Message 树与 Run 尚未在独立 server 接通。
项目必须使用独立 clone，不能与旧 daemon 共管同一目录或共享 Git worktree。

## 启动

```sh
export AIT_SERVER_TOKEN="$(openssl rand -hex 32)"
cargo run -p server-bin --bin server -- --listen 127.0.0.1:7316
```

凭据只从 `AIT_SERVER_TOKEN` 读取，要求 32–256 字节可见 ASCII、无空格；请使用随机值。
不支持命令行 server bearer、URL server bearer、自动加载 `.env` 或配置文件中的 token。
文件下载使用单独签发的短时一次性 token，详见下方文件接口。
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

运行期 mutable daemon 配置单独保存在 `<data-dir>/config.json`，由
`daemon.config.get.request`、`daemon.config.set.request` 和 `daemon.config.reload.request`
管理。它不保存 bearer token，也不改变上述启动参数优先级。

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
`negotiated_capabilities`。`info.capabilities` 列出 195 个可协商规范名称及独立方法，
`info.implemented_capabilities` 列出当前 host 真正组装的 122 个方法。尚未实现的 73 个规范方法
可协商，但调用后会返回 `not_implemented`；每次 hello 最多传 64 个 optional 和 64 个 required
名称，请按需声明。能力必须协商后才能调用。`client_id` 仅用于诊断，不用于身份、
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
| `subscription.release.request` | `subscriptionId` | 幂等释放本连接的 connection-owned 订阅 |

状态通知形如 `{"type":"status","subscription_id":"…","lifecycle":"ready"}`。
每连接最多 16 个订阅，断开即清理，重连需要重新订阅。退出时 best-effort 发出 `draining`。
这是临时连接通知，没有持久序号/回放语义；业务 outbox 与 cursor 留到后续 M1 切片。

错误形如 `{"type":"error","request_id":"r1","code":"not_implemented","message":"Method is not implemented yet","retryable":false}`。
已登记但未实现的方法返回 `not_implemented`，未知方法或原版旧名称返回 `method_not_found`；未协商
的方法返回 `unsupported_capability`。有效请求的错误保留 request ID。客户端通知使用
`{"type":"event","method":"terminal.input","params":{...}}`；客户端回传使用
`{"type":"response","request_id":"...","method":"browser.automation.execute.response","params":{...}}`。
这两类当前都返回 `not_implemented`；通知的错误没有 request ID，回传错误会附带提供的 ID。
不合法 envelope、重复 hello、未协商或非法二进制消息与握手失败会关闭连接；未知 method 或错误参数可修正后
继续使用当前连接。请求按连接顺序处理，已响应的 request ID 可以再次使用；它不是幂等键。

## Daemon、配置与诊断

以下 capability 已在生产 binary 组装：

| Method | Params | Result |
| --- | --- | --- |
| `daemon.get_status.request` | `{}` | server/version/pid/executable/start/listen、relay、provider 状态 |
| `daemon.get_pairing_offer.request` | `{}` | `url`、`qr`、`relayEnabled`；当前无 relay，返回空 offer |
| `daemon.config.get.request` | `{}` | 规范化 mutable config |
| `daemon.config.set.request` | `config` patch | 原子持久化后的规范化 config |
| `daemon.config.reload.request` | `{}` | live、需重启、启动 override 控制的路径分类 |
| `diagnostics.request` | `{}` | 不含凭据的进程、系统与 capability 文本报告 |
| `daemon.update.request` | `{}` | Paseo update 结果；standalone 安装明确返回 unsupported failure |
| `server.restart.request` | 可选 `reason` | 响应后 drain，释放锁并在同一进程重新组装实例 |
| `server.shutdown.request` | `{}` | 响应后 drain 并正常退出 |

legacy 名称 `get_daemon_config_request`、`set_daemon_config_request`、
`restart_server_request`、`shutdown_server_request` 不作为 alias，返回 `method_not_found`。
外部 SIGINT/SIGTERM 与 WebSocket lifecycle 请求共用 15 秒 drain 预算。restart 保留 `server_id`，
生成新的 `instance_id`；使用端口 0 时系统可能分配新的实际端口。

## Workspace 标签

以下 capability 已在生产 binary 组装：

| Method | Params | Result |
| --- | --- | --- |
| `workspace.label.list.request` | 可选 `sync:{generation,afterSeq}`；订阅时加 `subscribe:{}` | `labels`、`sync`，订阅时另有服务端 `subscriptionId` |
| `workspace.label.assignment.set.request` | `workspaceId`、`label:{name,color}`、`assigned` | 权威 label 与完整 `workspaceLabels` |
| `workspace.label.update.request` | `name`，可选 `newName` / `color` | 原子编辑后的 label 与受影响 Workspace 数 |
| `workspace.label.delete.inspect.request` | `name` | active/archived Workspace 的受影响数量 |
| `workspace.label.delete.request` | `name` | 删除 definition 和 assignments 后的受影响数量 |

标签名会 trim 并折叠内部空白，以不区分大小写的 key 查找；同名已有 definition 时，其显示名和
颜色优先。palette 为 `violet`、`sky`、`emerald`、`orange`、`pink`、`indigo`、`teal`、
`red`、`amber`、`blue`。assignment 只接受 active Workspace；删除不存在的标签以及释放不存在的
订阅均幂等成功。

订阅 ID 由服务端分配；现代请求必须发送 `subscribe:{}`，不能提供 `subscriptionId`。同一连接可
建立多个独立订阅。live update 使用统一 envelope：

```json
{"type":"event","method":"workspace.label.update","params":{"kind":"upsert","subscriptionId":"…","label":{"name":"QA","color":"blue"},"generation":"…","seq":1}}
```

客户端保存 `sync.generation` 和 `sync.headSeq` 作为下次 list 的 cursor。同一进程内且 cursor 仍在
256 条 journal 窗口内时返回压缩后的 `changes`；进程重启、generation 不匹配、未来序号或过期
cursor 返回完整 `snapshot`。调用 `subscription.release.request` 或断开连接后停止该订阅的推送。

catalog 位于 `<data-dir>/projects/workspace-labels.json`。跨 catalog 与 `workspaces.json` 的写入使用
`workspace-labels.transaction.json` 恢复；正常完成后 journal 会删除。出现
`workspace_label_storage_uncertain` 时停止写入并重启 server，让启动恢复先确定一致状态。

## Worktree

以下 capability 已在生产 binary 组装：

| Method | Params | Result |
| --- | --- | --- |
| `workspace.worktree.list.request` | `cwd` 或 `repoRoot` | managed `worktrees` 与 inline `error` |
| `workspace.worktree.create.request` | `cwd`，可选 `projectId`、`worktreeSlug`、first-Agent context、`refName`、`action` | Workspace descriptor、setup 字段与 inline error |
| `workspace.worktree.archive.request` | path 或 repo+branch，可选 `workspaceId`、`scope` | `success`、`removedAgents` 与 inline error |

worktree 固定创建在 `<data-dir>/worktrees/<repo-hash>/<slug>`，不会把 repository 内的任意 linked
worktree 认作服务所有。`branch-off` 从 `refName` 或 default branch 创建新分支；`checkout` 使用
现有 local branch，必要时从 origin 获取。同名 branch 或目录采用 `-1`、`-2` 后缀。source cwd
可以位于 repository 子目录，返回的 `workspaceDirectory` 保留相对位置；source cwd 中未跟踪的
`paseo.json` 会以 create-new 方式复制到对应目录，不覆盖 checkout 已有文件。

`scope` 缺省为 `workspace`：只归档目标 Workspace，且只在没有其他 active Workspace 引用时删除
managed worktree。`scope:"worktree"` 归档该 worktree 内全部 active Workspace 后删除目录；外部
路径返回 `NOT_ALLOWED`。legacy `deleteWorktreeFromDisk` 被解析但不控制删除，规则由 scope、引用与
ownership 共同决定。成功创建后，响应之后会收到
`{"type":"event","method":"workspace.update",...}` upsert event。

当前 create 会在 registry 提交后异步执行 `paseo.json` setup，但不创建 PTY setup terminal；archive
不执行 teardown 或 Agent 清理，因此 `removedAgents` 为空；真实 PTY 由 Terminal reconciliation
在 Workspace/Project 归档或移除后关闭。`checkoutSource` 与
`githubPrNumber` 在 Forge 服务接通前返回明确失败。这些限制不会返回伪成功，完整差异见第四、
第五阶段报告。

## Workspace setup 与脚本

以下 capability 已在生产 binary 组装：

| Method | Params | Result |
| --- | --- | --- |
| `workspace.setup.status.request` | `workspaceId` | `workspaceId` 与 nullable setup `snapshot` |
| `workspace.setup.run.request` | `workspaceId` | `started` 与 inline `error` |
| `workspace.script.list.request` | `workspaceId` | 已排序 script payload 与 inline `error` |
| `workspace.script.start.request` | `workspaceId`、`scriptName` | 启动后的 `script` 或 inline `error` |
| `workspace.script.stop.request` | `workspaceId`、`scriptName` | 停止后的 `script` 或 inline `error` |

配置读取 Workspace cwd 下的 `paseo.json`。文件必须是不超过 1 MiB 的普通文件；symlink、非 JSON 或
非 object 会返回明确 parse error。`worktree.setup` 接受一条字符串或字符串数组，trim 后按顺序执行；
`scripts` 只采纳 object 中带非空 `command` 的条目，`type:"service"` 识别为 service，其他 type 按
Paseo 规则视为普通 script。list 以 script name 做不区分大小写的稳定排序。

setup 命令注入 `PASEO_SOURCE_CHECKOUT_PATH`、`PASEO_ROOT_PATH`、`PASEO_WORKTREE_PATH`、
`PASEO_BRANCH_NAME` 和进程内稳定的 `PASEO_WORKTREE_PORT`。单条命令最多运行 30 分钟、capture 最多
8 MiB；公开 log 保留 64 KiB 的首尾并标记 truncated。第一条失败命令会终止 setup 序列，status 可
轮询 running/completed/failed 和逐命令结果。

带 `untrustedSource` 的 change-request Workspace 在批准前返回 blocked snapshot，也拒绝 script start。
调用 setup run 会先持久化清除 provenance，再启动后台 setup；已 trusted 的 Workspace 返回
`started:false`。普通 worktree 创建会自动触发 setup。

script start 启动真实 shell 子进程，同一 Workspace/name 运行中重复启动会失败；list 会刷新退出状态，
stop 先终止 Unix process group 再等待回收。server 最后一个 runtime owner 释放时也会清理仍在运行的
Unix process group 或 Windows 直接子进程。当前 `terminalId` 是逻辑 process identity，没有 PTY
history/input；stdout/stderr 不进入
Terminal API。service proxy URL 与 health 尚未实现，相关字段为 null/省略，hostname 暂用 script key。
实时 `workspace_setup_progress` / `script_status_update` event、`worktree.terminals` 自动启动和 archive
teardown 等待后续订阅、Terminal 与 proxy 切片。完整差异见第五阶段报告。

## Workspace attention 与归档恢复

以下 capability 已在生产 binary 组装：

| Method | Params | Result |
| --- | --- | --- |
| `workspace.clear_attention.request` | `workspaceId` 字符串或字符串数组 | flattened `clearedAgentIds`、逐 Workspace `results`、`success` 与 inline `error` |
| `workspace.mark_unread.request` | `workspaceId` | nullable `markedAgentId`、`success` 与 inline `error` |
| `workspace.recovery.inspect.request` | `workspaceId` | `recoverable` 或 `unavailable` 的 `state` |
| `workspace.recovery.restore.request` | `workspaceId` | `accepted` 与 inline `error`；成功后发送 `workspace.update` |

clear-attention 只处理 active Workspace 中归属 ID 完全匹配、未归档、非 internal 且没有 permission reason
的 Agent。批量请求逐项执行，单个 Workspace 失败不会撤销已经提交的其他项。mark-unread 沿
`paseo.parent-agent-id` 找到 Workspace root，跳过循环、缺失 parent、running、已 unread 或 archived Agent，
在候选中选择更新时间最新的 finished Agent；写入 `requiresAttention:true`、`attentionReason:"finished"`。

recovery inspect 的稳定 unavailable reason 为 `workspace_not_found`、`workspace_not_archived`、
`project_not_found`、`project_directory_missing`、`workspace_directory_missing` 和
`worktree_branch_missing`。现存目录直接 unarchive；删除的 managed worktree 从保存的 main repository 和
branch 恢复到原 worktree root，并验证原 Workspace 相对目录仍存在。分支已在其他 checkout 使用时恢复失败，
不会创建另一条带后缀的 branch。恢复成功会同时取消 owning Project 的 archive。

当前已接通 native turn，但尚无 Provider pending-permission 集合；clear-attention 以
`attentionReason:"permission"` 作为 fail-safe 排除条件。attention 修改尚无 Agent/Workspace subscription
event；recovery 只发布统一 envelope 的 `workspace.update`。恢复不会重新探测 Project kind/project key、
merged change-request latch，不写 Paseo metadata，也不调用 plugin recovery hook。完整对齐范围和差异见
第七阶段报告。

## Git checkout 状态、Diff 与提交历史

以下 capability 已在生产 binary 组装：

| Method | Params | Result |
| --- | --- | --- |
| `checkout.status.get.request` | `cwd` | Git/root/branch/dirty/base/upstream/remote/managed-worktree 状态与 inline `error` |
| `checkout.refresh.request` | `cwd` | `success` 与 inline `error` |
| `checkout.diff.get.request` | `cwd`、`compare` | path-sorted structured `files`、inline `error`、可选 `diffTooLarge` |
| `checkout.diff.subscribe.request` | 可选 `subscriptionId`、`cwd`、`compare` | 初始 snapshot；变化后发送 `checkout.diff.update` |
| `checkout.diff.unsubscribe.request` | `subscriptionId` | 已释放的 `subscriptionId` |
| `checkout.commits.list.request` | `cwd` | Workspace commits、最多十条 base context、remote/base 标记与文件统计 |
| `checkout.commits.file_diff.request` | `cwd`、hex `sha`、repository-relative `path` | nullable textual structured diff 与 inline `error` |

`compare.mode` 支持 `uncommitted` 和 `base`；后者比较 merge-base 到 `HEAD`，不会混入 working tree
修改。uncommitted 模式合并 tracked、staged 与 untracked 文件。Git 子进程不经过 shell，清理继承的
`GIT_*` 环境，禁用 optional locks/fsmonitor/color，限制为 10 秒和有界输出。managed ownership 只在
checkout 是 `<data-dir>/worktrees/<repository-hash>/<slug>` 下的 linked worktree 时成立。

diff 订阅属于当前 WebSocket connection；同一 ID 再次订阅会替换旧订阅，显式 unsubscribe、通用
`subscription.release.request`、连接断开和 server drain 都会取消轮询任务。当前实现每 200 ms 读取并按
snapshot fingerprint 去重；Paseo 使用 filesystem observer、workspace snapshot 和 150 ms debounce。
refresh 只是强制执行一次新 Git 读取，尚不触发 Forge cache invalidation、workspace snapshot 广播或 diff
fanout。结构化 diff 不生成 syntax-highlight tokens；aggregate 超过 4 MiB 时整体返回
`diffTooLarge:true`，还没有 Paseo 的 per-file budget/placeholder。完整对齐范围见第八阶段报告。

## Git 分支与修改操作

以下 capability 已在生产 binary 组装：

| Method | Params | Result |
| --- | --- | --- |
| `checkout.branch.validate.request` | `cwd`、`branchName` | `exists`、规范化 `resolvedRef`、`isRemote` 与 string `error` |
| `checkout.branch.suggestions.request` | `cwd`、可选 `query`/`limit` | 排序后的 `branches`、local/remote/divergence `branchDetails` 与 string `error` |
| `checkout.branch.switch.request` | `cwd`、`branch` | `success`、`branch`、可选 `source` 与 inline `error` |
| `checkout.rename_branch.request` | `cwd`、`branch` | `success`、nullable `currentBranch` 与 inline `error` |
| `checkout.commit.request` | `cwd`、可选 `message`/`addAll` | `success` 与 inline `error` |
| `checkout.merge.request` | `cwd`、可选 `baseRef`/`strategy`/`requireCleanTarget` | `success` 与 inline `error` |
| `checkout.merge_from_base.request` | `cwd`、可选 `baseRef`/`requireCleanTarget` | `success` 与 inline `error` |
| `checkout.pull.request` | `cwd` | `success` 与 inline `error` |
| `checkout.push.request` | `cwd` | `success` 与 inline `error` |
| `checkout.discard_changes.request` | `cwd`、非空 repository-relative `paths` | `success` 与 inline `error` |
| `checkout.stash.save.request` | `cwd`、可选 `branch` | `success` 与 inline `error` |
| `checkout.stash.pop.request` | `cwd`、非负 `stashIndex` | `success` 与 inline `error` |
| `checkout.stash.list.request` | `cwd`、可选 `paseoOnly` | `entries` 与 inline `error` |

branch switch 要求工作区干净；origin-only branch 会创建同名 local tracking branch。rename 使用 Paseo 的
lowercase slug 规则。commit 默认 `addAll:true`；独立 server 尚无 Paseo Provider commit-message generator，
所以省略或传入空 `message` 会返回 `UNKNOWN / Commit message is required`。merge-from-base 默认要求当前
checkout 干净；merge-to-base 默认普通 merge，也支持 squash。base branch 在另一个 linked worktree 中时，
变更在那个 worktree 执行；同一 checkout 临时切到 base 后会恢复原 branch。检测到冲突会尝试 `merge --abort`
并返回 `MERGE_CONFLICT`。

discard 使用 literal pathspec，恢复 tracked 内容并删除所选 untracked 路径。stash save 包含 untracked 文件，
消息前缀固定为 `paseo-auto-stash:`；stash list 默认只返回该前缀的条目。所有 Git 写操作不用 shell，清理
`GIT_*` 环境，并使用 120 秒 deadline 与有界输出。当前没有 Paseo mutation observer/Forge cache，成功变更
不会立即广播 Workspace/status event；已有 diff polling subscription 会在下一次轮询观察到变化。完整对齐
范围见第九阶段报告。

## Forge、Pull Request 与检查状态

以下 capability 已在生产 binary 组装：

| Method | Params | Result |
| --- | --- | --- |
| `forge.search.request` | `cwd`、`query`、可选 `limit`/`kinds` | neutral issue/change-request `items`、`authState`、string `error` |
| `github.search.request` | 同上，兼容 `github-issue`/`github-pr`/`pr` kind | legacy issue/PR `items` 与两个 availability flag |
| `checkout.pr.create.request` | `cwd`、`title`、`body`、可选 `baseRef` | nullable `url`/`number` 与 inline `error` |
| `checkout.pr.merge.request` | `cwd`、`mergeMethod` | `success` 与 inline `error` |
| `checkout.pr.status.request` | `cwd` | nullable current PR、check rollup、forge/auth 与 inline `error` |
| `checkout.pr.timeline.request` | `cwd`、正数 `prNumber`、`repoOwner`、`repoName` | review/comment/thread items、`truncated` 与 timeline error |
| `checkout.forge.set_auto_merge.request` | `cwd`、`enabled`、启用时必需 `mergeMethod` | requested state、`success` 与 inline `error` |
| `checkout.github.set_auto_merge.request` | 同上 | GitHub compatibility 入口，结果 shape 相同 |
| `checkout.forge.get_check_details.request` | `cwd`、repo identity、check/workflow ID | annotation/failed-job details 与 inline `error` |
| `checkout.github.get_check_details.request` | 同上 | GitHub compatibility 入口，结果 shape 相同 |

当前 adapter 使用本机 `gh` 认证，只支持 GitHub 和已配置的 GitHub Enterprise。搜索 limit 为 1–50；省略
kind 时同时查询 issue 和 PR。PR create 需要显式非空 title/body，先执行 `git push -u origin <head>`，再调用
GitHub API。独立 server 尚无 Paseo Provider PR 文本生成器，不能补齐省略字段。auto-merge 启用时必须提供
`merge`、`squash` 或 `rebase`，禁用时不能携带 merge method。

CLI stdin 关闭，读操作限制 30 秒，push/创建/merge/auto-merge 限制 120 秒，输出有界；凭据仍由用户现有
Git/`gh` 配置提供。当前没有 GitLab/Gitea/Forgejo/Codeberg adapter、Forge cache/batch poll、status event、
mutation invalidation、GitHub merge-policy GraphQL facts或 failed-job log tail。完整对齐范围见第十阶段报告。

## 文件、目录与上传下载

以下 11 个 capability 已在生产 binary 组装：

| Method | Params | Result |
| --- | --- | --- |
| `directory.suggestions.request` | `query`，可选 `cwd`/`includeFiles`/`includeDirectories`/`matchMode`/`limit` | `directories`、`entries`、nullable `error` |
| `fs.explorer.request` | `cwd`、`mode: list/file`，可选 `path`/`acceptBinary`/`maxBytes` | `directory` 或 `file`，或者二进制文件帧 |
| `fs.file.subscribe.request` | `cwd`、`path`，可选 `subscriptionId` | `subscriptionId`、`initial`；随后 `fs.file.update` |
| `fs.file.unsubscribe.request` | `subscriptionId` | `subscriptionId` |
| `fs.file.write.request` | `cwd`、`path`、`content`、`expectedModifiedAt`，可选 `expectedRevision` | `result.status: written/conflict/error` |
| `fs.entry.create.request` | `cwd`、`parentPath`、`name`、`kind: file/directory` | nullable `path`、`success`、`error` |
| `fs.entry.rename.request` | `cwd`、`path`、`name` | nullable `renamedPath`、`success`、`error` |
| `fs.entry.duplicate.request` | `cwd`、`path` | nullable `duplicatedPath`、`success`、`error` |
| `fs.entry.delete.request` | `cwd`、`path` | `success`、`error` |
| `fs.file.download_token.request` | `cwd`、`path` | nullable `token`/`fileName`/`mimeType`/`size`、`error` |
| `file.upload.request` | `fileName`、`mimeType`、`size`、`modifiedAt` | End 后返回 uploaded-file attachment 或 `error` |

写入优先比较 `expectedRevision`，缺失时比较显示时间；只编辑已存在、最多 1 MiB 的 UTF-8 文本。
临时文件同步、再次检查版本后原子替换，并保留权限。目录按 mtime 降序、同时间按名称排序；复制使用
`name copy.ext` / `name copy 2.ext`；已跟踪条目通过 `git mv` 重命名；删除 symlink 只删除链接。
目录入口拒绝作用域外路径，workspace 根不可重命名、复制或删除。

文件订阅每 200 ms 检查 metadata，发布 `ready/missing/error` 版本，不传文件内容。订阅 ID 属于当前连接，
同名订阅替换旧订阅，`subscription.release.request` 也可释放，断线自动取消。

`acceptBinary:true` 的文件读取用 Paseo binary framing 返回 Begin/Chunk/End，chunk 最大 256 KiB；
上传在标准 request 后以相同 `request_id` 发送这三类帧，完成时才收到标准 response。空文件也必须发送
Begin 和 End。上传最多 64 MiB，每连接最多 8 个待完成传输，10 分钟空闲后清理（最多 30 秒检查延迟）。
未完成文件在失败和断线后删除；已完成文件保留在独立 data-dir 的 `uploads/`。

JSON preview 上限 512 KiB，图片用 base64，其他二进制只返回 metadata。大文件使用 binary 或 HTTP 下载：
先通过 WebSocket 获取 token，再调用 `GET /api/files/download?token=...`，无需 server bearer；token
60 秒过期、单次消费，Host/Origin 检查仍生效。未提供 token 返回 400，无效/重复 token 返回 403，
目标已移除返回 404。响应是附件流，带 `no-store`/`nosniff`。不要把通用 server bearer 放在 URL 中。

目录搜索默认 limit 30（1–100）、默认只含目录；有 cwd 时返回相对路径且空 query 浏览子项，无 cwd 时
搜索 home、返回绝对路径且空 query 不返回结果。当前 fuzzy 排序、轮询监听和资源上限与 Paseo 的差异
记录在第十一阶段报告。

## Agent runtime 目录与元数据生命周期

以下 capability 已在生产 binary 组装：

| Method | Params | Result |
| --- | --- | --- |
| `agent.list.request` | 可选 `scope:"active"`、`filter`、`sort`、`page` | placement 后的 unarchived Agent rows 与 `pageInfo` |
| `agent.history.get.request` | 可选 `filter`、`search`、`sort`、`page` | 默认包含 archived Agent 的 rows 与 `pageInfo` |
| `agent.get.request` | `agentId`（完整 ID、唯一前缀或精确标题） | nullable `agent`、nullable `project` 与 inline `error` |
| `agent.update.request` | `agentId`，以及非空 `name` 或 `labels` | `agentId`、`accepted` 与 inline `error` |
| `agent.archive.request` | `agentId` | `agentId`、`archivedAt` |
| `agent.delete.request` | `agentId` | 已永久删除的 `agentId` |
| `agent.detach.request` | `agentId` | `agentId`、`accepted` 与 inline `error` |
| `agent.attention.clear.request` | 一个 `agentId` 或 Agent ID 数组 | 原 selection 与更新后的 `agents` |
| `agent.items.close.request` | `agentIds`、`terminalIds` | 成功归档的 `agents` 与逐项 `terminalId/success` |

runtime snapshot 保存于 `<data-dir>/agents/agents.json`，shape 来自 Paseo `StoredAgentRecord`，与下文
ADR-024 Agent preset catalog 是两类数据。list/history 会用 Workspace/Project registry 生成 placement，
过滤 internal 或 placement 已不存在的记录；支持 label、project key、status、attention、thinking、archive
过滤，最多每页 200 条。当前 cursor 是十进制 offset。get 仍可读取 placement 已不存在的公共记录。

archive 会清除 attention，把 running/initializing snapshot 收敛为 idle，并递归归档同 Workspace 的 delegated
child；跨 Workspace 或带 open-tab label 的 child 会 detach。delegated Agent 的 detach 删除
`paseo.parent-agent-id` 和全部 `paseo.open-agent-tab.*` label，已经没有 parent label 时保持不变。delete
是永久删除；close-items 独立处理每个 Agent，按 Paseo 行为只返回成功项。

未恢复的 stored Agent 保守返回 `providerUnavailable:true`、`persistence:null`；成功创建或恢复的
live Agent 返回原生 handle 和可选 activeTurn。list 的 `subscribe`/`sync`、timeline、流式事件
和权限交互已接通。原生执行与当前 provider 能力见下节及 [能力矩阵](../reports/provider-parity.md)。

## Codex / Claude Code 原生执行

生产 host 默认从 PATH 启动 `codex app-server`，也可在启动 server 前通过 `AIT_SERVER_CODEX_BIN`
指定可执行文件；Claude 使用 `AIT_SERVER_CLAUDE_BIN` 或 PATH 中的 `claude`。两种 CLI
自行管理认证。Codex 支持 read-only、auto、full-access 和经原生版本协商的 auto-review；
Claude 模式见 [使用说明](claude-code.md)。配置、权限及能力边界见 [ADR-052](../decisions/adr-052-native-provider-capabilities.md)。

先用 `workspace.open.request` 打开目录，再在已协商相应 capability 的连接中调用：

| 方法 | 参数 | 结果 |
| --- | --- | --- |
| `agent.create.request` | `config:{provider:"codex",cwd:"/absolute/path",modeId:"read-only"}`；可选 `agentId` UUID、`workspaceId`、`labels`；config 可加 title/model/thinkingOptionId/systemPrompt | `status:"agent_created"`、agentId、agent snapshot |
| `agent.resume.request` | `handle:{provider:"codex",sessionId:"..."}`，必须已登记在本 server | 同一个 Agent ID、`status:"agent_resumed"`、snapshot |
| `agent.message.send.request` | `agentId`、`text`、可选 `messageId` / `activeTurnBehavior` / 图片与附件 | accepted 与 inline error；默认中断后投递，steer 向运行中轮次追加 |
| `agent.cancel.request` | `agentId` | 发送原生 interrupt 后的 snapshot；终态由 wait 确认 |
| `agent.finish.wait.request` | `agentId`、可选 `timeoutMs`，1–30000，默认 30000 | idle/error/timeout、final snapshot、lastMessage、error |

wait 不阻塞同一连接继续发 cancel；响应按 request_id 匹配，可能与其他响应交错。
断开连接只停止等待，已接纳 turn 继续运行。超时也只结束观察。Agent 的原生 turn 没有固定运行时限。
退出 server 会关闭并回收原生进程；重启后使用已保存的 handle 恢复，不创建替代 Agent。

创建要求活动 Project/Workspace 与 cwd 匹配；未给 workspaceId 时复用该目录最早的活动 Workspace，
本阶段不隐式创建 placement。归档恢复只读取原生身份，不能发送消息。lastMessage 只缓存本进程
最近完成 turn 的最后 assistant 文本，重启后不会伪装成已加载的历史。原生历史仍由各自 CLI 保存。

现已接通 initialPrompt、图片/附件、messageId 幂等、排队/steer、providerOptions/MCP、
工具策略、权限交互、timeline 和订阅。消息重试必须保持相同内容与投递策略；不确定是否接收
的原生输入不会自动重发。创建时的 Git/worktree/env 操作不由 provider 会话隐式执行。
最多 32 个 live session、64 个待处理 worker 命令、32 个并发 wait；native RPC 超时 10 秒，
JSON 行上限 2 MiB。超过预算会返回错误或终止有问题的原生连接，不静默丢失数据。

## 早期 Project 接口已废除

`project.open`、`project.list`、`project.get`、`project.close` 已从路由与能力协商中移除，
调用返回 `method_not_found`。使用上文的 `project.add.request`、`project.list.request`、
`workspace.open.request` 等 Paseo 接口注册目录和管理 Workspace。

新 Project 注册不创建根 Message、项目 SQLite、project.lock 或 Project ID/epoch 文件。
既有 `.ait-server/project.sqlite3` 等历史文件不自动迁移或删除，也不被新接口读取。
全局 `catalog.sqlite3` 继续保存 Agent presets；已有旧 Project 表保持不变且不再使用。

## Agent 配置（M1 第二个切片）

在 hello 中协商所需的以下五项 capability。配置操作不需要打开 Project 或启动 Desktop。

| Method | Params | Result |
| --- | --- | --- |
| `agent.configure` | `config`、`idempotency_key`；修改还需 `agent_id`、`expected_revision` | `operation_id`、`agent_id`、不可变 `revision` |
| `agent.get` | `agent_id`；可选 `revision` | 精确配置、revision、`recorded_at`（Unix 毫秒） |
| `agent.list` | `{}` 或 `after`、`limit`（1–50，默认 20） | 当前配置的 `agents`、`next_after` |
| `agent.default.get` | `{}` | `agent_id`（可空）、`version`（初始 0） |
| `agent.default.set` | `agent_id`（必须提供，可显式 null）、`expected_version`、`idempotency_key` | `operation_id`、历史 `selection` |

创建配置：

```json
{"type":"request","request_id":"a1","method":"agent.configure","params":{"config":{"name":"My Codex","driver_type":"codex","model":"explicit-model-id","credential_ref":null,"enabled":true},"idempotency_key":"create-agent-1"}}
```

`model` 示例是占位标识，应替换为计划使用的模型。这里只验证标识格式，不访问 Provider。
driver 当前只接受 `codex`；保存成功不代表模型已验证、凭据可用或执行已启用。
不提供 `provider.models` 或运行能力，不使用用户现有原生会话做配置探测。

修改时完整提供 `config`，并加上返回的 `agent_id` 与从 get 读到的 `expected_revision`。
新操作追加 revision，不覆盖旧配置；按旧 revision 查询仍得到原值。相同配置以新 key 提交也
产生新 revision。创建不自动设为默认；显式选择或清空默认时使用 default.get 返回的 version。
默认指向 Agent 身份，未来 Session/Run 还需冻结具体 revision；禁用默认 Agent 前先清空或换选。

字段限制：name 1–255 UTF-8 字节，不能全空白或含控制字符；model 1–128 ASCII 字节，
仅允许字母、数字、`-_.:/`。credential_ref 可省略/null，或为 `env:AIT_SERVER_CREDENTIAL_<NAME>`；
NAME 1–64 字节，以大写字母开头，其余仅大写字母、数字和下划线。数据库只保存该引用，
不会读取对应环境变量，也不验证它是否存在。不要把秘密写入 name/model；API 拒绝 api_key、
token、任意参数、endpoint 等未知字段。不支持自动加载 `.env`。

写请求沿用 1–128 字节可见 ASCII key。配置 fingerprint 包含目标、expected revision 和完整
配置，默认 fingerprint 包含目标及 expected version；重试保留这些参数。先查完成回执，再
检查当前状态，因此旧重试不会回退配置或重新设定默认。get/list 用于查询当前状态。
常见错误为 `agent_not_found`、`agent_revision_not_found`、`agent_revision_conflict`、
`agent_default_conflict`、`agent_disabled`、`agent_is_default`、`idempotency_conflict`。
I/O 和 catalog 争用分别返回可重试的 `agent_io` / `catalog_busy`。

已有独立 v1 catalog 在启动时先备份为 `<data-dir>/catalog-v1-backup-<随机名>.sqlite3`，再事务
升级到 v2，保留原项目注册和回执。全新 catalog 直接初始化 v2，项目数据库保持 v1。
备份不会自动删除；升级失败保留 v1 数据和已完成备份。v1 server 拒绝打开 v2 catalog，
恢复备份需停服显式处理，会丢失备份之后的 catalog 修改。此升级不读取旧 Ait 数据库。

## 预算和停止

- 输入 JSON message 和单帧均上限 1 MiB；包括分片累计大小。
- Agent 与 Project 共用每实例一个短阻塞任务预算，超出返回 `resource_exhausted`。
- 最多 64 个同时升级的连接，超限 HTTP 429；待 hello 的连接同样占名额。
- 每连接待发送队列最多 256 条，消息内容合计 4 MiB，正在写入的内容仍占字节预算。
- 队列满立即终止慢连接；一次写入最多等 5 秒，最后排空/Close 最多 1 秒。
- Unix 支持 SIGINT/SIGTERM；Windows 使用 Ctrl-C。停止先关闭接纳，再排空 HTTP 和已跟踪
  WebSocket 和已接纳的项目/Agent 操作；排空预算为 15 秒。超时记录关闭错误，非零退出。

客户端断开不会撤销已接纳的项目/Agent 事务；服务停止会等待这些任务。关闭期限超时时返回错误，
仍在执行的阻塞任务会继续持有 data-dir 锁，直到完成或进程结束。Tokio 的阻塞工作不能强行取消，
因此此时进程实际退出可能晚于 15 秒；当前没有硬终止阻塞线程的机制。后续 Run 生命周期仍独立于连接。
Linux/Windows 的平台验收以 CI/后续实测为准；本次本地报告记录具体已验证平台。

## 开发验证

```sh
cargo test -p server-protocol -p server-api -p server-bin -p server-domain -p server-provider -p server-metadata -p server-filesystem -p server-terminal
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

## Agent 后续 turn 配置与 Session 事件

按 [ADR-034](../decisions/adr-034-agent-config-session-events.md)，新增以下已实现方法：

| 方法 | params | 行为 |
| --- | --- | --- |
| `agent.model.set.request` | `{agentId, modelId: string \| null}` | 保存后续 turn 的模型覆盖 |
| `agent.thinking.set.request` | `{agentId, thinkingOptionId: string \| null}` | 保存后续 turn 的推理等级覆盖 |
| `agent.config.apply.request` | `{agentId, config: {modelId?, thinkingOptionId?}}` | 一次落盘整个配置补丁 |
| `session.events.set_subscription.request` | `{events: string[], notifications?: boolean}` | 创建独立连接订阅，返回 subscriptionId |
| `session.heartbeat` | 下文客户端 event | 更新进程内活动与焦点，不返回响应 |

三个配置方法返回 `{agentId, accepted, error, notice}`。省略字段保持原值，null 清除宿主覆盖并
交给原生 Provider 继承；它不保证回到创建时的模型。修改不打断正在执行的 turn；活动期间成功
修改会返回下一轮生效的 notice。accepted 只表示保存成功，真实模型可用性仍在执行时验证。
Codex thinking 支持 none/minimal/low/medium/high/xhigh；模型非空、无控制字符且最多 256 字节。
归档 Agent 拒绝修改。批量 config 暂不接受 modeId 或 featureValues；sandbox 仍是只读。

Session 事件支持 `agent_attention_required`、`status.daemon_config_changed`、
`status.server_info`。未实现的事件类别明确返回 unsupported_capability。事件沿用通用信封：

```json
{"type":"event","method":"agent_attention_required","params":{"subscriptionId":"...","agentId":"...","reason":"finished","timestamp":"2026-09-24T00:00:00.000Z","shouldNotify":false}}
```

订阅无初始快照、无持久化重放；订阅响应在事件之前入队。每次调用创建独立 owner，
`subscription.release.request` 的 `{subscriptionId}` 仅释放本连接订阅；断连全部释放。
与其他业务订阅共享每连接 16 个配额。Agent 完成/失败事件在终态落盘后发布，取消不触发提醒。
配置变更事件的 params 包含 status、config、subscriptionId；服务关闭事件包含 status、info、
subscriptionId，info.lifecycle 为 draining。可通过 server.info 主动读取初始服务状态。

心跳需要协商 `session.heartbeat`，使用 event 信封：

```json
{"type":"event","method":"session.heartbeat","params":{"deviceType":"web","focusedAgentId":null,"focusedTerminalId":null,"lastActivityAt":"2026-09-24T00:00:00.000Z","appVisible":true}}
```

时间使用 RFC3339；可选 appVisibilityChangedAt 也需合法。未来 lastActivityAt 在接收时截断，
活动有效期 180 秒。新鲜、可见且聚焦目标 Agent 的连接会抑制所有提醒；其他情况下只让最近
活动、启用 notifications 的订阅连接得到一次 shouldNotify=true。其余订阅仍收到状态事件。
无心跳或过期连接的 shouldNotify=false；不投递 push，不自动清除 Agent attention，
focusedTerminalId 仅兼容解析。这里的 Session 表示连接协议，不是领域 Message 引用。


## Terminal PTY

Terminal 分组全部 10 个方法由 `server-terminal` 实现。创建前先打开 Workspace：

```json
{"type":"request","request_id":"open","method":"workspace.open.request","params":{"cwd":"/absolute/project"}}
{"type":"request","request_id":"create","method":"terminal.create.request","params":{"cwd":"/absolute/project","size":{"rows":24,"cols":80}}}
{"type":"request","request_id":"stream","method":"terminal.subscribe.request","params":{"terminalId":"<returned-id>","restore":{"mode":"visible-snapshot","scrollbackLines":200}}}
{"type":"event","method":"terminal.input","params":{"terminalId":"<returned-id>","message":{"type":"input","data":"pwd\r"}}}
```

hello 需逐项协商相应 capability；输入使用 event，其他九项为 request。订阅响应返回
`subscriptionId` 和 0–255 的连接内 `slot`。二进制消息以 opcode/slot 两字节开头：0x01 output、
0x02 input、0x03 resize JSON、0x04 legacy state JSON、0x05 ANSI restore。
仅协商 `terminal.input` 且持有该 slot 的连接可发送 binary input/resize。

`terminal.list.subscribe.request` 要求 cwd，可加 workspaceId；返回初始列表及 subscriptionId，
后续事件为 `terminal.list.changed`。退出事件为 `terminal.stream.exit`。通用 subscription release
和专用 unsubscribe 都释放本连接订阅，断线不会杀终端。重连后重新 subscribe；重启 server 会杀掉
并清空终端。`terminal.capture.request` 的 start/end 是闭区间，负数从尾部计算。

resize `{type:"resize",rows,cols,intent:"claim"}` 获取尺寸控制；`intent:"update"` 仅更新该连接
拥有的尺寸。终端最多 32 个，输入每次不超过 64 KiB；尺寸不超过 100×200 且总 visible cells
不超过 10,000。屏幕/滚动历史有界；慢 observer 会收到新快照，超时连接关闭。

`agent.items.close.request` 支持 terminalIds；Workspace/Project archive/remove 后最多约 250 ms
开始清理所属 PTY。setup/script executor 的逻辑 terminalId 暂不对应此处的真实 PTY；activity hooks
和代理健康检查仍保持原边界。详细语义及仿真差异见 [ADR-033](../decisions/adr-033-server-terminal.md)。

## App 浏览器接入

`apps/app` 的浏览器连接使用 Bearer 换取短时一次性 WebSocket 票据。启动 server 时添加
`--web-origin http://localhost:8081`（或 TOML `web_origins`），允许对应的本地页面来源。
默认 listener 仍为 loopback；远端来源、任意端口通配和 URL 凭据不被接受。

完整本地启动入口与原生端说明见 [App README](../../apps/app/README.md)，协议和边界见
[ADR-049](../decisions/adr-049-app-rust-browser-transport.md)。

## Claude Code Provider

独立 server 已注册 `claude`，使用本机 Claude Code 的认证、工具和原生会话。
安装、二进制路径覆盖、模式与接口示例见 [Claude Code 使用说明](claude-code.md)。
