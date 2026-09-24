# 独立 server：Agent 后续配置与 Session 事件

实现 [ADR-034](../decisions/adr-034-agent-config-session-events.md)，新增五个可协商且已接通的方法：

| 方法 | 所属 crate | 已实现行为 |
| --- | --- | --- |
| `agent.model.set.request` | server-provider | 校验并持久化下一轮模型覆盖 |
| `agent.thinking.set.request` | server-provider | 校验并持久化下一轮推理等级 |
| `agent.config.apply.request` | server-provider | 原子应用 model/thinking 配置，省略保留、null 清除覆盖 |
| `session.events.set_subscription.request` | server-metadata | 连接拥有的有界订阅、响应先于事件、通用释放 |
| `session.heartbeat` | server-metadata | 无应答活动事件、焦点抑制及通知连接选择 |

参考固定 Paseo `2c8e8a826810337492cc5a38bb0bbd705b6fb632` 的 messages、session、agent-config、
attention policy 与 Codex adapter；核对本机 codex-cli 0.153.4 的 `TurnStartParams` schema。
Agent 从 14/32 到 **17/32**，Session 从 2/5 到 **4/5**。这里统计已接入的方法，不代表与 Paseo
所有参数、事件类别和 Provider 的完整行为对齐。

本轮自身新增五个方法，基于上轮 107 个已实现方法计为 112。工作区同时接入独立 Terminal 的十个
方法后，实际 host 是 **122/195**；其中 Paseo 规范方法 **115/188（61.17%）**，自定义方法 7 个。
并行 Terminal 的实现与限制单独记录在 ADR-033；本报告的整工作区测量包含这些同时存在的修改。

## 实现与边界

Provider 在同一 Agent JSON record 内一次提交配置和 updatedAt，失败保留旧配置及无关元数据。
活动 turn 的配置在开始时冻结，后续修改附带 next-turn notice；下一次 `turn/start` 读取最新 durable
配置。原生接纳后的 runtime 信息写失败保留待写状态，不把已接纳 turn 错报为未接纳；完成/失败
attention 在终态提交成功后发布一次。取消、内部、归档与已删除 Agent 不产生完成通知。

Session 协议及进程内 presence/subscription 归 metadata，provider 通过原有向内依赖发布业务事件，
API 负责鉴权、协商、连接所有权、16 个共享订阅配额和有界 outbound。订阅准备阶段最多缓存
64 条 / 1 MiB，慢连接不能回滚已提交的业务写入。释放与断连回收 owner；事件不持久化或回放。

事件只开放已有 producer 的 `agent_attention_required`、`status.daemon_config_changed` 和
`status.server_info`。心跳校验时间和 focus ID、截断未来 activity，180 秒后 presence 过期。
所有订阅者收到状态，最多一个启用通知且有新鲜心跳的连接收到 `shouldNotify=true`；任意新鲜的
可见连接正在查看目标 Agent 时，全部抑制通知。没有外部 push。

模型是否存在及是否获授权由原生调用决定；配置 accepted 表示保存成功。null 清除宿主覆盖并按
原生继承规则执行，不承诺恢复创建时默认值。当前仍只支持 read-only Codex；模式/feature 修改、
权限交互、其他 Provider、timeline/流式历史、import/refresh 和 `creation.subscribe.request`
尚未实现。连接 Session 不改变 ADR-001 的 Message 树、领域 Session 引用或 Run 完成条件。

## 验证

新增测试覆盖 omitted/null patch、原子配置与错误回滚、活动 turn 不被修改、下一轮原生参数、
重启恢复配置、归档拒绝、runtime/terminal 写失败重试，以及已提交事件只发布一次。
Session 测试覆盖准备/激活顺序、主题过滤、订阅/字节预算、跨连接释放隔离、断线和过期、
重复订阅去重、焦点抑制、未来时间截断、无心跳、producer 缺失、坏请求及 draining 通知。
真实 WebSocket/子进程测试使用离线协议 peer；没有调用真实模型或外部付费 API。

最终源码的完整插桩回归：**880 passed、0 failed、5 ignored**，71 个测试 target；生成 HTML
和逐文件 JSON 工件。`cargo fmt --all --check`、`git diff --check` 与 workspace Clippy
`-D warnings` 均通过。

普通完整工作区首轮为 877 passed、2 failed、5 ignored，另 1 项 doctest 通过。失败分别是旧 daemon
的离线 worker handshake 超时，以及旧 workspace-local 的 Git deadline 状态断言；对应完整 target
未改业务源码补跑后为 5/5、30/30。较早的服务端专项遇到两个 WS 等待超时；进程 target 补跑通过
这两个用例，但一次既有 checkout refresh 断言失败，该用例单独重跑通过。故障/生命周期专项
18/18 通过，包含最后新增的四类 attention 抑制情形。上述原始结果与重跑结果在工件中分别保留，
最终完整插桩回归验证当前冻结源码，不把补跑结果改写成首轮全绿。

## Test coverage

| 范围 | 已覆盖 / 总行数 | 行覆盖率 | 相对上轮 |
| --- | --- | --- | --- |
| Workspace | 40948 / 50205 | **81.5616%** | +0.3738 个百分点 |
| 八个 server package | 17521 / 20074 | **87.2821%** | 新增 Terminal，范围扩大 |
| server-provider | 2909 / 3202 | 90.8495% | +0.1762 个百分点 |
| server-metadata | 4796 / 5513 | 86.9944% | +0.4639 个百分点 |
| server-api | 2295 / 2451 | 93.6353% | -0.6344 个百分点 |
| server-bin | 374 / 388 | 96.3918% | +0.0471 个百分点 |

新增 Session 服务为 216/221（97.7376%），API Session 接线为 74/79（93.6709%），
Agent 配置 patch 为 17/17（100%）。共享审查工件：
[逐文件覆盖率、源码校验值、命令与测试结果](server-agent-session-coverage.json)。基线为
[上一轮原生 Provider 执行覆盖率](server-native-provider-coverage.json)：workspace 39096/48155，
81.1878%；七个 server package 15669/18024，86.9341%。

revision：`e9261b8b5a74b39eebaaef1983495f1892e8d389` 加本轮 Agent/Session 及并行 Terminal 工作区
修改。测量前后 Rust/manifest SHA-256 一致：
`978ae924315db65e0e13dc904cd254cd4c74922a6e970cc7569b5d332bc144d1`。
按相对路径排序，对 crates/bins 下全部 `.rs`、Cargo.toml，以及根 Cargo.toml、Cargo.lock，依次
hash `path + NUL + file bytes + NUL`；离线 peer、Paseo 源文件与 native schema 校验值也在工件中。

```sh
cargo fmt --all --check
git diff --check
CARGO_TARGET_DIR=/tmp/ait-agent-session-target cargo clippy --workspace --all-targets --offline -- -D warnings
CARGO_TARGET_DIR=/tmp/ait-phase10-workspace-target cargo test --workspace --no-fail-fast --offline -- --test-threads=1
CARGO_TARGET_DIR=/tmp/ait-agent-session-target cargo test -p server-metadata -p server-provider -p server-api -p server-bin --offline --no-fail-fast -- --test-threads=1
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-session-cov-target cargo llvm-cov clean --workspace --offline
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-session-cov-target cargo llvm-cov clean --workspace --profraw-only --offline
RUST_TEST_THREADS=1 CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-session-cov-target cargo llvm-cov --no-clean --workspace --html --offline --no-fail-fast -j1
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-session-cov-target cargo llvm-cov report --json --summary-only --output-path /tmp/server-agent-session-coverage-raw.json
```

首次冷构建测量在并行 Terminal 源码更新后停止，其 profiles 全部清除；最终测量只复用编译缓存，
重新构建受影响的 package 并执行全部 workspace 测试。核对 433 份 profiles 均晚于重置时间，
没有混入旧版本 profiles。逐项补跑命令与重置工具的参数提示保存在工件中。

测量使用整个 workspace 默认 features，无额外文件排除，cargo-llvm-cov 默认测试/build-source
过滤；覆盖率不包含 doctest。环境为 macOS arm64、rustc 1.98.1、cargo-llvm-cov 0.8.4；
Linux/Windows 未执行。HTML 位于 `/tmp/ait-agent-session-cov-target/llvm-cov/html/index.html`，
共享审查使用上面的 JSON 工件。

5 个忽略项沿用基线：真实 Codex/Python、DeepSeek 实时 catalog、WF10 Codex、WF11 DeepSeek，
以及需要外部 worker 的权限 replay。本轮没有新增 ignored 测试。未覆盖真实模型认证与不存在模型
的原生拒绝、部分 mutex/channel/registry 故障及少数 observer 生命周期交错；Terminal 的平台和
仿真限制另见其报告。后续新增 Provider、完整事件 producer 或跨平台支持时需补对应集成验证。
