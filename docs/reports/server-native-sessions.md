# 原生 Session 发现、导入、刷新与上下文导出

实现 [ADR-040](../decisions/adr-040-native-session-import-refresh-context.md)，继续只扩展独立
server。对照固定 Paseo `2c8e8a826810337492cc5a38bb0bbd705b6fb632` 与本机 Codex 0.153.4
app-server schema，接通四个规范方法。

| 方法 | 本轮行为 |
| --- | --- |
| `provider.sessions.recent.list.request` | 原生分页、cwd/since/query/Provider 筛选，排除活动导入记录和 metadata 生成会话 |
| `agent.import.request` | 校验原生身份、目录及完整历史；自动关联 Workspace；重复导入拒绝，归档导入复用原 ID |
| `agent.refresh.request` | 取消本进程 turn、关闭 writer、读取原生历史；原子刷新投影，成功后取消 Agent 归档 |
| `agent.fork_context.request` | 按包含边界的 cursor/message ID 导出 chat_history 文本附件，拒绝过期游标 |

生产 host 已实现方法由 134 增至 **138/195**。扣除 7 个自定义方法，Paseo 规范接口由
127/188 增至 **131/188（69.68%）**，增加 2.13 个百分点，仍有 **57** 个占位。
Agent 为 **25/32**，Provider 为 **7/9**，Session 为 **5/5**。这是接通方法数，不代表
参数、Provider 数量或所有运行行为与 Paseo 等价。

## 实现与兼容边界

四个接口的协议、用例、原生端口和存储均在 server-provider。Workspace 创建/复用仍调用
server-metadata 的 Directory；API 不接收新的业务分支，server-protocol 的依赖约束不变。

导入和发现使用短生命周期 stdio 连接读取原生事实，不启动模型或占用 writer。新 Agent
保留原生 model、reasoning effort 和 createdAt；归档导入保留人工标题及配置，清除旧父 Agent
标签。下一次发送消息通过已有 resume 链路执行。错误只暴露稳定公共错误码。

Timeline v1 自动迁移为 v2。原生历史仅追加时保留 epoch；改写、删除或重排时把旧代原样
存入 retired_entries，事务内切换新 epoch，保留插件条目，提交后推送 replacement 事件。
旧 cursor 的普通查询返回 reset/staleCursor，上下文导出拒绝；不改写领域 Message 树。

仍保留的差异：

- 仅实现 Codex、read-only；recent 的最后 prompt 预览为空，有界扫描可能不覆盖更旧候选。
- 上下文附件只展示工具名称，尚无 Paseo 的细分工具摘要；reasoning/plugin/原始工具输入不进入附件。
- refresh 要求活动且存在的 Workspace，不自动恢复 legacy placement；成功后暂不恢复 writer。
- metadata、Agent JSON 与 Timeline SQLite 没有跨存储事务，后半段失败可能留下 Workspace 或
  未关联投影。refresh 读取失败保留旧投影，但此前 writer 可能已关闭；不提供跨进程 native 锁。
- 退休 Timeline 代保留但暂无清理/查询接口；首次 Timeline 加载仍依赖 Provider，可用性降级、
  外部历史自动监控和数据库范围分页延续 ADR-039 的限制。

Agent 后续剩余：rewind、commands.list、mode.set、feature.set、permission.resolve，
以及 provider_subagents 的 list/timeline.get。Provider 剩 diagnostic 与 usage.list。

## 验证

离线 stdio peer 覆盖分页去重、循环游标、原生 ID/cwd 校验、活动/不完整历史、重复 item
和配置保留。SQLite 回归覆盖 v1 迁移、追加不换 epoch、改写/截断分代、插件保留、失败回滚
和重启。附件回归覆盖两类包含边界、cursor 优先级、空历史、隐私字段排除及超限。

三个真实 WebSocket 场景串联导入筛选与重启、归档恢复、刷新替换事件、取消运行后再执行，
并验证原生读取失败不创建 Agent/Workspace/Project，原生错误正文不泄漏。测试使用可控离线
Python stdio peer，不访问真实 Codex 账户、模型 API 或外部服务。

`cargo fmt --all --check`、workspace Clippy `-D warnings`、`git diff --check` 通过。
完整 workspace 回归为 **928 passed、0 failed、5 ignored**，包含 1 项 doctest。
新增三个 WebSocket 场景均包含在该完整结果中，不另行相加。

```sh
cargo fmt --all --check
git diff --check
CARGO_TARGET_DIR=/tmp/ait-agent-interface-target cargo clippy --workspace --all-targets --offline -j2 -- -D warnings
CARGO_TARGET_DIR=/tmp/ait-agent-interface-target cargo test --workspace --offline --no-fail-fast -j2 -- --test-threads=1
```

早期定向回归发现旧 capability 数量断言及测试对空 registry 文件已存在的错误假设，已在
本次全量测量之前修正。普通回归与覆盖率运行使用相同源码快照。

## Test coverage

Workspace 行覆盖率为 **82.17%（43,266 / 52,652）**，相对
[ADR-039 同范围基线](server-agent-timeline-provider-creation-coverage.json)
81.99%（42,547 / 51,894）增加 **0.1852 个百分点**。

| 涉及的包 | 已覆盖 / 总行数 | 行覆盖率 |
| --- | ---: | ---: |
| server-provider | 4,836 / 5,248 | 92.15% |
| server-bin | 387 / 403 | 96.03% |
| server-api | 816 / 841 | 97.03% |
| server-metadata | 5,472 / 6,238 | 87.72% |

server-provider 相对上一轮 91.63% 增加 **0.5198 个百分点**。新原生适配器为
169/173（97.69%），导入/刷新 RPC 为 163/170（95.88%），管理服务为 240/252（95.24%），
上下文附件为 78/79（98.73%）；修改后的 Timeline 存储为 242/244（99.18%）。

覆盖率插桩运行是 **927 passed、0 failed、5 ignored**，不包含普通回归的 1 项 doctest。
测量范围为完整 workspace、默认 features、cargo-llvm-cov 默认源码过滤，无额外排除文件。
平台 macOS arm64，rustc 1.98.1、cargo-llvm-cov 0.8.4；Linux/Windows 未运行。
忽略的 5 项中 4 项需要真实模型/凭据，1 项需要外部 worker，完整名称与原因见 JSON 工件。

```sh
RUST_TEST_THREADS=1 CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-interface-target cargo llvm-cov --workspace --html --offline --no-fail-fast -j2
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-interface-target cargo llvm-cov report --json --summary-only --output-path /tmp/ait-native-sessions-coverage-raw.json
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-interface-target cargo llvm-cov report --show-missing-lines --offline
```

源码基于 `97f13722f2f17ca298074d9bdbd101562ac3a2ba`，包括进入本轮前的 ADR-038/039
未提交实现与本轮修改。两次全量运行及交付源码使用同一个 SHA-256：
`f67d26eb4ed15eabb7d5b184a4ef18c0785f140e77608843aee79dca931b19d4`。
哈希按路径排序，拼接根 Cargo.toml/Cargo.lock、crates/bins 下的 .rs、.py 和 Cargo.toml
的相对路径、NUL、内容、NUL，共 640 个文件。coverage 使用默认 clean，未合入旧 profile
或增量运行；440 个 profile 均晚于该源码快照，294 个被测源码路径全部存在。

可审查工件：[逐文件覆盖率、源码哈希与测量记录](server-native-sessions-coverage.json)。
本地 HTML 补充入口为 `/tmp/ait-agent-interface-target/llvm-cov/html/index.html`。

重要未覆盖分支包括 recent 省略 cwd、多候选排序及并列顺序、显式 Workspace 导入、旧 native
handle alias、归档记录 cwd 不匹配、refresh 取消超时和关闭 writer 后的存储故障注入。
后续优先补这些路径的定向回归；真实原生历史并发写竞态、跨平台及真实模型会话未在本轮验证。
行覆盖率不代表这些功能边界已经解决。
