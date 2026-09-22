# Paseo WebSocket 接口移植：第八阶段

- 日期：2026-09-23；分支：`new`。
- 基线：`a0dcbdb`；Paseo 来源：`2c8e8a826810337492cc5a38bb0bbd705b6fb632`。
- 决策：[ADR-026](../decisions/adr-026-canonical-paseo-websocket-surface.md)。

## 已接通方法

本阶段接通 Git checkout 状态、Diff 和提交历史分组的 7 个规范 WebSocket 方法。累计已接通 58 个规范方法，
剩余 133 个 catalog 条目仍为 catalog-only。

| 规范方法 | Paseo 来源名 | 状态与行为 |
| --- | --- | --- |
| `checkout.status.get.request` | `checkout_status_request` | 返回 Git/root/branch/dirty/base/upstream/remote/managed-worktree 状态和 inline error |
| `checkout.refresh.request` | 同名 | 强制执行一次新的本地 Git 状态读取 |
| `checkout.diff.get.request` | 同名 | 返回 uncommitted 或 merge-base 到 `HEAD` 的 path-sorted structured diff |
| `checkout.diff.subscribe.request` | `subscribe_checkout_diff_request` | 返回初始 snapshot，随后用 `checkout.diff.update` 发布去重后的变化 |
| `checkout.diff.unsubscribe.request` | `unsubscribe_checkout_diff_request` | 释放当前 connection 的指定 diff 订阅 |
| `checkout.commits.list.request` | 同名 | 返回全部 Workspace commits，再附最多十条 fork-point base context |
| `checkout.commits.file_diff.request` | 同名 | 返回指定 commit/file 的 textual structured diff；缺失或 binary 为 null |

旧下划线名称不作为 wire alias。请求和响应继续使用新 server 的统一 envelope；Paseo method payload 位于
`params`/`result`，不复制第二层 `requestId`。服务端事件也按用户要求规范为 `checkout.diff.update`，而
Paseo 固定快照的来源名是 `checkout_diff_update`。

## 实现边界

`server-protocol::checkout` 复制 Paseo checkout status、inline error、compare、structured diff、commit 和
file-diff shape。status 使用条件序列化：非 Git 结果不带 `mainRepoRoot`，Git 结果即使没有 linked main
repository 也显式返回 `mainRepoRoot:null`，与 Paseo status projection 一致。`ignoreWhitespace` 默认 false，
`baseRef` 和可选兼容字段保持 camelCase/null/omission 语义。

`server-ports::checkout::CheckoutRuntime` 是新的阻塞 Git port；`server-application::checkout::Checkout` 只做
use-case delegation；生产 binary 用新的 `server-workspace::LocalCheckout` 组装。没有依赖或复用旧 Ait
domain/application/adapter 组件。

本地 adapter 只通过参数数组启动 `git`，不经过 shell；清理继承的 `GIT_*` 环境，关闭 optional locks、
fsmonitor、color 和可配置 diff prefix。所有读操作有 10 秒 deadline，stderr 为 64 KiB，普通结果为
256 KiB，diff/commit 输出为 4 MiB；超时、输出超限和 Git error 转换成有界 inline error。cwd 会先展开 `~`
并 canonicalize，commit file path 必须是非空 repository-relative normal-component path。

status 从实时 Git 计算 worktree root、main repository root、branch、dirty、default/base、ahead/behind、精确
upstream、remote 和 URL。managed ownership 同时要求路径精确位于
`<data-dir>/worktrees/<repository-hash>/<slug>`，并且 Git common dir 指向不同的 main repository；仅把普通
repository 放到相同目录层级不会被标记为 owned。

uncommitted diff 合并 tracked/staged 与 untracked 文件；base diff 用显式或推导 base 的 merge-base 到
`HEAD`，不会混入 working tree。parser 输出 file/hunk/line、rename old path、new/deleted、addition/deletion
和 binary status，并按 path 排序。commit list 用 `base..HEAD` 取得全部 Workspace history，再从 merge-base
向后附十条 base history；`git rev-list HEAD --not --remotes` 标记 local-only commit，raw/numstat 同时生成
added/modified/deleted/renamed 文件统计。merge commit 按 first parent 计算文件变化。

订阅属于物理 WebSocket connection。服务端先读取并返回 initial snapshot，response 成功排队后才启动轮询；
每 200 ms 在共享有界 blocking-job semaphore 下读取，序列化 snapshot fingerprint 相同则不发送。相同 ID
的新订阅插入时 drop 旧 RAII owner；显式 unsubscribe、`subscription.release.request`、连接断开和 server
drain 都会取消任务。subscription 总数与既有 status/label 订阅共用每 connection 16 个上限。

## 与 Paseo 的对齐和差异

1. method payload、status union、error code、compare 默认值、structured diff/commit/file shape、inline error、
   commit 排序、fork-point base context、十条 base 上限、first-parent merge、remote/base 标记、订阅初始响应、
   同 ID replacement 和 connection cleanup 均以固定 Paseo 实现与测试为基准。
2. Paseo diff manager 使用共享 filesystem observer、Workspace Git snapshot、150 ms debounce、in-flight coalesce
   和同 target watcher 复用。当前独立 server 使用每订阅 200 ms bounded polling；它能去重并最终发送变化，
   但相同 cwd/compare 的多个订阅不会共享一次 Git read，base subscription 也会周期读取。
3. Paseo refresh 会 invalidate Forge cache、强制刷新含 Forge facts 的 Workspace snapshot，并安排所有相关
   diff subscription 更新。当前 refresh 是一次无缓存的本地 status read，不 fetch remote、不广播 status，
   也不强制 diff event；remote counts 只反映本地 remote refs。
4. Paseo 从 `worktree.json` 读取 exact stored base ref，并结合 workspace observer/cache 选择 comparison ref。
   当前没有 Paseo metadata，owned 判定来自 managed path layout 加 linked-worktree common-dir，base 从实时 Git
   的 `origin/HEAD`、`main`、`master` 或 current branch 推导；保存的 base 已删除、改名或指向同名不同 remote
   时可能与 Paseo 不同。
5. Paseo 的 Forge/remote resolver 能处理 branch tracking remote、fork 与更多认证状态。当前 status 的 URL
   选择 `origin`，没有 `origin` 时退到第一个 remote；`hasRemote` 表示存在任一 remote，因此非 origin-only
   repository 的投影与 Paseo `remote.origin.url` 语义不同。
6. Paseo 对总 diff 和单文件分别限额，可保留其他文件并为单文件返回 `too_large`/`binary` placeholder；当前
   聚合输出超过 4 MiB 时返回空 files 和 `diffTooLarge:true`。普通 binary diff 有 `binary` placeholder，
   commit-file binary 与 Paseo 一样返回 null。
7. Paseo 用 syntax highlighter 填充可选 line `tokens`；当前保留协议字段但不生成 token。Git quoted path、
   tab/newline 文件名和无法表示为 UTF-8 的命令输出也没有 Paseo parser 的完整处理，普通 UTF-8 path、空格、
   rename 和 binary 已覆盖。
8. Paseo `assertSafeGitRef` 允许安全的 branch/tag/commit-ish；当前 file-diff 的 `sha` 限制为 4–64 位十六进制，
   因而不接受 branch/tag 名。无效 sha/path 当前映射 `NOT_ALLOWED`，Paseo 的普通 `Error` 会映射 `UNKNOWN`。
9. 新 server 的所有 Git 工作共享 API blocking-job semaphore，单个 Git 进程还有独立 deadline/output budget；
   Paseo 使用 Workspace Git service 自身的 cache、observer 和 concurrency 配置。两者过载时的排队与刷新时序
   不完全相同。

## 测试执行

对应移植的 Paseo 测试包括：canonical capability 与 camelCase/default/null shape、inline error；真实 Git 的
Git/non-Git status、linked owned 判定、tracked/staged/untracked diff、base diff 排除 working changes、commit
newest-first、remote reachability、全部 Workspace history、十条 base 上限、base 前进后的 fork point、merge
first-parent、rename/modify/delete 文件状态、commit file diff 和路径拒绝；API projection；真实 binary 的七个
request、初始 diff、同 ID replacement、live update、unsubscribe 和 refresh。

本阶段新增 17 个测试：protocol 5、API 3、真实 Git adapter 8、真实 binary WebSocket 1。最终验证：

```text
cargo llvm-cov -p server-bin -p server-api -p server-application -p server-domain \
  -p server-ports -p server-protocol -p server-storage -p server-workspace \
  --json --summary-only --no-fail-fast -j1
  240 passed, 0 failed, 0 ignored
cargo test --workspace --no-fail-fast -j1
  730 passed, 0 failed, 5 ignored
cargo clippy --workspace --all-targets -- -D warnings
  passed
cargo fmt --all -- --check
  passed
```

coverage 运行中的 12 个新 server test target 共 240 个测试通过。生产 Rust 源码为 10,582/12,169 行，
行覆盖率 86.96%；本阶段三个独立实现模块为 1,004/1,196 行，行覆盖率 83.95%。protocol checkout DTO、
手写 status serializer 与 checkout port 的可执行行分别计入 protocol/ports crate；完整 crate 级数据、命令和
基线记录在 [phase 8 coverage artifact](paseo-websocket-surface-phase-8-coverage.json)。

早期非 coverage package 运行中，既有
`server_workspace::tests::freezes_instructions_and_excludes_runtime_with_exclusive_leases` 曾两次偶发返回
`Busy`；该测试不调用 checkout 模块。其 exact rerun、串行 `server-workspace` target、coverage 运行和最终
全 workspace 回归均通过，报告保留这个环境波动供后续跟踪。
