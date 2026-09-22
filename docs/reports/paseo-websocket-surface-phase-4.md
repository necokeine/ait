# Paseo WebSocket 接口移植：第四阶段

- 日期：2026-09-22；分支：`new`。
- 基线：`3a6a652`；Paseo 来源：`2c8e8a826810337492cc5a38bb0bbd705b6fb632`。
- 决策：[ADR-026](../decisions/adr-026-canonical-paseo-websocket-surface.md)。

## 已接通方法

本阶段接通 Worktree 分组的 3 个 WebSocket 方法。累计已接通 33 个规范方法，剩余 158 个 catalog
条目仍为 catalog-only。

| 规范方法 | Paseo 来源名 | 状态与行为 |
| --- | --- | --- |
| `workspace.worktree.list.request` | `paseo_worktree_list_request` | 只列出当前 data-dir owned root 下、属于所选 repository 的 linked worktree |
| `workspace.worktree.create.request` | `create_paseo_worktree_request` | branch-off/existing-branch checkout、registry 注册、失败回滚与 response 后 workspace event |
| `workspace.worktree.archive.request` | `paseo_worktree_archive_request` | workspace/worktree scope、active 引用计数、ownership gate 与幂等磁盘清理 |

原下划线名称没有注册为 alias。请求继续使用新 server 的
`{type:"request",request_id,method,params}` envelope，结果放在 correlated `result`；list/archive 的
inline checkout error 保留 Paseo 的 `NOT_GIT_REPO`、`NOT_ALLOWED`、`MERGE_CONFLICT`、`UNKNOWN`
形状，create 保留 lowercase `errorCode`。

## 实现边界

`server-protocol::worktrees` 复制 Paseo list/create/archive 的公开字段、archive 默认 scope、legacy
`deleteWorktreeFromDisk` 忽略规则、first-Agent context、attachment normalization、checkout source
和 response payload。Paseo 内部 `archiveCommand` 的 `worktreeSlug` 不是公开 WebSocket schema 字段，
因此新协议也会把未知同名字段剥掉，不把内部参数误发布成 wire capability。

`server-ports::worktrees::ManagedWorktrees` 定义阻塞 Git 边界；
`server-workspace::LocalManagedWorktrees` 实现：

- 从 source cwd 解析 checkout root、Git common dir、main repository root 与相对子目录；
- 使用 Paseo 相同的 SHA-256 前 64 bit/base36 规则生成 8 字符 repository hash；
- 在 `<data-dir>/worktrees/<hash>/<slug>` 下创建 linked worktree，branch/path collision 使用数字后缀；
- branch-off 优先 local base、再找 `origin/<base>`；checkout 缺 local branch 时尝试从 origin 获取；
- list 解析 `git worktree list --porcelain`，按 managed project root 过滤，并返回 branch、HEAD 与创建时间；
- ownership 对 realpath/symlink 敏感，只接受 `<hash>/<slug>` 及其后代；删除先尝试
  `git worktree remove --force`，再清理残留目录并 prune；
- Git 子进程无 stdin、禁止 credential prompt、清除继承的 `GIT_*`、限制输出并设置 10/120 秒期限。

`server-application::worktrees::Worktrees` 负责 registry 协调。创建成功后优先复用 source cwd 或
repo root 的 active Workspace 所属 Project；显式 `projectId` 必须存在且 active；否则按 main repo
root 新建 Project。Workspace record 使用 `kind=worktree`、`isPaseoOwnedWorktree=true`、精确
`worktreeRoot`、`mainRepoRoot`、branch 与 creation-time base ref。registry 失败会删除刚创建的
worktree；删除也失败时同时返回原失败和 rollback failure。

归档的 workspace scope 只改变一个 record；最后一个 active 引用消失时才删 owned worktree。
worktree scope 先归档 root 或其子目录下的全部 active Workspace，再删目录。显式 `workspaceId`
始终以该 record 的 durable backing placement 决定清理目标，也支持重复归档时移除上次遗留的 owned
目录；请求里另一个 path 不会删除错误 worktree。非 owned path 仍可在 workspace scope 下归档精确
匹配的 record，但不能触发目录删除；worktree scope 直接返回 `NOT_ALLOWED`。

## 与 Paseo 的对齐和差异

1. list 的 `repoRoot` 优先级、缺少 cwd/repoRoot 的 inline error、managed-root 过滤、branch/HEAD/null
   形状与 Paseo 对齐。
2. create 的 slugify/50 字符截断、branch-off 默认、base ref 查找、existing branch collision、path
   collision、existing branch checkout、source-relative cwd 映射、`paseo.json` create-new copy、
   Project 选择、Workspace placement、registry 失败 rollback、first prompt provisional title 与 Paseo
   对齐。
3. Paseo 未提供 slug 时使用 `mnemonic-id`；当前 Rust adapter 使用合法的随机
   `worktree-<8 hex>`。identity 不同，唯一性与 wire contract 相同。
4. Paseo 在 worktree root 写 metadata，保存 exact comparison ref、change-request lookup、runtime
   port 和首次 Agent 自动 branch-name 状态。当前只把 creation-time comparison base 写进 Workspace
   registry，尚未创建这份 metadata；后续 checkout diff、setup 与 Agent auto-name 接入时补齐。
5. Paseo 的 `checkoutSource`/`githubPrNumber` 通过 Forge service 解析和 fetch change-request refs，
   并为跨仓库来源写 untrusted provenance。新 server 尚无 Forge service，这两种输入返回
   `errorCode:"unknown"` 与明确 message，不创建 Git/registry 数据。
6. Paseo create response 后异步执行 setup、启动 terminals/scripts，并维护 setup snapshot；当前
   `setupTerminalId` 为 null，不执行 setup。Paseo archive 会先处理 Agent、terminal、script 和
   teardown；当前只协调 Workspace registry 与 Git，因此 `removedAgents` 始终为空。
7. Paseo recursive removal 有短退避重试；Rust adapter 先让 Git 删除、再执行一次 recursive remove。
   Windows 短暂文件占用下可能比 Paseo 更早返回失败，失败不会伪装成功。
8. Paseo 发顶层 `workspace_update`；新 server 按 ADR-026 在 create response 之后发送
   `{type:"event",method:"workspace.update",params}`。workspace payload 与 `kind:"upsert"` 保持一致。
9. attachment normalization 会接受非数组为 empty，并过滤不符合已知 attachment discriminator、
   MIME 与必填字段的数组项；因为本阶段不创建 Agent，合法 attachment 暂不进入持久化或执行。

## 测试执行

从 Paseo `messages.test.ts`、`worktree.posix.test.ts`、`paseo-worktree-service.test.ts`、
`worktree-session.test.ts` 的对应场景移植：archive scope 默认/未知字段、attachment normalization、
positive change-request number、legacy/current first-Agent context、repository hash 隔离、managed-only
list、nested cwd、untracked config seed、default/base branch、branch/path collision、unknown branch、
checked-out branch copy、descendant ownership、external path rejection、registry rollback、source Project
复用、provisional title、workspace reference counting、record-owned backing、residual cleanup、directory
workspace deletion guard、worktree-scope bulk archive 和真实 WebSocket response/event 顺序。

本阶段新增 27 个测试：protocol 9、application 10、workspace adapter 6、API 1、真实 binary
WebSocket 1。阶段性验证：

```text
cargo test -p server-protocol worktrees
  9 passed, 0 failed
cargo test -p server-application worktrees
  10 passed, 0 failed
cargo test -p server-workspace worktrees
  7 passed, 0 failed（其中 1 个为已有 worktree 过滤命中的旧测试）
cargo test -p server-api worktrees
  1 passed, 0 failed
cargo test -p server-bin --test process binary_creates_lists_and_archives_canonical_worktrees
  1 passed, 0 failed
cargo clippy -p server-protocol ... -p server-bin --all-targets -- -D warnings
  passed
```

全新 server 的 12 个 test target 共 170 个测试通过，0 失败、0 ignored。`cargo llvm-cov`
测得生产 Rust 源码 7,300/8,269 行，行覆盖率 88.28%；本阶段 4 个带可执行行的 Worktree
模块为 1,048/1,273 行，行覆盖率 82.33%。完整的 crate 级数据、命令和基线记录在
[phase 4 coverage artifact](paseo-websocket-surface-phase-4-coverage.json)。

全 workspace 严格 Clippy 已通过。全 workspace test 回归结果如下：

```text
cargo clippy --workspace --all-targets -- -D warnings
  passed
cargo test --workspace --no-fail-fast -j1
  72 ordinary targets: 661 passed, 0 failed, 5 ignored
  doctests: 1 passed, 0 failed
```
