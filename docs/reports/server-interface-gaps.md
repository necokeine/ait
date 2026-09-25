# 独立 server 与 Paseo 的剩余接口

- 核对日期：2026-09-25。
- 口径：当前工作树（包含未提交实现），固定 Paseo `2c8e8a826810337492cc5a38bb0bbd705b6fb632` 的规范方法。
- 完整基线来自 Paseo `SessionInboundMessageSchema` 的 205 个原始名称，按用户要求排除 Hub、Chat、Loop、Plugin 共 34 项，再合并三个旧别名后为 168 个规范方法；独立提取 fixture 减去显式排除清单后与 Rust catalog 做集合相等校验。
- 用完整 catalog 减去生产 host 安装的各能力 crate 的 `IMPLEMENTED_GROUPS` 统计占位；“已接通”仅表示有业务处理器，不表示全部行为与 Paseo 对齐。
- 当前范围的 168 个 Paseo 规范方法均有处理器，剩 0 个 catalog 占位。加上 7 个自定义方法，生产安装总数为 175/175；这不是行为等价率。
- Schedule 9 项和 Browser 2 项已接入；Plugin 15 项已删除。详见 [本轮报告](server-schedule-browser.md)。

## 分组统计

| 协议业务组 | 已接通 / 总数 | 剩余 |
| --- | ---: | ---: |
| Daemon | 9/9 | 0 |
| Project | 10/10 | 0 |
| Workspace | 24/24 | 0 |
| Agent | 32/32 | 0 |
| Provider | 9/9 | 0 |
| Skills | 5/5 | 0 |
| Git | 30/30 | 0 |
| Files | 11/11 | 0 |
| Terminal | 10/10 | 0 |
| Editor（旧兼容响应） | 2/2 | 0 |
| Schedule | 9/9 | 0 |
| Voice | 8/8 | 0 |
| Session | 5/5 | 0 |
| Push | 2/2 | 0 |
| Browser | 2/2 | 0 |

Skills 虽然使用 `agent.skills.*` 前缀，在协议目录里单独分组，不包含在 Agent 的 32 个方法中。

Skills 的目录配置、删除确认、恢复与上游差异见 [实施报告](server-skills.md)。

## 尚未完全对齐的行为

生产目录没有占位。Browser 尚未向 Codex 注入 MCP browser tools，Schedule 沿用当前 Codex Provider、只取最终文本且未接 Agent 归档即时 sweep；完整说明与资源限制见[Schedule / Browser 报告](server-schedule-browser.md)。其他能力仍有以下已知差异。

## Push 管理后续实现

`push.register` 与 `push.unregister.request` 已实现生产装配，包含私有文件持久化、48 小时租约、半租约续租抑制、心跳续租、旧 tokens 数组迁移和连接局部撤销状态。实现边界、与上游差异及当前验证见 [Push 实施报告](server-push-tokens.md)。实际 Expo 投递及 Agent 结束事件到通知发送的链路尚未实现，不能将 Token 管理支持等同于通知投递支持。

Hub、Chat、Loop 已按用户要求删除，不再登记、协商或返回占位响应。旧名称与规范名称均视为未知方法；19 项仅保留在测试排除清单和原始上游 fixture 中，用于防止误恢复。见 [删除报告](server-removed-groups.md)。

## 上一轮目录与状态修复

- 补齐 Chat、Loop 与旧 Editor 共 14 个漏登记名称；两个 Editor 方法按 Paseo 返回迁移到桌面端的兼容响应，不打开本地程序。
- 保存从上游独立提取的 [205 项 fixture](../../crates/server-protocol/src/methods/fixtures/paseo-inbound.txt)，增加完整集合校验。可运行 `python3 scripts/check-paseo-protocol.py /path/to/paseo` 验证 fixture；升级基线时必须审阅 pin 和差异。
- daemon 状态和诊断从生产 Provider 服务读取可用性；未安装 Provider 时为空，已注册但不可用时显示该 Provider 为 unavailable。诊断不包含 Provider 后端错误详情。
- 此次没有实现原有 40 个业务占位或新登记的 Chat/Loop；目录来源以外的 Workspace 组合创建等差异仍需后续实现。

## 已接通方法仍存在的差异

- 配对功能尚未安装，接口已改为明确返回 `unsupported_capability`，不再伪装为成功的空 URL；安装方式不支持自更新，`daemon.update.request` 明确返回失败。这两项仅接通处理器。


- Voice 已接入八个方法，后端需显式配置；本地为 whisper.cpp/Piper，未包含 Sherpa/Parakeet 和模型下载器，详见 [ADR-042 实施报告](server-voice.md)。
- Agent 执行/导入目前仅有 Codex 适配器，支持 read-only/auto/full-access 与 fast_mode，未接 plan_mode、files rewind 或持久审批授权；接通方法不等于多 Provider 支持。
- Timeline 仍按事件序号提供展示投影；Codex 已支持持久增量、最终正文去重和游标续传，尚无完整 Paseo 投影合并语义；首次读取仍依赖原生 Provider。显式 steer 及限制见 [ADR-046 实施报告](server-codex-streaming.md)。
- `agent.fork_context.request` 的工具摘要仅包含名称；refresh 不自动恢复已归档或丢失的 Workspace。
- Workspace 创建只接目录来源，Workspace + Agent/worktree 组合创建仍拒绝。
- Session 五个方法已接通，事件订阅目前只支持 Provider snapshot、Agent attention、server info、daemon config 四类生产者，并未接通 Paseo 全部事件类别。
- 完整限制与已验证范围见 [ADR-039 实施报告](server-agent-timeline-provider-creation.md) 、[ADR-040 实施报告](server-native-sessions.md) 和 [ADR-041 实施报告](server-agent-controls.md)。

## 核对来源

- [规范方法目录](../../crates/server-protocol/src/methods.rs)。
- [能力集合合并](../../crates/server-api/src/capabilities.rs) 与 [生产服务装配](../../bins/server/src/host.rs)。
- [metadata 声明](../../crates/server-metadata/src/capabilities.rs)、[filesystem 声明](../../crates/server-filesystem/src/capabilities.rs)、[provider 声明](../../crates/server-provider/src/capabilities.rs)、[terminal 声明](../../crates/server-terminal/src/capabilities.rs)。
- [生产方法覆盖及占位测试](../../bins/server/tests/process/catalog.rs)。

## Test coverage

本轮结果见 [Schedule / Browser 实施报告](server-schedule-browser.md)。以下为历史目录与状态修复的测量，不代表当前工作树。

### 历史目录与状态修复

修订：`97f1372` 加当前未提交工作树（包含本轮开始前用户已有修改）；macOS aarch64，默认 features。

- **Workspace coverage: not measured**。`cargo llvm-cov --offline --workspace --html` 在旧 worker 的 `supervisor_rejects_bad_workers_and_reaps_descendants` 失败后中断（预期 `VersionMismatch`，实际 `HandshakeTimeout`），不能把部分运行当作全量覆盖率。下一步需独立排查旧 worker 的启动/进程清理时序，再重跑 workspace 覆盖率。
- 本轮四个修改包的行覆盖率：**89.51%（6,957 / 7,772）**。命令：`cargo llvm-cov --offline -p server-protocol -p server-metadata -p server-api -p server-bin --html`。没有额外的文件排除或手工跳过测试，使用 llvm-cov 默认报告过滤；其他 workspace 包和其他平台不在这个百分比范围内。没有可直接比较的同口径基线，不计算覆盖率变化。
- 可审阅的逐文件覆盖率制品：[server-interface-fixes-coverage.json](server-interface-fixes-coverage.json)。HTML 在本地 `target/llvm-cov/html/index.html`，未发布到共享站点。

| 修改包 | 已覆盖 / 总行数 | 行覆盖率 |
| --- | ---: | ---: |
| server-bin | 463 / 483 | 95.86% |
| server-api | 886 / 912 | 97.15% |
| server-metadata | 5,551 / 6,320 | 87.83% |
| server-protocol | 57 / 57 | 100.00% |

新增回归覆盖完整上游集合、旧 Editor 成功/校验错误/未知方法、daemon Provider 可用与不可用、诊断脱敏、配对明确拒绝及生产占位分发。Editor RPC 行覆盖率为 100%；daemon RPC 为 97.67%，API 跨能力分发为 98.84%。其余未覆盖主要仍在 metadata 的存储/文件失败分支；Provider 返回无法解码的内部 availability 数据分支未注入测试。52 项占位和已有部分支持的功能不因该覆盖率而视为已实现。

测试执行结果（与覆盖率分别记录）：

- `cargo test --offline --target-dir target/server-audit -p server-protocol -p server-metadata -p server-api -p server-bin`：**200 通过，0 失败，0 跳过**；选包覆盖率运行也全部通过。
- `cargo test --offline --target-dir target/server-audit --workspace --no-fail-fast`：**977 通过，2 失败，5 跳过**。失败为旧 `ait-daemon::codex_http::macos_gui_path_reaches_codex_in_the_worker`（握手超时）和 `ait-tools::shell_permissions::cancels_descendants_before_drain_returns_and_cleans_up_after_normal_exit`（子进程退出断言）。两者在随后的 workspace 覆盖率运行均通过；前者另经 `cargo test --offline --target-dir target/server-audit -p ait-daemon --test codex_http macos_gui_path_reaches_codex_in_the_worker -- --exact` 单独复验通过。本轮没有修改这些旧 Ait 路径，未将首次失败隐去。
- 全量测试的 5 个跳过项为仓库显式 ignored 的真实 Codex/DeepSeek 调用及外部 worker 回放，分别需要模型凭据、付费调用或 `AIT_TEST_WORKER_EXECUTABLE`；本轮未额外屏蔽测试。
- `cargo clippy --offline --target-dir target/server-audit-lint --workspace --all-targets -- -D warnings`、`cargo fmt --all --check`、`git diff --check` 均通过。
- `python3 scripts/check-paseo-protocol.py /Users/necokeine/Documents/paseo` 验证 fixture 与固定上游版本的完整 205 项入站定义一致。
