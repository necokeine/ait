# 独立 server M1：Agent 配置切片

- 日期：2026-09-22；分支：`new`。
- 基线：`132124e4d1553fdcab9e599951b7e3fdc8a3af14`，本批是其后的未提交修改。
- 决策：[ADR-024](../decisions/adr-024-server-agent-configuration.md)；接口见
  [操作说明](../operations/independent-server.md#agent-配置m1-第二个切片)。

## 交付范围

新增 Agent 配置的创建、CAS 改版、精确 revision 读取、当前配置分页，以及显式全局默认的
读取、选择和清空。五个新 WS 方法贯通新 domain、ports、application、SQLite、protocol、
API 和 binary。内部依赖仍全部属于 server 系列，旧 Ait Rust 源码未修改，第三方锁定版本未升级。

Agent revision 不可变，SQL trigger 阻止更新与删除。修改 head、保存 revision 和写 receipt
在一个事务内；默认选择也与回执原子提交。重放旧操作返回旧回执，不覆盖新配置或恢复已清空的
默认。没有隐式选择首个 Agent；默认 Agent 禁用前必须显式取消选择。

凭据只允许指定命名空间内的环境变量引用；不接收或解析凭据正文，不读取 provider 登录态。
当前只保存 Codex 配置 schema，没有远程模型验证、Provider 执行或 fake 生产 driver。
Session/Run 固定配置到 Project、工具策略和其他执行字段仍属后续切片。

Catalog v1 升级前通过 SQLite backup API 留存并同步独立备份，再事务添加 Agent 表并升至
v2；原项目登记与操作意图/回执保留，Project 数据库格式不变。升级失败回滚，备份保留。
已有 v2 catalog 不再次备份；全新 catalog 直接建立 v2。

Agent 与 Project 共用有界准入和阻塞任务追踪；响应连接结束不取消已接纳的写入。
binary 为 Agent catalog 保留 data-dir lease，覆盖数据库与可能延迟完成的阻塞工作生命周期。

## 测试执行

- `cargo fmt --all --check`、`git diff --check`：通过。
- `cargo clippy --workspace --all-targets -- -D warnings`：通过。
- 新 server 八个 package 的普通测试：**63 passed、0 failed、0 ignored**，12 个测试目标。
- `cargo test --workspace --no-fail-fast` 的 72 个普通测试目标全部结束：
  **553 passed、1 failed、5 ignored**；完整 workspace 回归未通过。
- 覆盖率回归：553 passed、0 failed、5 ignored、1 filtered，72 个普通测试目标。

失败项是既有 daemon 的
`daemon_is_ready_and_rejects_unsent_native_recovery_without_replay`：
`bins/daemon/tests/codex_http.rs:389` 的 15 秒就绪断言超时，定向重跑仍失败
（0 passed、1 failed、4 filtered）。根因尚未确定；该测试在本批覆盖率回归中通过，
但不能据此代替普通回归结果。

首次普通回归还在既有 shell 的
`cancels_descendants_before_drain_returns_and_cleans_up_after_normal_exit`
失败（`crates/tools/tests/shell_permissions.rs:311`，取消后仍存在 escaped 标记）。
该项随后定向重跑通过（1 passed、3 filtered），并在上述完整普通目标回归中通过。
本批未修改这两个旧组件的 Rust 源码，也未放宽断言。

失败项复核命令：

```sh
cargo test -p ait-tools --test shell_permissions cancels_descendants_before_drain_returns_and_cleans_up_after_normal_exit -- --exact
cargo test --workspace --test codex_http daemon_is_ready_and_rejects_unsent_native_recovery_without_replay -- --exact
```

文档测试**未完成**。普通测试结束后，首个 `ait_agent_adapters` rustdoc 进程长时间无输出，
停止等待时该目标刚返回 1 passed，并开始 `ait_api_http`；整个命令以中断状态 130 结束。
因此原命令累计观测到 554 passed、1 failed、5 ignored、73 个目标，但不代表所有 doc-tests
完成。随后执行 `cargo test --workspace --doc --no-fail-fast`，再次停在首个 rustdoc 目标，
约 3 分钟后停止（状态 130，无新增测试结果）。未将文档测试记为通过；后续需在可正常完成
rustdoc 的环境重跑，并调查旧 daemon 的就绪时间失败。

关键验收：

- 领域字段边界、引用语法、UUID/修订号/时间范围和 revision 耗尽；存储拒绝无效时间快照且不留下配置/回执。
- revision 不可变、精确历史、key 参数冲突、旧回执优先、重开恢复与 keyset 分页。
- 默认初始为空、版本冲突、禁用保护、显式清空及旧回执不重新选择。
- 两个 SQLite 连接同时修改同一 revision，只能提交一个新 head。
- receipt 插入故障时，新建/改版/default 事务回滚；v1 升级中途失败不残留新表或版本。
- v1 已完成项目登记和未完成意图的保留；备份版本、回执和 integrity_check。
- 真实 binary 的 WS 参数与 capability、重连、SIGTERM/restart、历史回执和配置恢复。
- 子进程使用测试凭据值，确认数据库、日志和错误响应没有该值；所有数据与 HOME 使用临时目录。
- 项目事务被暂停时，Agent 请求同样受共享准入预算限制；已接纳事务纳入关闭排空。

## Test coverage

测量命令：

```sh
cargo llvm-cov --workspace --html -- --skip command_approval_secrets_never_reach_durable_or_reconnected_views
cargo llvm-cov report --json --summary-only --output-path /tmp/ait-server-agents-coverage-summary.json
```

| 范围 | Covered / total lines | 行覆盖率 | 相对 M1 项目打开 |
| --- | ---: | ---: | ---: |
| Workspace | 25,646 / 32,487 | 78.94% | +0.38 个百分点 |
| server-bin | 282 / 294 | 95.92% | +0.50 个百分点 |
| server-api | 673 / 693 | 97.11% | -0.06 个百分点 |
| server-application | 178 / 179 | 99.44% | +0.30 个百分点 |
| server-domain | 187 / 187 | 100.00% | +0.00 个百分点 |
| server-ports | 16 / 16 | 100.00% | +0.00 个百分点 |
| server-protocol | 92 / 99 | 92.93% | +3.77 个百分点 |
| server-storage | 598 / 656 | 91.16% | +0.93 个百分点 |
| server-workspace | 220 / 232 | 94.83% | +0.00 个百分点 |

八个 server package 合计 **2,246 / 2,356，95.33%**；domain 与 ports 均为 100%。
Workspace 为 **78.94%**，仍低于工程规范 80% 目标，本批不通过修改旧组件来补齐全局数字。

基线为 [M1 项目打开 artifact](independent-server-m1-coverage.json) 的 24,980 / 31,795（78.57%），
本批增加 **0.38 个百分点**。其全部 52 个代码文件 SHA-256 与起点 HEAD 相符；工具链、平台、
默认 features 和 coverage-only 排除项相同。未手动排除生产文件。

- 版本：`132124e` 加本批未提交变更；逐文件 SHA-256、总指纹与完整 revision 见 artifact。
- 平台：macOS 26.6.2 / Darwin 25.6.0 arm64；rustc 1.98.1，cargo-llvm-cov 0.8.4。
- 范围：workspace 默认 features，无 `--features` / `--all-features`，175 个生产源文件；
  测试源/build scripts 按 llvm-cov 默认规则过滤，doc-tests 未插桩。
- 唯一显式排除沿用前两批：`command_approval_secrets_never_reach_durable_or_reconnected_views`，
  该旧测试此前在插桩下反复超过 3 秒审批等待，普通 workspace 回归不排除它。
- 5 个既有 ignored：`codex_native_tools_create_and_verify_python_hello_world`、
  `deepseek_live_default_catalog`、`wf11_real_deepseek_python_hello_world`、
  `wf10_create_project_with_real_codex_and_commit`（真实模型环境），以及
  `permission_change::replay_permission_change_with_external_worker`（独立 worker）。
- 可评审 artifact：[Agent 配置覆盖率 JSON](independent-server-m1-agents-coverage.json)，包含行计数、
  crate 汇总、范围/命令、代码指纹和测试结果；本地 HTML 为 `target/llvm-cov/html/index.html`。

主要未覆盖分支为文件/SQLite I/O 错误、锁竞争错误转换，以及迁移备份与 fsync 失败。

未验证范围包括 Linux/Windows、真实 Provider 与环境凭据解析、Project 内配置冻结和
Session/Run 执行。SQLite 故障用 trigger 模拟事务失败；未执行真实断电、磁盘耗尽或备份 fsync
失败实验。关闭超过排空预算后的不可取消 I/O 限制沿用上一批；行覆盖率不等于分支和平台覆盖率。
