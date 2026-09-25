# Agent 原生控制与 Provider 检查

实现 [ADR-041](../decisions/adr-041-agent-controls-provider-inspection.md)，继续补足独立
server-provider，参考固定 Paseo `2c8e8a826810337492cc5a38bb0bbd705b6fb632` 和本机
Codex 0.153.4 app-server schema。

| 方法 | 实现行为 |
| --- | --- |
| `agent.rewind.request` | conversation 排他回退：fork 原生 thread、切换 Agent 指针、保留源历史；失败后的投影恢复 |
| `agent.commands.list.request` | 从原生 cwd 读取启用的 skills，支持草稿配置与 `/skill 参数` 执行 |
| `agent.mode.set.request` | read-only、auto、full-access；原子保存并在下一 turn 应用 |
| `agent.feature.set.request` | fast_mode；校验所选模型 service tier，批量 config.apply 同步支持 |
| `agent.permission.resolve.request` | 命令/文件审批、阻塞问题答案，Agent 范围校验、断线恢复、过期/重复响应拒绝 |
| `agent.provider_subagents.list.request` | 通过原生父关系发现直接及间接后代，不增加宿主 Agent |
| `agent.provider_subagents.timeline.get.request` | 验证后代范围后读取完整原生历史，持久 epoch/cursor 与分页 rows |
| `provider.diagnostic.request` | 可执行文件、协议和登录类型检查，不回传账号邮件/凭证/原生错误正文 |
| `provider.usage.list.request` | 原生账户额度窗口、使用/剩余百分比及重置时间；不可用时明确标记 |

生产 host 已实现 **147/195**，比 ADR-040 增加 9 个方法。扣除 7 个自定义方法，Paseo
规范方法为 **140/188（74.47%）**，增加 4.79 个百分点；剩 **48** 个占位。
Agent **32/32**、Provider **9/9**、Session **5/5**。这里只统计方法接通，不表示所有
Provider、可选参数、消息事件或完整运行行为已与 Paseo 等价。
[剩余接口清单](server-interface-gaps.md)已同步更新。

## 边界与限制

全部业务处理留在 server-provider，metadata 继续负责 placement 和 Session attention；
没有向 server-api 添加业务处理或改变 server-protocol 的依赖方向。原生 history 仍由
Provider 所有；本轮只操作 Agent 的 native handle 和不可变展示投影，领域 Message 树不变。

回退保存新 handle 与恢复标记后才提交 Timeline 新代；SQLite 故障时重启可恢复，resume
和发送不会绕过恢复。旧原生 thread 与退休展示代保留。审批 UUID 与当前原生进程绑定，
客户端断线可以继续审批，进程重启则失效；待审批内容不持久化到展示历史。

当前仍只有 Codex。未提供 plan_mode、files/both rewind、持久 policy amendment、
会话级 grantRoot 授权、非阻塞原生问题或其他原生交互类型。未知交互明确失败。
命令只涵盖启用的原生 skills，不包含 compact/goals/custom prompts；`agent.skills.*`
管理接口仍未实现。子 Agent 查询不提供持续更新订阅，历史只包含已有完整 turn，
idle/notLoaded 的终止原因不能全部区分。usage 不提供余额换算或额外计费 API。

原生进程、Agent JSON 与 Timeline SQLite 不能一起提交；原生 fork 成功但 registry 失败
可能留下未关联 thread，审批发出后 registry 写入失败也无法撤回回答。外部 Codex 进程的
并发修改仍依赖 Provider 协调。详见 ADR-041 的失败边界。

## 验证

定向测试覆盖原生模式/feature 映射、无 fast tier 的原子拒绝、命令输入、额度归一化、
脱敏诊断、原生审批和问题答案、跨 Agent/重复/过期审批、拒绝与完成竞态、客户端重连、
重启清理、子 Agent 嵌套/范围/分页、回退源历史保留、首 turn 回退及分步写入恢复。
所有原生操作使用可控离线 Python stdio peer，没有调用真实模型、账户或外部 API。

最终完整 workspace 测试为 **944 passed、0 failed、5 ignored**，包含 1 项 doctest，
较 ADR-040 增加 16 项测试。fmt、workspace Clippy `-D warnings` 和 diff 检查通过。
计数断言遗漏已修正，以上是修正后完整重跑结果，未把早期失败的运行累计成通过数。

## Test coverage

工作区行覆盖率为 **82.46%（44,528/54,001）**，较可比 ADR-040 基线
82.17%（43,266/52,652）提高 **0.28 个百分点**。

| 范围 | 覆盖行/总行 | 行覆盖率 | 相对 ADR-040 |
| --- | ---: | ---: | ---: |
| workspace | 44,528/54,001 | 82.46% | +0.28 个百分点 |
| server-provider | 6,076/6,597 | 92.10% | -0.05 个百分点 |
| server-api | 816/841 | 97.03% | +0.00 个百分点 |
| server-bin | 387/403 | 96.03% | +0.00 个百分点 |

新增模块：Codex controls 177/183（96.72%）、permissions 169/174（97.13%）、
rewind 97/101（96.04%）、inspection 75/81（92.59%）；RPC controls 207/214（96.73%）、
manager controls 292/303（96.37%）。逐文件数据和源文件哈希见
[可审阅 JSON 覆盖率制品](server-agent-controls-coverage.json)。

普通完整测试：**944 passed、0 failed、5 ignored**，97 个目标，包含 1 项 doctest。
覆盖率完整测试：**943 passed、0 failed、5 ignored**，72 个目标；默认 llvm-cov 不含 doctest。
测试通过数与行覆盖率分别统计。五项跳过包括两个真实 Codex 场景、两个真实 DeepSeek 场景，
以及一个要求外置 worker 的重放场景；制品保存完整测试名和原因。

测量版本：基于 `97f13722f2f17ca298074d9bdbd101562ac3a2ba` 的工作树，包含已有
ADR-038/039/040 改动和本轮 ADR-041。655 个 Rust/Python/manifest 源文件的 SHA-256：

`f3e0dfe559ecab57c6557d9927c0ce3e9b58048eda4460efa2e453ee6b38eb81`

范围：macOS arm64 全工作区、默认 features、cargo-llvm-cov 默认 source filters；
没有显式文件排除，未测 Linux/Windows，也未进行真实模型或账户调用。使用默认清理后的
一次完整覆盖率运行；441 份 profile 均生成于最终源码快照之后，没有合入早期失败数据。
基线采用同一平台、workspace/default features 和源码哈希规则，详见
[ADR-040 制品](server-native-sessions-coverage.json)。

```sh
cargo fmt --all --check
git diff --check
CARGO_TARGET_DIR=/tmp/ait-agent-interface-target cargo clippy --workspace --all-targets --offline -j2 -- -D warnings
CARGO_TARGET_DIR=/tmp/ait-agent-interface-target cargo test --workspace --offline --no-fail-fast -j2 -- --test-threads=1
RUST_TEST_THREADS=1 CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-interface-target cargo llvm-cov --workspace --html --offline --no-fail-fast -j2
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-interface-target cargo llvm-cov report --json --summary-only --output-path /tmp/ait-controls-coverage-raw.json
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-interface-target cargo llvm-cov report --show-missing-lines --offline
```

本机 HTML：`/tmp/ait-agent-interface-target/llvm-cov/html/index.html`；仓库内 JSON 制品
同时包含聚合值、逐文件数据、工具链、引用源码/schema 哈希、基线和验证日志哈希。

未完全覆盖：原生管道写入/关闭失败、审批预算溢出、回答发出后 registry 故障，以及
大规模子 Agent 分页上限和部分畸形描述符组合。回退投影写入故障已有重启恢复测试；
registry 与原生 fork/审批之间仍无事务。后续应补这些故障注入和受支持 Codex 账户的集成
验证；其他 Provider、plan_mode、文件回退属于尚未实现的功能，不计为已验证行为。
