# Paseo WebSocket 接口移植：第七阶段

- 日期：2026-09-22；分支：`new`。
- 基线：`1f758c5`；Paseo 来源：`2c8e8a826810337492cc5a38bb0bbd705b6fb632`。
- 决策：[ADR-026](../decisions/adr-026-canonical-paseo-websocket-surface.md)。

## 已接通方法

本阶段接通 Workspace attention 与归档恢复分组的 4 个规范 WebSocket 方法。累计已接通 51 个规范方法，
剩余 140 个 catalog 条目仍为 catalog-only。

| 规范方法 | Paseo 来源名 | 状态与行为 |
| --- | --- | --- |
| `workspace.clear_attention.request` | 同名 | 接受一个或多个 Workspace ID；逐 Workspace 清除非 permission attention，保留部分成功结果 |
| `workspace.mark_unread.request` | 同名 | 在 active Workspace 中选择最新的 finished/read root Agent，并写入 finished attention |
| `workspace.recovery.inspect.request` | 同名 | 只读判定 `unarchive`、`restore` 或 Paseo 的六类 unavailable reason |
| `workspace.recovery.restore.request` | 同名 | 取消归档，或先按原路径和原分支恢复 managed worktree；成功后发布 `workspace.update` |

请求只注册上述 dotted wire name。请求和响应使用新 server 的统一 envelope；Paseo method payload 位于
`params`/`result`，不复制第二层 `requestId`。

## 实现边界

`server-application::workspace_state::WorkspaceState` 组合第六阶段的 durable Agent runtime registry、
Paseo Project/Workspace registry 和本阶段新增的 `WorkspaceRecoveryRuntime` port。生产 binary 传入共享的
file-backed registry 实例，因此 attention 写入、目录读取和恢复后的 Workspace 投影观察同一份内存缓存和
原子 JSON writer。

clear-attention 先验证每个 Workspace active，再按完全相同的 `workspaceId` 选择 public、active、
`requiresAttention` 且 reason 不是 `permission` 的 Agent。每个 Agent 立即持久化；一个 Workspace 失败不会
撤销之前 Workspace 或 Agent 的成功写入。响应同时包含 flattened `clearedAgentIds` 和逐 Workspace `results`。

mark-unread 通过 `paseo.parent-agent-id` 解析 Workspace root；循环、缺失 parent、跨 Workspace handoff、
running、archived 和已经 unread 的记录不会成为候选。`idle` 与 `closed` 视为 finished，在 read root 中按
`updatedAt` / `lastActivityAt` 的有效最新时间选择一项，并用单调 RFC 3339 毫秒时间写入
`attentionReason:"finished"`。

recovery inspect 复制 Paseo 的六个稳定 unavailable reason。现存 Workspace cwd 对应 `unarchive`；删除的
managed worktree 只有在 branch 和 source repository 都可用时对应 `restore`。本地 adapter 使用保存的
main repository、worktree root、branch、base ref 和 cwd 相对路径调用真实 Git：恢复必须占用原路径和原
分支，branch 已在别处 checkout 时明确失败，不生成带后缀的新 branch；嵌套 Workspace cwd 必须在新
worktree 中真实存在。Git 或目录验证失败时执行 best-effort worktree rollback，且 registry 仍保持归档。

恢复成功后取消 Workspace 归档；仅当 owning Project 也归档时才更新 Project，避免无意义重写 active
Project。成功 response 写出后发布一个 `workspace.update` upsert event。

## 与 Paseo 的对齐和差异

1. request/result 字段、单值或数组 Workspace ID、inline batch error、root Agent 选择、finished 状态、六个
   recovery unavailable reason 和 `unarchive`/`restore` 判定均取自固定 Paseo 实现。新 server 对空
   `archivedAt` 按未归档处理，与 Paseo 的 falsey 判断一致。
2. 当前 Agent runtime 只有 durable stored snapshot，没有 live Provider session 或 pending-permission
   集合。clear-attention 以 `attentionReason:"permission"` 作为 fail-safe 排除条件；如果外部旧数据遗漏
   reason，新 server 无法像 Paseo 一样从 Provider pending request 再确认。
3. attention 修改尚不发布 Agent/Workspace subscription event。Paseo 会通知 Agent directory 和 Workspace
   observers；新 server 必须等独立 Event runtime 建立后补齐。恢复已有通用 `workspace.update`，但没有
   Agent refresh event。
4. 当前没有 Provider catalog 的可用性投影，attention 候选会考虑全部可放置的 public stored Agent；Paseo
   还会结合当前 Provider 可用性过滤目录记录。
5. Paseo 恢复会经过完整 provisioning，重新探测 Project kind、project key、checkout facts，并处理 merged
   change-request latch。当前复用 durable placement，只验证恢复所需的 source repository、branch、worktree
   root 与相对 cwd，不改写上述衍生字段。
6. 当前不写 Paseo worktree metadata，也不调用 plugin recovery hook 或刷新 live Agent session。保存的
   `baseBranch` 会传给 adapter 作为 branch 缺失时的 Git 起点，但 metadata/check-status 对齐等待后续切片。
7. 新 server 创建的 Worktree record 总是保存 `worktreeRoot`。旧记录缺失该字段时，当前以 `workspace.cwd`
   作为原恢复路径；Paseo 还会执行 managed-root ownership discovery，因此 legacy record 的路径推导可能不同。
8. Project 与 Workspace 分别写入两个原子 registry 文档，没有跨文件事务。如果 Project 取消归档成功而
   Workspace 写入失败，会留下可诊断的部分状态；当前 registry port 无法提供 Paseo provisioning callback
   周围更完整的协调边界。
9. 恢复精确分支与路径、拒绝已被 checkout 的 branch、嵌套 cwd 验证和失败 rollback 使用真实 Git 测试，
   不是内存模拟。rollback 是 best-effort；操作系统或 Git 再次失败时，磁盘上可能保留可人工处理的 worktree。

## 测试执行

从 Paseo Workspace session 与 recovery service tests 对应移植：canonical capability、单值/批量 ID、
camelCase/unknown-field 解析、全部 recovery discriminator、逐 Workspace 部分成功、permission attention 保留、
最新 finished root 选择、无候选拒绝、inspect 全状态、restore registry 协调、真实 Git 精确分支/嵌套目录恢复、
已 checkout branch 拒绝，以及真实 binary WebSocket 请求、event 和重启后持久化检查。

本阶段新增 15 个测试：protocol 4、application 5、API 3、真实 Git adapter 2、真实 binary WebSocket 1。
最终验证：

```text
cargo llvm-cov -p server-bin -p server-api -p server-application -p server-domain \
  -p server-ports -p server-protocol -p server-storage -p server-workspace \
  --json --summary-only --no-fail-fast -j1
  223 passed, 0 failed, 0 ignored
cargo test --workspace --no-fail-fast -j1
  713 passed, 0 failed, 5 ignored
cargo clippy --workspace --all-targets -- -D warnings
  passed
cargo fmt --all -- --check
  passed
```

coverage 运行中的 12 个新 server test target 共 223 个测试通过。生产 Rust 源码为 9,497/10,886 行，
行覆盖率 87.24%；本阶段两个独立实现模块为 452/575 行，行覆盖率 78.61%。共享 Worktree adapter 模块为
599/743 行，行覆盖率 80.62%；protocol DTO 与 recovery port 没有 LLVM 可执行行。完整 crate 级数据、命令
和基线记录在 [phase 7 coverage artifact](paseo-websocket-surface-phase-7-coverage.json)。

第一次非 coverage server package 运行中，既有
`identity::tests::epochs_survive_release_and_never_regress_to_a_copied_database` 偶发返回 `Busy`；立即单测和整个
`server-workspace` target 重跑均通过，之后独立 target 的 coverage 与全 workspace 运行也都通过。该测试与
本阶段 Workspace state 路径无调用关系，报告保留此现象以便后续跟踪文件租约测试的环境波动。
