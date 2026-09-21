# 独立 server M1：项目打开切片

- 日期：2026-09-22；分支：`new`。
- 基线：`0b0b06f`，在已提交的 M0 上继续开发；本报告对应其后的未提交变更。
- 决策：[ADR-023](../decisions/adr-023-server-project-opening.md)；运行方式见 [使用说明](../operations/independent-server.md)。

## 交付范围

新增 `server-domain`、`server-ports`、`server-application`、`server-storage`、`server-workspace`，
并把项目用例接入现有新 binary/API/protocol。全部内部依赖属于本次独立 server 系列；没有
修改或复用旧 Ait 实现、旧数据库或旧测试 fixture，第三方锁定版本未升级。

服务端现在能够通过 WS 打开一个既有独立 Git 根目录，持久化项目身份、初始 HEAD 和根
system Message，分页读取 catalog，校验所有权并关闭项目。根指令只捕获项目根的 AGENTS.md，
不回写历史。项目与 catalog 使用独立 schema family、事务和持久回执；数据库 trigger 拒绝
修改/删除已保存的 Project 创建事实和 Message。

跨 data-dir 的独占依赖规范路径锁和 HOME 下共用的 Project ID 锁；复制数据库保留 ID，
仍不能同时持有。ID 锁旁另以原子文件保存代次上限，切换旧数据库副本时也不能重用旧 epoch。首次打开未完成的 catalog 意图可以显式重试，已经完成的 open/close
则只读返回原 operation 回执。旧 close 不会影响后来的新 owner。

阻塞 Git/SQLite 工作由 API 受监督执行，最多同时接纳一个短项目操作。响应连接消失后，
任务继续持有追踪 token 和进程锁；关闭流程等待它完成。项目资源按数据库 → ID 锁 → 路径锁
顺序释放，data-dir 锁覆盖整个 application 生命周期，包括超时后尚未完成的阻塞工作。

本批只完成 M1 的“项目打开”切片。Agent/revision、Session/worktree、输入/Run、
fake 结果发布及历史/事件回放仍按实施计划推进；没有声明 Provider 或 worker 能力。

## 测试执行

- `cargo fmt --all --check`、`git diff --check`：通过。
- `cargo clippy --workspace --all-targets -- -D warnings`：通过。
- 八个 server package 的 51 项普通测试通过，无失败或 ignored。
- `cargo test --workspace`：通过，**543 passed、0 failed、5 ignored、0 filtered**；
  包括 72 个普通测试目标的 542 项测试及 24 个文档测试目标的 1 项测试。
- 依赖守卫通过；没有修改旧 Ait Rust 源码，新增 domain/protocol 的纯度检查也禁止 rusqlite。

关键验收覆盖：

- 原子初始化、重开保持身份/根 Message/初始 HEAD、不可变 trigger、owner 递增和旧 owner 拒绝。
- 路径别名、旧标记、无 HEAD、子目录、linked/shared worktree、tracked runtime、忽略规则覆盖、symlink 拒绝。
- Project commit 后而 catalog receipt 前的恢复；空 catalog 对已有 Project 的重建登记。
- 不同 catalog 的锁竞争、复制同 ID 数据库的竞争、跨旧副本代次不回退、计数器损坏/symlink 拒绝、key 参数冲突、关闭回执优先于过期 epoch。
- 实际 WS 握手/能力、参数错误、分页、断线重试，以及事务暂停时的有界准入和关闭等待。
- 实际 `server` 子进程的跨 data-dir 竞争、SIGTERM、重启和再次接管；所有数据与 HOME 均使用临时目录。

## Test coverage

测量命令：

```sh
cargo llvm-cov --workspace --html -- --skip command_approval_secrets_never_reach_durable_or_reconnected_views
cargo llvm-cov report --json --summary-only --output-path /tmp/ait-server-m1-coverage-summary.json
```

| 范围 | Covered / total lines | 行覆盖率 |
| --- | ---: | ---: |
| Workspace | 24,980 / 31,795 | 78.57% |
| server-bin | 250 / 262 | 95.42% |
| server-api | 515 / 530 | 97.17% |
| server-application | 116 / 117 | 99.15% |
| server-domain | 79 / 79 | 100.00% |
| server-ports | 13 / 13 | 100.00% |
| server-protocol | 74 / 83 | 89.16% |
| server-storage | 314 / 348 | 90.23% |
| server-workspace | 220 / 232 | 94.83% |

八个 server package 合计 **1,581 / 1,664，95.01%**。Workspace 仍低于规范的 80% 目标；
本批没有为提高全局数字而修改旧组件。

与 [M0 artifact](independent-server-m0-coverage.json) 的同口径比较：workspace 从
24,059 / 30,803（78.11%）到 24,980 / 31,795（78.57%），增加 **0.46 个百分点**。
M0 artifact 的全部 25 个代码校验值与本次起点 HEAD 一致；工具链、默认 features、平台和
coverage-only 排除项相同。新增五个 crate 没有改动前的 crate 覆盖率基线。

- 版本：`0b0b06f` 加本批未提交变更；完整 revision、逐文件 SHA-256 与总指纹见 artifact。
- 平台：macOS 26.6.2 / Darwin 25.6.0 arm64；rustc 1.98.1，cargo-llvm-cov 0.8.4。
- 范围：workspace 默认 features，无 `--features` / `--all-features`；166 个生产源文件，
  无手动排除生产文件。测试源与 build scripts 按 llvm-cov 默认规则过滤；doc-tests 未插桩。
- 覆盖率测试：**541 passed、0 failed、5 ignored、1 filtered**，72 个普通测试目标。
- 唯一显式排除沿用 M0：`command_approval_secrets_never_reach_durable_or_reconnected_views`。
  该既有测试曾在插桩下反复超过 3 秒审批等待；普通 workspace 回归不排除它。
- 原有 ignored：`codex_native_tools_create_and_verify_python_hello_world`、
  `deepseek_live_default_catalog`、`wf11_real_deepseek_python_hello_world`、
  `wf10_create_project_with_real_codex_and_commit`（真实模型环境），以及
  `permission_change::replay_permission_change_with_external_worker`（显式独立 worker）。
- 可评审 artifact：[M1 覆盖率 JSON](independent-server-m1-coverage.json)，包含所有文件的行计数、
  各 crate 汇总、命令/范围与源文件指纹。本地 HTML 在 `target/llvm-cov/html/index.html`。

主要未覆盖/未验证范围：部分错误码的具体输出文案、数据库/文件 I/O 失败分支，Git 子进程
真正卡死或超出输出预算，以及磁盘耗尽、原子替换/fsync 失败和真实断电。Project/catalog
中间状态用分步提交再重开的 fixture 验证，不能等同于断电实验。15 秒是关闭排空预算；
不可取消的阻塞 I/O 可能使实际进程退出更晚，相关硬超时路径仍需故障注入。
Linux/Windows、真实 Provider 和执行进程树不在本次测量范围；行覆盖率不代表分支或平台覆盖。
