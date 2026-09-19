# ADR-017 实现与验证

实现基于 `d5b6a9d91158b4de7f282e455277fc0642cb8ae8` 的工作区修改，设计见
[ADR-017](../decisions/adr-017-unified-native-codex-worker.md)。

## 实现结果

生产 Codex 调用统一经过 `daemon → WorkerSupervisor → ait-worker → app-server`。
新建与导入 Session 都使用持久原生 Thread；创建、恢复、执行、完整历史读取、列举、模型发现和
标题生成均由 worker 完成。辅助调用使用独立 scope，wire 协议升级为 2.0。

Run 持久化 input intent 后才能发送输入；结果不明时只对账，不重发。新 Thread 首次输入前尚未
materialize 的情形单独处理。原生 writer 准备不占用关闭屏障，持久化准入在屏障内再次检查
draining；关闭后也禁止新增 Git 重试。

删除旧 `CodexWorkspaceAgent`、Codex prompt/tool 包装、每 Run 临时 worktree、路径重写和
合并/回滚执行协议。新 Session 仍保留固定 Session worktree；导入 Thread 保留其原生 cwd。
失败或取消后的文件修改保留。旧 SQLite 执行 journal 仅用于完成切换前已经决定的存储事务，
随后清理，不再驱动 Run 执行。

`codex.auto_commit` 默认关闭，设置在 Run 准入时冻结。成功发布权威原生历史后，Ait 独立
准备和发布 Git commit；结果保存在 Run receipt，不修改 Message。起始目录脏、Git 基线变化、
没有差异时跳过；失败/取消/超限的模型执行不提交。Git 失败保留模型完成状态，CLI、HTTP 和
Desktop 可以只重试 Git。持久化的精确 commit ID、Git ref transaction 和有所有权证明的
index receipt 支持中断/ACK 丢失后的对账，外部 Git lock 不会被删除。

首次打开旧存储时一次性清理旧 Ait 会话记录。离线 Project 延迟到下次验证身份后清理；保留
Project 根消息、配置、命名 Agent、Provider，以及不依赖旧会话的 Cron。不删除 `.ait` 目录、
Session 文件或原生 Codex rollout。本次开发仅在临时测试数据库验证切换，未运行用户数据清理。

## 回归重点

- Worker 真实子进程：接纳前零输入、创建/恢复保持原生 cwd、关闭回收、辅助请求进程回收、
  原生 item 超限后 interrupt，以及协议版本/分块/陈旧 scope 检查。
- 原生执行：仅发送当前输入、首次拒绝不生成虚假 Message、未知结果不重放、后续历史同步保留
  Ait 的资源超限结论、关闭期间延迟 writer 不能接纳输入。
- Git 收尾：设置快照、脏目录仍能执行、无变化/失败/取消不提交、精确 commit 重试不重复调用
  Codex、不改写 Message、启动恢复、HEAD/分支/index 变化、外部锁和自有锁恢复。
- 存储切换：清理一次、保留工作文件和根配置、离线 Project 补清理、身份不匹配拒绝操作。
- CLI/HTTP/daemon/Desktop：统一 native fixture、异步提交/进度/审批、Git receipt 展示与重试入口。

## Test coverage

普通测试结果：`cargo test --workspace --no-fail-fast` 为 451 通过、0 失败、5 忽略；
Desktop `npm test` 为 137 通过、0 失败，`npm run typecheck` 通过。
`cargo fmt --all --check`、`cargo build --workspace`、
`cargo clippy --workspace --all-targets -- -D warnings` 和 `git diff --check` 均通过。

早期并行编译/测试时出现过 worker 握手和 daemon readiness 超时；独立复跑及最终顺序执行的
完整普通测试、覆盖率测试均通过。没有放宽生产启动时限，也不据此断言超时根因已经确定。

覆盖率运行使用 `cargo llvm-cov --workspace --html`，450 通过、0 失败、5 忽略；与普通运行
相差的 1 项为 doctest。通过测试数量与行覆盖率分别统计。

| 范围 | 覆盖行 / 总行 | 行覆盖率 | 相对 ADR-016 参考结果 |
| --- | ---: | ---: | ---: |
| Cargo workspace | 21,389 / 27,673 | 77.29% | -1.50 个百分点 |
| application | 9,529 / 11,489 | 82.94% | -0.68 个百分点 |
| agent-adapters | 2,006 / 3,342 | 60.02% | -12.83 个百分点 |
| ipc | 803 / 1,278 | 62.83% | 无同表基线 |
| worker | 72 / 534 | 13.48% | 无同表基线 |
| workspace-local | 1,212 / 1,512 | 80.16% | 无同表基线 |
| storage-sqlite | 1,515 / 1,677 | 90.34% | 无同表基线 |
| ports | 124 / 202 | 61.39% | +5.59 个百分点 |

测量范围为 macOS arm64、默认 features、完整 Cargo workspace，Rust 1.98.1、
cargo-llvm-cov 0.8.4。没有额外排除源码；工具结果不包含测试源文件和 doctest，其他平台的
`cfg` 分支未测。4 项需要真实凭据/计费的模型测试及 1 项需要指定外部 worker 的测试保持忽略，
具体名称列在摘要中。

测量版本为本报告开头所列基线上的工作区源码，源码 SHA-256 为
`e0f121efe7f6f38336dbd7d3adf2d0aae2e519c1ee19c3afc8625d7b83e503e8`，
计算范围和算法见 [本次覆盖率摘要 JSON](adr-017-coverage-summary.json)。该文件包含所有 crate、
修改文件的行数和测试统计，可随代码审查。HTML 明细保存在 `target/llvm-cov/html/index.html`。
导出命令为 `cargo llvm-cov report --json --summary-only --output-path /tmp/ait-unified-coverage.json`。

比较引用 [ADR-016 摘要](adr-016-coverage-summary.json) 中相同平台/default features 的历史测量，
并非重新测量 `d5b6a9d` 的干净检出；代码和行数总体已改变，差值仅作为参考。

当前仍低于 80% 的工作区目标。新原生准入 `runs/native.rs` 为 402/443（90.74%），独立 Git
收尾为 171/207（82.61%），IPC native 边界为 126/145（86.90%），本地 Git 发布为
196/222（88.29%），存储切换为 81/87（93.10%）。尚未覆盖的重点包括部分存储/CAS 失败路径、
Git 对象与收据损坏分支、原生进程 I/O 错误组合。

另外，worker 的正常 Unix 收尾也会通过 `cleanup_worker_group` SIGKILL 自身进程组；
强制结束的进程不会执行退出时的 profile 刷写，环境白名单也不转发 `LLVM_PROFILE_FILE`。
因此真实 worker 集成用例通过，但 `bins/worker/src/codex.rs` 的测量仍为 0/224。所有 Codex
操作移入该进程后，adapter 的测量也受到此边界影响。没有调整百分比或排除这些文件；后续
需要完善子进程覆盖率采集，并对上述错误分支增加有针对性的故障注入。不能把集成测试通过
等同于这些行已被覆盖率工具记录。

## 验证边界

本轮未执行需要登录/计费的真实 Codex、DeepSeek 用例；app-server API 形状以本机
Codex 0.153.4 schema 和不发送 Turn 的隔离探测核对，模型执行使用协议 fixture。
Desktop 做类型检查和单元测试，未做真实 Electron GUI 人工验收。其他操作系统未实测。

自动提交覆盖 cwd 在收尾时的文件差异。干净基线与 Git 身份校验能避免覆盖已知的外部
HEAD/分支/index 变更，但无法归因 Run 执行期间的并发未暂存文件编辑；这些改动可能一同提交。
Ait lease 不会锁住用户编辑器或其他原生客户端。该功能默认关闭。

原生 fork/steer、历史节点派生、繁忙 Session 自动分支、Codex Cron、绑定原生 Thread 的便携
Ait archive 导入/导出仍返回明确能力错误；这些入口没有旧执行链路回退。从 Project 根创建
独立任务及复用当前空闲 Session 可以执行。绑定原生 Thread 后不能切换 Provider。
