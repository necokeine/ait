# 可靠性、安全、可观测性与备份恢复手册

## 可观测性

daemon 对每个实体操作 API 调用输出一行 JSON。日志与 `/v1/metric/list` 的每个采样点使用
相同关联字段：`project_id`、`session_id`、`run_id`、`call_id`。操作入口一定生成
`call_id`；其余 ID 在请求或结果可确定时补齐。

日志只记录操作名、成功状态和耗时，不记录请求正文。`authorization`、`cookie`、
`password`、`secret`、`token`、`api_key`、`credential` 等键以及 Bearer 字符串会在 JSON
输出前递归替换为 `[REDACTED]`。本地指标入口：

```bash
curl http://127.0.0.1:7314/v1/metric/list
```

当前指标为进程内单调计数器，重启后清零；权威业务状态仍是 SQLite snapshot 和 durable
event outbox。不要把高基数关联标签转发到收费的远程指标后端，除非先配置采样与聚合。

## Project 导出与导入

导出一个 Project：

```bash
cargo run -p ait-cli -- export --project-id project-1 --output project-1.ait.json
```

导入时必须显式选择本机目标目录；该目录会重新经过规范化与 Git root 校验：

```bash
cargo run -p ait-cli -- import --input project-1.ait.json --workdir /absolute/new/workdir
```

归档保留完整 Message 树/分支、Message ID 和 parent edge，以及 Project、Agent、Session
revision。为保证恢复安全，Session 的 `active_run_id` 会清空；Run、Cron、附件字节和凭证
不会导出。导入在一个 SQLite snapshot 事务中完成，任一 ID 冲突、悬空 parent、环、跨
Project 指针、未知 Agent 或版本不兼容都会整体拒绝。

分享归档前仍需人工检查 Message 和工具结果；“不含凭证”不代表内容不敏感。

## 数据保留与附件清理

默认保留策略：

| 数据 | 默认策略 | 清理条件 |
| --- | --- | --- |
| Project、Session、Message | 永久保留 | 只接受显式用户删除/归档 |
| Run、attempt、tool execution、durable event | 90 天 | Run 已终态，且不破坏审计/重放窗口 |
| 本地 JSON 日志 | 14 天 | 按日期轮转后删除 |
| 本地指标快照 | 7 天 | 非权威数据，可直接过期 |
| 附件 | 被引用时永久保留 | 仅清理无引用内容 |
| SQLite 备份 | 7 个日备份、4 个周备份、12 个月备份 | 新备份校验通过后轮转 |

附件采用 mark-and-sweep：先从所有未删除 Message 的 `FileRef` 和仍在保留期的 Run/
ToolExecution/Checkpoint 标记 attachment digest，再将未标记对象移动到隔离区。隔离 7 天
后重新扫描；仍无引用才删除。禁止直接按文件 mtime 删除内容寻址对象。删除前记录 digest、
大小和最后引用时间，不记录附件内容。当前纵向切片尚未接入附件字节存储，因此上线附件
adapter 前必须把该流程实现为可 dry-run 的维护命令。

## SQLite 备份

不要直接复制处于 WAL 模式的 `.sqlite3` 文件。在线备份使用
`SqliteControlStore::backup_to`（SQLite Online Backup API）；备份包含 control snapshot
和 durable event outbox，不包含外置 provider secret。每次备份后：

1. 以只读/隔离连接打开备份。
2. 执行 `PRAGMA quick_check;`，结果必须是 `ok`。
3. 记录备份时间、数据库 revision、文件大小和 SHA-256；不要记录业务正文。
4. 只有新备份校验通过后才按保留策略轮转旧备份。

也可用 SQLite CLI 执行同一在线备份语义：

```bash
sqlite3 ait.sqlite3 ".backup 'backups/ait-2026-09-04.sqlite3'"
sqlite3 backups/ait-2026-09-04.sqlite3 "PRAGMA quick_check;"
```

## 恢复演练

1. 停止 daemon 并记录当前数据库、`-wal`、`-shm` 文件位置。
2. 保留故障现场副本，不在原文件上试修。
3. 对备份执行 `PRAGMA quick_check;`。
4. 恢复到一个新数据库路径；库内调用可使用 `SqliteControlStore::restore_from`。
5. 用 `snapshot` 验证 revision、Project 数量、Session head 和 Message 路径；用
   `events --after <已知 cursor>` 验证 outbox 连续性。
6. 仅在验证通过后将 daemon 指向恢复库。恢复后的新写入从恢复 revision 继续；备份之后
   已确认成功的写入不会自动重放，需依据审计记录人工确认。

至少每季度做一次隔离恢复演练。单元验收
`online_backup_restores_a_consistent_revision_and_outbox` 会验证 revision 与 outbox 的一致恢复。

## 可靠性测试矩阵

| 场景 | 集成测试 |
| --- | --- |
| 断电/进程中断恢复 | `crash_recovery_persists_a_known_tool_outcome_without_reexecution` |
| 重复调度 | `claimed_saga_recovers_existing_run_without_duplicate` |
| Provider 限流 | `openai_compatible_adapter_classifies_rate_limits` |
| 工具超时 | `a_hung_tool_is_cancelled_at_the_persisted_runtime_deadline` |
| 并发分支 | `concurrent_cas_keeps_the_losing_message_as_a_sibling_branch` |
| SQLite 备份恢复 | `online_backup_restores_a_consistent_revision_and_outbox` |
| Project 导入导出 | `project_export_import_preserves_tree_and_revisions_without_runtime_or_credentials` |

运行完整门禁：

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

## 性能基线

在固定电源模式、相同 Rust toolchain 和无其他重负载的机器上运行：

```bash
cargo bench -p ait-application --bench message_path
cargo bench -p ait-providers --bench context_assembly
cargo bench -p ait-scheduler --bench scheduler_scan
```

三个 Criterion case 分别是 `message_path/10k_depth`、`context_assembly/2k_messages` 和
`scheduler_scan/1k_due_plans`。首次结果保存为机器基线；后续同机中位数回退超过 20% 时阻止
合并并分析 allocation、SQLite query plan 或 Cron parsing。绝对数值只用于容量评估，不跨
不同硬件比较。

2026-09-04 初始基线（Apple M4、aarch64 macOS、rustc 1.96.0、release profile）：

| Case | 中位数 |
| --- | ---: |
| `message_path/10k_depth` | 1.560 ms |
| `context_assembly/2k_messages` | 120.29 µs |
| `scheduler_scan/1k_due_plans` | 1.407 ms |
