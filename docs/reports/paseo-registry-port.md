# Paseo Project / Workspace 模型与 registry 移植

- 日期：2026-09-22；分支：`new`，未提交。
- 基线：`a4502b9`；来源：Paseo `2c8e8a826810337492cc5a38bb0bbd705b6fb632`。
- 决策：[ADR-025](../decisions/adr-025-paseo-registry.md)。

## 交付与边界

逐字段移植 Project 的 10 个持久化字段、Workspace 的 18 个字段，以及 protocol 的项目/
工作区 descriptor、placement、checkout 联合类型、脚本和 Git/forge runtime 嵌套结构。
保留字符串身份、时间、camelCase、legacy 枚举、缺失/null/default 差异。

`server-protocol::project` 现在导出 Paseo 投影，原租约 DTO 单独放入 `project_lease`，
类型改名 `ProjectLeaseSnapshot`。Paseo 原记录位于 `server-domain::registry`；存储记录、
协议投影和执行租约不再共用一个含糊的 Project 定义。

`server-ports::registry` 定义源 registry 的阻塞接口，`server-storage::registry` 实现
JSON 数组持久化、活动根分配、CRUD、归档、通知和冻结。实例 clones 共享串行写入，文件替换
失败不更新 cache、不通知；列表保持插入顺序，重开保留记录。

本批是模型与 registry 基础移植，**尚未把它切换到生产 binary 的 handler/组装**。
现有 WS 租约接口继续运行；未自动迁移 SQLite、创建生产 JSON registry 或读取用户的 Paseo 数据。
下一批应移植目录发现、placement reconciliation、descriptor 聚合和对应 Paseo request/response，
再切换组装并验证重启与迁移。Session 固定 worktree 计划暂停。

明确差异包括：损坏 registry 文件返回错误而非按空集合继续；非 ISO 日期与同时间 ID 排序
不完整模拟 JavaScript 的宽松 Date.parse/localeCompare；observer 为锁外同步回调。
跨文件 labels journal、跨进程写入协调、真正断电和目录 fsync 不在本批验收范围。
许可、固定源码位置与修改说明保存在 `third-party/paseo/`。旧 Ait Rust 源码未修改，
内部依赖仍限于新 server 系列；Cargo.lock 只新增对既有依赖版本的引用。

## 测试执行

原始 Zod 对照 fixture 共 **369 项**：record 108 项，wire 261 项。记录源 commit、源文件
SHA-256 和 Zod 4.4.3；生成器直接执行该固定源码的 schema。Rust 测试验证接受/拒绝及规范化
JSON 输出，正常测试不依赖 Node、Paseo 或网络。

复现 fixture：

```sh
npm install --prefix /tmp/ait-paseo-oracle --ignore-scripts --no-audit --no-fund zod@4.4.3
node scripts/paseo-registry-fixtures.mjs /path/to/paseo /tmp/ait-paseo-oracle/node_modules/zod
```

已完成定向检查：`cargo test -p server-domain -p server-protocol -p server-storage --lib`，
**31 passed、0 failed、0 ignored**。包括全部 Zod fixture、名称覆盖、20 个并发分配、并发
更新/归档、旧 ID/重复记录/碰撞、路径比较、重开、观察者失败、退订、冻结，以及写入失败后
磁盘/cache/通知一致性和损坏文件保留。

`cargo fmt --all --check`、`git diff --check` 和
`cargo clippy --workspace --all-targets -- -D warnings` 已通过。
覆盖率回归：**565 passed、0 failed、5 ignored、1 filtered**，72 个普通测试目标。
`cargo test --workspace --no-fail-fast` 的 72 个普通测试目标全部通过：
**566 passed、0 failed、5 ignored、0 filtered**。其中新 server 为
**75 passed、0 failed、0 ignored**，12 个测试目标。另有 24 个文档测试目标完成，
其中 1 项 doc-test 通过。完整命令退出码为 0，累计 **567 passed、0 failed、5 ignored**。

## Test coverage

测量命令：

```sh
cargo llvm-cov --workspace --html -- --skip command_approval_secrets_never_reach_durable_or_reconnected_views
cargo llvm-cov report --json --summary-only --output-path /tmp/ait-paseo-registry-coverage-summary.json
```

| 范围 | Covered / total lines | 行覆盖率 | 相对前一批 Agent 配置 |
| --- | ---: | ---: | ---: |
| Workspace | 26,237 / 33,111 | 79.24% | +0.30 个百分点 |
| server-bin | 282 / 294 | 95.92% | +0.00 个百分点 |
| server-api | 674 / 694 | 97.12% | +0.00 个百分点 |
| server-application | 178 / 179 | 99.44% | +0.00 个百分点 |
| server-domain | 213 / 213 | 100.00% | +0.00 个百分点 |
| server-ports | 16 / 16 | 100.00% | +0.00 个百分点 |
| server-protocol | 190 / 197 | 96.45% | +3.52 个百分点 |
| server-storage | 1,068 / 1,155 | 92.47% | +1.31 个百分点 |
| server-workspace | 220 / 232 | 94.83% | +0.00 个百分点 |

八个新 server package 合计 **2,841 / 2,980，95.34%**。workspace 为 **79.24%**，
仍低于工程规范的 80% 目标；本批没有为了覆盖率数字改动旧组件。

- 版本：`a4502b9` 加本批未提交修改；测量记录 86 个源文件/manifest/fixture/生成器文件的 SHA-256。
  之后仅三文件缩短注释或调整 fixture 空白以满足行长规范，已核对非注释 Rust token 一致，
  生产代码行位置未变；重新检查 fmt/clippy。测量与最终交付指纹分别保存在 artifact。
- 可比基线：[Agent 配置报告 artifact](independent-server-m1-agents-coverage.json)，
  workspace 25,646 / 32,487（78.94%）；其全部 66 个代码文件 hash 与本批起点 HEAD 相符。
- 平台：macOS 26.6.2 / Darwin 25.6.0 arm64；rustc 1.98.1、cargo-llvm-cov 0.8.4。
- 范围：workspace 默认 features，185 个生产源文件；无手动生产文件排除；test/build 源按
  llvm-cov 默认规则过滤，doc-tests 默认不插桩。
- 唯一显式排除沿用前几批：`command_approval_secrets_never_reach_durable_or_reconnected_views`。
  该旧测试的 3 秒审批等待此前在插桩环境反复超时；普通回归仍包含它。
- 五个既有 ignored：`codex_native_tools_create_and_verify_python_hello_world`、
  `deepseek_live_default_catalog`、`wf11_real_deepseek_python_hello_world`、
  `wf10_create_project_with_real_codex_and_commit`，以及
  `permission_change::replay_permission_change_with_external_worker`。
- 可评审 artifact：[覆盖率 JSON](paseo-registry-coverage.json)，包含逐文件行计数、crate 汇总、
  测试结果、源代码与 fixture 指纹、源 Zod 版本和范围；本地 HTML：`target/llvm-cov/html/index.html`。

未覆盖重点包括随机源失败、部分文件 I/O/poisoned lock 错误、部分路径语法分支，以及既有
SQLite/关闭异常路径。未测真实断电、目录 fsync、跨进程多 writer、Linux/Windows 系统调用。
Windows lexical path 已在 macOS 上进行纯字符串测试，不等于 Windows 平台测试。
生产 binary 的 registry handler、目录发现与 descriptor 聚合尚未接入，不包含在当前覆盖率
通过声明中；后续接入必须补真实 WS/重启/迁移验证。

