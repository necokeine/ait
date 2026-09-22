# Paseo WebSocket 接口移植：第九阶段

- 日期：2026-09-23；分支：`new`。
- 基线：`1b8b8ed`；Paseo 来源：`2c8e8a826810337492cc5a38bb0bbd705b6fb632`。
- 决策：[ADR-026](../decisions/adr-026-canonical-paseo-websocket-surface.md)。

## 已接通方法

本阶段接通 Git 分支与修改分组的 13 个规范 WebSocket 方法。累计已接通 71 个规范方法，剩余 120 个
catalog 条目仍为 catalog-only。

| 规范方法 | Paseo 来源名 | 状态与行为 |
| --- | --- | --- |
| `checkout.branch.validate.request` | `validate_branch_request` | 解析 local/origin-only/not-found，返回 Paseo 的 string error shape |
| `checkout.branch.suggestions.request` | `branch_suggestions_request` | local/origin 合并、query/prefix/date 排序、limit 与 ahead/behind |
| `checkout.branch.switch.request` | `checkout_switch_branch_request` | 干净树准入；local checkout 或创建 origin tracking branch |
| `checkout.rename_branch.request` | 同名 | 用 Paseo lowercase slug 规则重命名当前 branch |
| `checkout.commit.request` | `checkout_commit_request` | `addAll` 默认 true，显式消息 commit |
| `checkout.merge.request` | `checkout_merge_request` | 把当前 branch merge/squash 到 local base checkout |
| `checkout.merge_from_base.request` | `checkout_merge_from_base_request` | 把 most-ahead base merge 到当前 branch，默认要求干净 |
| `checkout.pull.request` | `checkout_pull_request` | 当前 branch 的 origin/upstream pull，失败清理 merge/rebase state |
| `checkout.push.request` | `checkout_push_request` | pushRemote/refspec、upstream 或 `origin current` 三段解析 |
| `checkout.discard_changes.request` | 同名 | literal pathspec；恢复 tracked，删除所选 untracked |
| `checkout.stash.save.request` | `stash_save_request` | `--include-untracked`，写 `paseo-auto-stash:` 前缀 |
| `checkout.stash.pop.request` | `stash_pop_request` | 按非负 `stashIndex` pop |
| `checkout.stash.list.request` | `stash_list_request` | 解析 index/message/branch/isPaseo，默认只列 Paseo stash |

旧下划线名称不作为 wire alias。请求和响应继续使用统一 envelope，Paseo payload 位于 `params`/`result`，
不复制第二层 `requestId`。

## 实现边界

`server-protocol::checkout` 新增 Paseo branch validate/suggestion/switch/rename、commit/merge、discard 和 stash
DTO；validate/suggestions 保留原版 string error，其余 mutation 保留 `CheckoutError`。`server-ports::checkout`
扩展全新的 `CheckoutRuntime` typed boundary；application 只做 use-case delegation；生产 binary 仍组装新的
`server-workspace::LocalCheckout`，没有依赖旧 Ait domain/application/adapter。

读操作继续使用 10 秒预算；全部写操作使用 120 秒 deadline、4 MiB stdout 和 64 KiB stderr 上限。Git 只接收
参数数组，不经过 shell，stdin 关闭，继承的 `GIT_*` 被清理，optional locks/fsmonitor/color/diff prefix 被固定。
API 的 blocking-job semaphore 限制并发，Checkout mutex 串行化进程内读写，避免两个 connection 同时修改
repository。

branch suggestion 合并 `refs/heads` 与 `refs/remotes/origin`，去掉 `refs/...`/`origin/`，过滤 remote HEAD，
query 是不区分大小写的 substring，prefix 优先，再按最新 committer date 和名称排序。local 与 origin 同时存在
时用 rev-list 计算 divergence。switch 先要求 clean tree；origin-only branch 用 `checkout -b --track`。

merge-to-base 先确定当前 branch 和显式/推导 base，去掉 origin 前缀，优先找到已经 checkout base 的 linked
worktree；没有时在当前 checkout 临时切换并最终恢复。普通 merge 与 squash+commit 均用 120 秒预算。
merge-from-base 在 local/origin 同名 ref 间选择独有提交更多的一侧。两种 merge 检测 unmerged paths，尝试
`merge --abort` 并映射 `MERGE_CONFLICT`。pull 失败尝试同时清理 merge/rebase state。

discard 先用 literal reset 把选择路径 unstaged；unborn HEAD 时退到 `rm --cached --ignore-unmatch`；随后从
porcelain NUL 输出区分 tracked/untracked，再分别 checkout/clean。stash save 包含 untracked，list 解析 Git
stash ref index 和 subject 中的 Paseo prefix。

## 与 Paseo 的对齐和差异

1. method payload、response field/null/omission、默认值、error 类型、branch resolution/suggestion 排序、
   clean preflight、rename slug、merge 方向、most-ahead base、conflict abort、literal discard 与 stash prefix/filter
   均以固定 Paseo 实现和对应测试为基准。
2. Paseo 在 commit message 为空时调用 Provider-backed `gitMetadataGenerator`。独立 server 尚无 Provider
   runtime，所以不能生成消息；它保留生成器仍为空后的原版结果，返回 `UNKNOWN / Commit message is required`。
3. Paseo 的 `notifyGitMutation` 会更新 Workspace Git cache、invalidate Forge、安排 diff refresh，并在 branch
   switch/rename 后立即发布 Workspace update。当前没有 mutation observer/Forge cache/Workspace status event；
   已建立的 200 ms diff polling subscription 会在下一轮读取到文件变化，branch/status 需要客户端重新请求。
4. Paseo mutation service 以 repository root 和 mutation queue 协调相关 checkout。当前 API 用一个 Checkout
   mutex 串行化所有 repository 的 checkout 操作；一致性更保守，但不同 repository 的 Git 写也不能并行。
5. Paseo 从 Workspace metadata 取得 stored base，并有更完整的 repository/default-branch resolver。当前沿用
   第八阶段实时 Git 推导：`origin/HEAD`、`main`、`master` 或 current branch；metadata 已改名/删除时可能不同。
6. push 对齐 pushRemote 的单个 `HEAD:refs/heads/...` refspec、现有 upstream 和 origin fallback。Paseo 在显式
   pushRemote 成功后额外更新 local remote-tracking ref；当前不写该 ref，status 要等本地 fetch 或其他 Git
   更新后才能反映 remote reachability。多个/通配 push refspec 同样不作为 configured target。
7. Git 网络命令使用本机 Git credential/config，stdin 关闭且 120 秒超时。测试只使用本地 bare remote，没有
   验证真实 forge、credential helper 或网络失败矩阵。
8. porcelain/stash/worktree parser 覆盖普通 UTF-8、空格和 rename NUL entry；无法表示为 UTF-8 的 path/output
   仍通过 lossy conversion，未达到 Paseo Node Buffer/path 处理的完整范围。

## 测试执行

对应移植的 Paseo 测试包括：canonical capability 和 camelCase/default/optional shape；local、origin-only、
missing 与 unsafe branch validate；query normalization、suggestion flags/divergence；clean switch、remote tracking
checkout 与严格 rename slug；commit 默认 staging；tracked/untracked discard；stash save/list/filter/pop；
merge-from-base、merge-to-base、原 branch 恢复与 conflict abort；本地 bare remote pull/push；以及真实 binary
通过一个 WebSocket 完成全部 13 个 request。

本阶段新增 14 个新 server 测试：protocol 2、API 3、真实 Git adapter 8、真实 binary WebSocket 1。最终验证：

```text
CARGO_TARGET_DIR=/tmp/ait-phase9-cov-target cargo llvm-cov -p server-bin -p server-api \
  -p server-application -p server-domain -p server-ports -p server-protocol \
  -p server-storage -p server-workspace --json --summary-only --no-fail-fast -j1
  254 passed, 0 failed, 0 ignored
CARGO_TARGET_DIR=/tmp/ait-phase9-workspace-target cargo test --workspace --no-fail-fast -j1
  746 passed, 0 failed, 5 ignored
cargo clippy --workspace --all-targets -- -D warnings
  passed
cargo clippy -p server-protocol -p server-ports -p server-application -p server-workspace \
  -p server-api -p server-bin --all-targets --all-features -- -D warnings
  passed
cargo fmt --all -- --check
  passed
```

coverage 运行中的 12 个新 server test target 共 254 个测试通过。生产 Rust 源码为 11,349/13,085 行，
行覆盖率 86.73%；本阶段三个 checkout 实现文件为 1,770/2,112 行，行覆盖率 83.81%。相比第八阶段，
server package 总覆盖率从 86.96% 降低 0.23 个百分点；新增大量真实 Git error/平台分支后仍高于 80%。
HTML 报告已生成到 `/tmp/ait-phase9-cov-target/llvm-cov/html/index.html`。完整 crate 级数据、命令、基线和未覆盖范围在
[phase 9 coverage artifact](paseo-websocket-surface-phase-9-coverage.json)。本地 HTML 未作为共享 artifact；
JSON 摘要已提交供审查。主要未覆盖项是超时/输出超限、unborn-HEAD discard fallback、pull 冲突清理和
configured push 后 tracking-ref 刷新差异；这些需要受控 fake Git 或更多独立进程 fixture。

第一次独立 full-workspace 运行中，既有 daemon 测试
`macos_gui_path_reaches_codex_in_the_worker` 曾因进程未在 ready 窗口内启动而失败；它不调用新 server。
该 exact test 随即通过，完整 workspace 重跑也通过。第一次补强测试后的 coverage 运行中，既有
`server-workspace::tests::freezes_instructions_and_excludes_runtime_with_exclusive_leases` 偶发返回 `Busy`；其
exact rerun 和完整 coverage rerun 通过。最终 workspace 为 746/0/5，coverage 为 254/0/0。额外尝试 whole-workspace
`--all-features` clippy 时，既有 `ait-worker` test 对 `AgentMode::Mock` 的 match 不完整而无法编译；本阶段修改的
六个 server package 加 binary 在 `--all-features` 下严格 clippy 通过，规定的 default-feature workspace
clippy 也通过。
