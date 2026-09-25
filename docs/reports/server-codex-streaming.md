# Codex 流式输出与运行中追加输入

本轮只补独立 server 的 Codex adapter，不新增 RPC 方法。协议能力数不代表功能等价率，因此不据此更新与 Paseo 的百分比估算。

参考固定 Paseo `2c8e8a826810337492cc5a38bb0bbd705b6fb632` 中 Codex adapter 的 delta 映射、`steerActiveTurn` 与协议事件；native wire 参照本地 Codex `0.153.4` 导出的 schema。边界见 [ADR-046](../decisions/adr-046-codex-streaming-and-steering.md)。

## 已实现

- assistant 文本、reasoning summary 增量；工具开始、命令/文件输出预览、完整工具结束状态；晚到、重复 completed 以及其他 thread/turn 的通知不污染当前输出。
- `agent_stream` 的每个显示片段先落盘，再带稳定 seq/epoch 推送；完整原生消息另外保留，最终只补齐正文尾部。重连、服务重启、原生历史重载不会把正文加两遍；跨片段搜索返回原始消息位置。
- 显式 `activeTurnBehavior: "steer"` 调用原生 `turn/steer`，核对 `expectedTurnId` 与回执。拒绝保持当前执行；不确定接收失败关闭且不重发。空闲时正常开始新 turn。调用省略该字段仍保持 busy 拒绝。
- 完成、取消、失败状态落盘后发布 turn 事件。已确认接收的 steer 与元数据写重试分离，避免诱发重复发送；保留语音调用的 native turn 所有权约束。
- Timeline SQLite v1/v2 原子升级到 v3。刷新改写时保留旧 epoch 的完整项及增量；I/O 失败不发送未提交内容，重试观察 ID 不产生重复片段。

请求示例：

```json
{"agentId":"<agent-id>","text":"优先修复测试，再继续实现","activeTurnBehavior":"steer"}
```

方法仍是 `agent.message.send.request`；返回的 `accepted` 仅表示本次调用的原生接收结果。

## 仍有差异

当前支持 v2 assistant/summary/command/file 事件，不处理 legacy exec delta、原始 reasoning text 或 token usage。工具预览只保留末尾 16 KiB，最终原生工具结果使用通用 detail 映射，沿用 256 KiB 单项存储上限。单项流式文字预算为 192 KiB，超过后失败关闭。

不支持默认 interrupt/排队、客户端 messageId 幂等、图片附件、审批等待中的 steer、slash command steer，也未增加 plan mode、MCP、tool policy 或持久授权。语音仍使用独占 send 语义。所有验证使用离线 fixture，未调用真实模型、账户或外部 Codex 服务。

## Test coverage

测量使用 `/tmp/ait-codex-streaming-verification` 中固定的 **29 个既有 workspace package**。共享工作树同期正在创建 `server-browser`、`server-schedule`，两者未纳入快照；快照已包含并行 Plugin 接口删除，但六处旧计数断言尚未同步。因此下面是该快照的实测覆盖率，**不是当前持续变化的整个工作树已通过验证的声明**。交付时逐一比对确认，当前 `server-provider` 全部 Rust/fixture 文件及新增 WebSocket 测试与测量快照完全一致。

| 范围 | 覆盖行 / 总行 | 行覆盖率 |
| --- | ---: | ---: |
| 快照 workspace | 47,403 / 57,065 | 83.07% |
| server-provider | 6,679 / 7,237 | 92.29% |
| 新增 streaming / manager streaming / progress 三模块 | 409 / 431 | 94.90% |

参考 [Skills 覆盖率基线](server-skills-coverage.json)：workspace 为 46,917/56,540（82.98%），Provider 为 6,184/6,712（92.13%）。本次分别相差 +0.09、+0.16 个百分点；workspace 包含并行接口变动和未通过的目录断言，差值只能作为背景，不能全部归因于 Codex 本轮实现。

可审阅制品：[逐文件覆盖率、源码哈希和运行结果 JSON](server-codex-streaming-coverage.json)。HTML 位于 `/tmp/ait-agent-interface-target/llvm-cov/html/index.html`。基于 `97f13722f2f17ca298074d9bdbd101562ac3a2ba` 的未提交快照，713 个 Rust/fixture/manifest 输入文件的聚合 SHA-256 为 `4c102be0d7143eac181b02e7e4724f445e03164e497f216c11ac5cdc57584789`；测量前后哈希一致。工具链为 macOS arm64、Rust 1.98.1、cargo-llvm-cov 0.8.4，默认 features 和 llvm-cov 文件过滤，无额外文件排除，覆盖率不含 doctests；Linux、Windows 未运行。

以下命令的工作目录均为上述隔离快照：

```sh
cargo fmt --all --check
CARGO_TARGET_DIR=/tmp/ait-agent-interface-target cargo clippy --workspace --all-targets --offline -j2 -- -D warnings
CARGO_TARGET_DIR=/tmp/ait-agent-interface-target cargo test --workspace --offline --no-fail-fast -j2 -- --test-threads=4
RUST_TEST_THREADS=4 CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-interface-target cargo llvm-cov --workspace --html --offline --no-fail-fast -j2
CARGO_TARGET_DIR=/tmp/ait-agent-interface-target cargo test -p ait-daemon --test codex_http --offline -j2 -- --test-threads=1
LLVM_PROFILE_FILE='/tmp/ait-agent-interface-target/llvm-cov-target/ait-codex-streaming-verification-retry-%p-%m.profraw' /tmp/ait-agent-interface-target/llvm-cov-target/debug/deps/codex_http-ad792b1205746ecb --test-threads=1
LLVM_PROFILE_FILE='/tmp/ait-agent-interface-target/llvm-cov-target/ait-codex-streaming-verification-gui-retry-%p-%m.profraw' /tmp/ait-agent-interface-target/llvm-cov-target/debug/deps/codex_http-ad792b1205746ecb --exact macos_gui_path_reaches_codex_in_the_worker --test-threads=1
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-interface-target cargo llvm-cov report --html
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-interface-target cargo llvm-cov report --json --summary-only --output-path /tmp/ait-codex-streaming-coverage-raw.json
```

测试执行结果与覆盖率分别记录：

- Provider **116 通过、0 失败**；新增真实 server WebSocket 流式、steer、断线和重启用例通过。
- 全量普通运行 **1,009 通过、8 失败、5 ignored**；覆盖率运行 **1,008 通过、8 失败、5 ignored**。两次均遍历了所有快照测试目标，普通运行另含 doctest。
- 其中六项是 Plugin 删除中间状态导致的 server-api / server-bin 目录数量断言，与本轮未增加 RPC 方法的实现分开记录；本轮没有改写这些并行任务的断言。应在并行改动稳定后重跑完整 workspace。
- 其余失败是旧 `ait-daemon::codex_http` 的启动或 worker 握手超时。普通串行复测 **5/5 通过**；同一全量 instrumented 二进制串行复测 **4/5 通过**，剩余 GUI PATH 的模型发现超时随后单独复测 **1/1 通过**。覆盖率合并这些同源码、同编译特征的新增 profiles；保留首次失败，未混入更早运行的 profiles。
- 五个默认跳过项依赖真实 Codex、DeepSeek 凭证/模型额度或外部 worker；完整名称在 JSON 内。格式、全 workspace 快照 Clippy 和原工作树 `git diff --check` 均通过。

未覆盖完的重点是部分默认不支持端口、极限资源预算和所有 I/O/进程时序组合。尚未做真实 Codex 账户联调、同步持久化下的长时间吞吐测量及跨平台验证；后续应在稳定 workspace 上补全量回归，再做这些实测。
