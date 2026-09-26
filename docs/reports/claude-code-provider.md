# Claude Code Provider

这是初次接入的历史报告。后续 provider 能力补齐以[当前清单](../plans/provider-parity.md)和
[ADR-052](../decisions/adr-052-native-provider-capabilities.md)为准；本文覆盖率不是当前代码的测量结果。

日期：2026-09-26。基于 `5e9fc8a759c886fb78e3212ec681391ed97318e7` 与已有工作区修改。
实现范围为独立 Rust server 与当前 Paseo App 的 provider 路径，保留原有未提交修改。

## 实现

`server-provider::local::claude::ClaudeClient` 实现既有 `AgentClient` / `AgentSession`，
生产 server 注册 `claude` 并接受该 provider 的 Agent 创建请求。前端已有 Claude Code 的
名称、图标和模式定义，通过既有 provider snapshot 显示新 adapter。

- 本机 CLI 认证和原生工具；支持 `AIT_SERVER_CLAUDE_BIN` 与 `CLAUDE_CONFIG_DIR`。
- 原生 `initialize` 模型、推理等级和命令发现，不发送模型请求。
- 创建、发送、模型/模式下一轮切换、同 UUID 恢复、取消和进程组回收。
- 文本/思考流与完整内容共用原生键；工具由 user/tool_result 完成，历史重放保持幂等。
- 单次工具审批和 AskUserQuestion，包括当前前端携带完整问题的回答格式。
- 本机历史发现、导入、展示与刷新，含 cwd 校验、读取预算及 macOS NFC/长路径编码。
- 未知控制请求、过大/损坏帧、错误 result、进程退出和握手超时均明确失败。

参考 Paseo `2c8e8a826810337492cc5a38bb0bbd705b6fb632` 的 Claude provider，
以及 Anthropic 官方 SDK 控制协议。边界、原始源码位置和差异见
[ADR-050](../decisions/adr-050-claude-code-provider.md)，使用说明见
[Claude Code](../operations/claude-code.md)。未引入 TypeScript server 或改变核心领域依赖。

本次不包含运行中追加输入、rewind、账户配额、子 Agent 专用面板、fast mode、附件输入，
或宿主侧 providerOptions/MCP 覆盖；未知配置明确拒绝。本机 Claude 配置中的技能与 MCP
继续由 CLI 加载。真实模型推理、真实工具执行和 Windows/Linux 运行尚未验证。

## 验证

新增 17 项默认执行的 adapter 回归、1 项真实 Rust server WebSocket 回归，以及 1 项
默认忽略、可显式执行的已安装 CLI 握手检查。离线测试只使用临时文件、受控 Python CLI
和本机 WebSocket，不读取用户会话或调用付费模型。

已安装 Claude Code **2.1.221** 的 Rust 握手检查通过，发现真实运行时模型，未提交提示词。
格式检查、`git diff --check`、workspace 全目标严格 Clippy 均通过。

| 执行范围 | 通过 | 失败 | 忽略 |
| --- | ---: | ---: | ---: |
| Claude adapter 专项 | 17 | 0 | 1 |
| 真实 CLI 握手（显式运行 ignored 测试） | 1 | 0 | 0 |
| 普通 `server-provider` 完整复测 | 279 | 0 | 1 |
| 覆盖率运行中的 `server-bin`（含 53 项进程测试） | 72 | 0 | 0 |
| 覆盖率 workspace 完整运行 | 1573 | 30 | 6 |
| 相同 instrumented provider 程序在 `/tmp` 复测 | 267 | 12 | 1 |
| 相同 instrumented daemon 超时用例在 `/tmp` 复测 | 1 | 0 | 0 |

本轮没有取得**单次 workspace 全绿**。覆盖率运行的失败集中在旧 daemon 模型发现及
使用 Codex Python 协议替身的测试。采样看到一个启动超过五秒的 `/usr/bin/env` 子进程
仍停在 macOS `_dyld_start`，尚未进入 Python；换到临时目录后仍有加载超时。
provider 两次 instrumented 运行的失败用例不重合，所有失败项均在另一轮相同源码、
相同二进制的 instrumented 运行中通过；逐项对应、二进制哈希和日志哈希收录在 JSON 工件。
这不等于单轮全量通过，也不代表已经修复宿主系统的加载问题。

较早普通 workspace 尝试为 1598 通过、6 失败、6 忽略，其中三项来自 provider 数量/顺序
断言和新增恢复测试误传 `agentId` 而非 `handle`，均已修正并在上述 server 进程测试中通过。
其余三项 Codex I/O 失败在普通 provider 完整复测中通过。中间 server 普通复测的三项
WebSocket 超时也在 instrumented server 全部 53 项进程测试中通过。原始结果保留，不覆盖失败记录。

验证命令：

```sh
cargo fmt --all --check
git diff --check
CARGO_TARGET_DIR=/tmp/ait-agent-session-target cargo test -p server-provider local::claude --offline -- --test-threads=1
CARGO_TARGET_DIR=/tmp/ait-agent-session-target cargo test -p server-provider installed_claude_control_protocol_discovers_models_without_inference --offline -- --ignored --test-threads=1
CARGO_TARGET_DIR=/tmp/ait-agent-session-target cargo clippy --workspace --all-targets --offline -- -D warnings
CARGO_TARGET_DIR=/tmp/ait-agent-session-target cargo test --workspace --offline --no-fail-fast -- --test-threads=1
CARGO_TARGET_DIR=/tmp/ait-agent-session-target cargo test -p server-bin --offline -- --test-threads=1
CARGO_TARGET_DIR=/tmp/ait-agent-session-target cargo test -p server-provider --offline -- --test-threads=1
```

普通测试和覆盖率串行执行；未修改系统安全设置或放宽已有测试超时。
待宿主加载恢复稳定后，应再次执行单轮 workspace 回归与覆盖率，确认全部通过。

## Test coverage

| 范围 | 覆盖行 / 总行 | 行覆盖率 |
| --- | ---: | ---: |
| Cargo workspace | 51114 / 60500 | **84.49%** |
| `server-provider` | 7968 / 8533 | **93.38%** |
| 新增 Claude 生产 adapter | 1183 / 1269 | **93.22%** |
| `server-bin` | 772 / 823 | **93.80%** |

测量为上述固定源码的 workspace 运行与相同 instrumented 二进制复测的 profile 合并结果。
覆盖率反映执行到的代码，不把失败测试计为通过。命令：

```sh
cargo llvm-cov --workspace --html --offline --no-fail-fast -- --test-threads=1
# 完整运行后，对上述两个 instrumented 测试程序在 /tmp 进行复测；确切命令见 JSON。
cargo llvm-cov report --html
cargo llvm-cov report --json --summary-only --output-path /tmp/ait-claude-coverage-summary.json
cargo llvm-cov report --lcov --output-path /tmp/ait-claude-coverage.lcov
```

复测前后确认二进制副本与原件 SHA-256 相同。测量前后 Rust、Python 测试替身与 Cargo
文件的聚合源码 SHA-256 均为
`51a97118745efdfdcaf1d0191feea600612f43165c2db58b0b30450275565931`。
版本为本文开头的 revision 加工作区修改；没有对修改前的 dirty working tree 单独测量，
因此无可比基线或增量百分比。

范围：默认 features、macOS 26.6.2 arm64、cargo-llvm-cov 默认源文件过滤，无额外排除。
doctest 由普通 workspace 测试执行，未做插桩。默认忽略的六项包括五项已有的真实模型/
外部 worker 检查及新增 CLI 握手；后者已另外显式执行并通过。Linux、Windows 未执行。

可审查工件：[覆盖率与测试结果 JSON](claude-code-provider-coverage.json)，包括逐文件行数、
全部源码哈希、完整命令、失败用例复测对应及测量范围。完整本机 HTML 为
`target/llvm-cov/html/index.html`，另已审查 LCOV 未覆盖行。

主要未覆盖行为：实际模型推理与工具执行、Windows 进程清理、默认 PATH/HOME 查找分支、
native permission cancellation、部分历史标题/目录读取与大小上限错误分支、bypass 模式的
子进程启动参数分支及异常进程退出清理。后续应在稳定 CI 中补充这些边界测试，并单独执行
经授权的真实模型端到端验证。
