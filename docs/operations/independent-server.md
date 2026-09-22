# 独立 server：使用与协议

> 当前 binary 已接通规范化后的 Paseo Project/Workspace、daemon/config、Workspace 标签、
> Worktree、Workspace setup/script 与 Agent runtime 目录/元数据生命周期
> WebSocket 接口，详见
> [ADR-026](../decisions/adr-026-canonical-paseo-websocket-surface.md)。下文的
> `project.open/list/get/close` 仍是过渡租约接口，不是 Paseo descriptor。

`server` 与现有 daemon 并存，当前支持本机服务、WebSocket、独立 Git 项目、Paseo Agent runtime
snapshot 和版本化 Agent 配置。内部代码全部来自新建的八个 `server-*` package；Session、Run 和
Provider 执行尚未实现。
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
| `subscription.release.request` | `subscriptionId` | 幂等释放本连接的 connection-owned 订阅 |

状态通知形如 `{"type":"status","subscription_id":"…","lifecycle":"ready"}`。
每连接最多 16 个订阅，断开即清理，重连需要重新订阅。退出时 best-effort 发出 `draining`。
这是临时连接通知，没有持久序号/回放语义；业务 outbox 与 cursor 留到后续 M1 切片。

错误形如 `{"type":"error","request_id":"r1","code":"method_not_found","message":"Unknown method","retryable":false}`。
尚未实现的业务 method 返回 `method_not_found`，不返回空成功。有效请求的错误保留 request ID。
不合法 envelope、重复 hello、二进制消息与握手失败会关闭连接；未知 method 或错误参数可修正后
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
不执行 teardown、Agent/terminal 清理，因此 `removedAgents` 为空。`checkoutSource` 与
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
| `agent.items.close.request` | `agentIds`；当前 `terminalIds` 必须为空 | 成功归档的 `agents` 与空 `terminals` |

runtime snapshot 保存于 `<data-dir>/agents/agents.json`，shape 来自 Paseo `StoredAgentRecord`，与下文
ADR-024 Agent preset catalog 是两类数据。list/history 会用 Workspace/Project registry 生成 placement，
过滤 internal 或 placement 已不存在的记录；支持 label、project key、status、attention、thinking、archive
过滤，最多每页 200 条。当前 cursor 是十进制 offset。get 仍可读取 placement 已不存在的公共记录。

archive 会清除 attention，把 running/initializing snapshot 收敛为 idle，并递归归档同 Workspace 的 delegated
child；跨 Workspace 或带 open-tab label 的 child 会 detach。delegated Agent 的 detach 删除
`paseo.parent-agent-id` 和全部 `paseo.open-agent-tab.*` label，已经没有 parent label 时保持不变。delete
是永久删除；close-items 独立处理每个 Agent，按 Paseo 行为只返回成功项。

当前没有 Provider runtime，所有 stored Agent 都以 `providerUnavailable:true` 返回，`persistence` 为 null，
没有 active turn、available modes 或 pending permissions。list 请求中的 `subscribe`/`sync` 和非空
`terminalIds` 返回 `unsupported_capability`。Provider 创建、恢复、消息、取消、timeline 与真正 Terminal
关闭等待后续切片。完整对齐范围和差异见第六阶段报告。

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
