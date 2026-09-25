# Agent Timeline、Provider 发现与创建订阅

实现 [ADR-039](../decisions/adr-039-agent-timeline-provider-creation.md)，继续只扩展独立 server。
对照固定 Paseo `2c8e8a826810337492cc5a38bb0bbd705b6fb632` 的协议与服务实现，新增
12 个规范方法；参考本机 Codex 0.153.4 导出的 app-server schema 接通原生发现和历史读取。

## 接口与归属

| 归属 | 新接通方法 | 行为 |
| --- | --- | --- |
| server-provider | `agent.timeline.get.request` | tail/before/after 分页、持久化 epoch/sequence、过期游标重置 |
| server-provider | `agent.timeline.search.request` | user/assistant 文本搜索、序号游标、空白及大小写归一化 |
| server-provider | `agent.timeline.list_prompts.request` | user prompt 索引及 Unicode 预览 |
| server-provider | `agent.timeline.append.request` | 插件展示条目、来源标记、重复内容去重与冲突拒绝 |
| server-provider | `agent.timeline.set_subscription.request` | 多 Agent 条目事件、连接独立 release ID、断线释放 |
| server-provider | `provider.available.list.request` | 已注册原生适配器的可用状态 |
| server-provider | `provider.models.list.request` | 真实 `model/list` 分页发现、去重、循环游标防护 |
| server-provider | `provider.modes.list.request` | 适配器实际支持的模式，目前只有 read-only |
| server-provider | `provider.features.list.request` | 适配器实际支持的 feature，目前为空 |
| server-provider | `provider.snapshot.get.request` | 按 cwd 缓存、稳定内容 hash、ifNoneMatch |
| server-provider | `provider.snapshot.refresh.request` | 显式刷新、发布 `providers_snapshot_update` |
| server-metadata | `creation.subscribe.request` | Agent/Workspace 持久化创建快照和进度观察 |

已有 `agent.create.request` 增加 `idempotencyKey`、`subscribe` 和 `initialPrompt`；
已有目录来源 `workspace.create.request` 增加幂等回执和订阅。Session events 接通 Provider
快照事件生产者。公共 `server-model::events` 负责有界传输观察者，API 只负责路由、预算、
鉴权与释放；server-protocol 继续只依赖 server-model。

生产 host 已实现方法由 122 增至 **134/195**。扣除 7 个自定义方法，Paseo 规范接口由
115/188 增至 **127/188（67.55%）**，增加 6.38 个百分点，仍有 **61** 个占位方法。
Agent 为 **22/32**，Provider 为 **6/9**，Session 为 **5/5**。这些数字表示方法已接入，
不代表全部参数、所有 Provider 或行为细节与 Paseo 等价。

## 持久化与执行

- `agents/timeline.sqlite3` 保存 append-only 展示投影及游标。原生 `thread/read` 不恢复 writer；
  执行期间的 `item/completed` 持久化后才发布。重读同一原生 item 不分配新序号，内容冲突拒绝。
- SQLite 文件及 WAL/SHM 拒绝符号链接，application ID/schema version 防止误用其他 SQLite 文件。
  整批追加原子提交；单条超限或事务失败不发布部分结果。
- `creations/receipts.json` 保存创建意图、预留资源 ID 和进度。相同 key/intent 重放回执，
  改动参数报幂等冲突；未知 key 可先订阅。创建响应入队后才激活本连接的进度事件。
- 重启后的未完成创建标记 `failed/outcomeUnknown` 并保留资源 ID，不自动再次创建可能已经存在
  的原生 session。`completed` 表示资源及初始 prompt 已接纳，不表示 Agent turn 已完成。
- Provider catalog 与执行共用已注册适配器；缓存最多 16 个 cwd，TTL 为 60 秒，发现错误只返回
  安全状态。离线测试通过独立 stdio peer 验证真实协议交互，没有访问真实模型或 API。

## 明确保留的差异

- Timeline 的 projected/canonical 目前都返回逐条 identity 投影，不合并相邻消息或改写工具
  生命周期条目；只推送完整条目，不推送 token delta。搜索不包含 Markdown 渲染后的补充匹配。
  查询当前读取该 Agent 的全部投影后分页，超大历史后续需要数据库范围查询优化。
- 原生历史在本进程首次访问 Agent 时加载；原生 Provider 不可用时，首次查询会失败，尚无离线
  投影降级或外部 native history 变更的持续同步。
- Plugin append 采用 `plugin:<id>` client label，调用者仍必须持有现有完整权限 token。
  label 不是独立认证身份；未实现多租户 Plugin 隔离、安装或宿主体系。
- Provider 只有 Codex 执行适配器；features 为空、模式为 read-only。完整 snapshot entries
  可用，compactSnapshot 尚未提供；refresh 同步完成发现后应答。diagnostic、usage、recent
  sessions 三个方法仍未实现。
- Workspace 创建只覆盖 directory source，组合 Workspace+Agent/worktree 创建仍拒绝。
  Agent 创建重放仍先验证 Workspace 有效性；Workspace 后续归档或路径失效时可以读取创建
  回执，但不能保证再次调用创建得到成功响应。原生创建、registry 与回执间没有跨系统事务。
- Agent 后续仍缺 import、fork_context、rewind、refresh、commands、mode/feature 切换、
  permission.resolve，以及 provider subagents 的列表和 Timeline。

## 验证

新增测试覆盖 SQLite 重启/重复条目/事务失败/超限、游标边界与重置、搜索分页和 Unicode
预览、Provider 缓存/内容 hash/不可用状态、stdio 模型发现和历史读取、创建回执重放/冲突/
中断、订阅暂停/激活/溢出/释放。进程级 WebSocket 回归串联 Provider 发现、初始 prompt、
创建事件、插件追加和 server 重启，并验证非对象创建参数只返回错误、不破坏连接。

格式检查、workspace Clippy `-D warnings` 和 `git diff --check` 通过。完整 workspace 回归为
**917 passed、0 failed、5 ignored**，含 1 项 doctest；新增接口独立 WebSocket 回归为
**1 passed、0 failed**。两次运行分别统计，不相加计算不同测试总数。

```sh
cargo fmt --all --check
git diff --check
CARGO_TARGET_DIR=/tmp/ait-agent-interface-target cargo clippy --workspace --all-targets --offline -j2 -- -D warnings
CARGO_TARGET_DIR=/tmp/ait-agent-interface-target cargo test -p server-bin --test process --offline agent_history -- --test-threads=1
CARGO_TARGET_DIR=/tmp/ait-agent-interface-target cargo test --workspace --offline --no-fail-fast -j2 -- --test-threads=1
```

前期回归发现并修正了 native 完整 user item 被过滤、Workspace 回执 ID 前缀，以及新测试读取
Workspace 字段错误的问题；最终检查还补齐了非对象创建参数拒绝和创建订阅文件读取的后台
调度。旧 WorkspaceAutomation 的 weak-Arc 断言在一次早期并行测试中触发，未修改该实现；
最终完整串行测试通过。

## Test coverage

Workspace 行覆盖率为 **81.99%（42,547 / 51,894）**，相对
[ADR-038 的同范围基线](server-protocol-dependencies-coverage.json)
81.63%（41,040 / 50,277）增加 **0.3605 个百分点**。

| 变更涉及的包 | 已覆盖 / 总行数 | 行覆盖率 |
| --- | ---: | ---: |
| server-model | 291 / 307 | 94.79% |
| server-api | 816 / 841 | 97.03% |
| server-metadata | 5,472 / 6,238 | 87.72% |
| server-provider | 4,116 / 4,492 | 91.63% |
| server-bin | 385 / 401 | 96.01% |

新增 Timeline 存储为 163/165（98.79%），查询为 141/143（98.60%）；创建回执服务为
168/172（97.67%），公共观察者为 97/101（96.04%），Provider 连接接线为 131/141（92.91%）。
覆盖率插桩的完整运行是 **916 passed、0 failed、5 ignored**，不含普通回归中的 1 项 doctest。
随后对同一个 WebSocket 场景补充内联 Agent 订阅断言并拆出测试 helper，增量回归通过，
其结果与完整运行分别记录，不累加成不同测试总数。

测量范围是完整 workspace、默认 features、cargo-llvm-cov 默认源码过滤，无额外排除文件。
平台为 macOS arm64，rustc 1.98.1、cargo-llvm-cov 0.8.4；Linux/Windows 未运行。
5 项忽略测试中 4 项需要真实模型/凭据，1 项需要外部 worker；完整名称和原因见 JSON 工件。

源码基于 `97f13722f2f17ca298074d9bdbd101562ac3a2ba`，包括进入本轮前已存在的 ADR-038 依赖
清理和本次实现。全量测量开始时源码 SHA-256 为
`c1c248c8608987819e889972197c96c165973379dd9df3fd1636cad6f0b4cf7c`；补充测试后的交付源码为
`df89bd86d712f0bf9ebe4a590e58cac1fd469e8d6fd42971c64a355f0a9cd42f`。
两者只差 `bins/server/tests/process/agent_history.rs` 的断言和 helper；**290 个被测生产
源码文件哈希全部保持不变**。哈希按路径排序，拼接根 Cargo.toml/Cargo.lock、crates/bins
下的 .rs、.py 和 Cargo.toml 的相对路径、NUL、内容、NUL。

完整测量使用默认 clean 清除旧 profiles，再合并生产源码不变的同场景增量回归。最终 584 个
profiles 均生成于该完整测量源码快照之后；290 个报告源码路径均存在。

```sh
RUST_TEST_THREADS=1 CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-interface-target cargo llvm-cov --workspace --html --offline --no-fail-fast -j2
RUST_TEST_THREADS=1 CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-interface-target cargo llvm-cov --no-clean --workspace --html --offline --no-fail-fast -j2 -- agent_history
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-interface-target cargo llvm-cov report --json --summary-only --output-path /tmp/ait-agent-interface-coverage-raw.json
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-interface-target cargo llvm-cov report --show-missing-lines --offline
```

可审查工件为 [逐文件覆盖率与测量记录](server-agent-timeline-provider-creation-coverage.json)，
包括文件哈希、各包聚合、测试结果、命令、Paseo/原生 schema 校验值和历史基线。
本地 HTML 补充入口为 `/tmp/ait-agent-interface-target/llvm-cov/html/index.html`。

重要未覆盖分支主要是连接订阅预算耗尽、错误 key 与未安装执行服务、原生发现异常分页游标/
thread ID、Provider 缓存淘汰、创建回执 revision 冲突，以及部分持久化重试和传输激活失败。
后续优先补连接预算与错误响应顺序、原生多页发现故障注入；功能差异另见上文，不能由行覆盖率
推导为已支持。
