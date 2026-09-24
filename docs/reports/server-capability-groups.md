# 独立 server：crate 自有能力分组与安装统计

实现 [ADR-035](../decisions/adr-035-server-capability-groups.md)。方法分组和安装规则由
metadata、filesystem、provider、terminal 各自的 `capabilities` 模块维护；server-api 合并
方法列表，并把带 crate 归属的分组连接到既有处理器。

## 变更

- 每个能力 crate 公开 `Group`、`IMPLEMENTED_GROUPS`、`InstalledServices` 和
  `installed_capabilities`，方法名称继续复用各自的 protocol 常量。
- 路由注册和路由测试共用 API 汇总迭代器，删除 API 内的逐组注册清单和测试中的重复清单。
- API 只传递服务存在性；Directory 对配置/图标的启用规则、Agent runtime/execution 的
  lifecycle 联合安装规则，以及 metadata 常驻方法都在所属 crate 内判断。
- 完整安装仍为 122 个已实现方法、195 个可协商方法；空宿主仍为 6/190。
  能力数组改为按 crate 分组排列，方法名称、安装条件与 request/event 方向保持不变。
- 保留 API 内 `server.status.unsubscribe` 兼容路由、现有协商和占位错误语义。
  未修改业务 dispatch、订阅激活顺序、执行服务或 crate 依赖。

## 验证

新增 10 个测试：四个能力 crate 各验证安装组合和完整分组，API 验证基础方法及合并唯一性。
组合覆盖 metadata 32 种、filesystem 64 种、provider 8 种、terminal 2 种，共 106 种。
已有路由测试继续验证精确名称、不同 owner、消息方向、协商要求与占位处理。

专项测试：15 passed，0 failed；其中 10 项为新增测试，5 项为名称匹配到的既有回归。
完整插桩回归：890 passed、0 failed、5 ignored，71 个测试 target。
普通全工作区回归：891 passed、0 failed、5 ignored，包含 1 项通过的 doctest。

`cargo fmt --all --check`、`git diff --check` 和全工作区 Clippy `-D warnings` 均通过。
Clippy 首轮发现两个新测试计数公式含多余的 `0 +`，删除后重新运行通过。

```sh
cargo fmt --all --check
git diff --check
CARGO_TARGET_DIR=/tmp/ait-agent-session-target cargo test -p server-metadata -p server-filesystem -p server-provider -p server-terminal -p server-api --offline capabilities -- --test-threads=1
CARGO_TARGET_DIR=/tmp/ait-agent-session-target cargo clippy --workspace --all-targets --offline -- -D warnings
CARGO_TARGET_DIR=/tmp/ait-phase10-workspace-target cargo test --workspace --no-fail-fast --offline -- --test-threads=1
```

## Test coverage

| 范围 | 已覆盖 / 总行数 | 行覆盖率 |
| --- | ---: | ---: |
| Workspace | 40,895 / 50,154 | 81.5389% |
| server-api | 2,190 / 2,348 | 93.2709% |
| server-metadata | 4,811 / 5,528 | 87.0297% |
| server-filesystem | 6,064 / 7,291 | 83.1710% |
| server-provider | 2,921 / 3,214 | 90.8836% |
| server-terminal | 865 / 988 | 87.5506% |
| 本轮五个 capabilities 模块 | 103 / 103 | 100% |

审查工件：[逐文件覆盖率、命令、测试统计与源码校验值](server-capability-groups-coverage.json)。
HTML 在 `/tmp/ait-agent-session-cov-target/llvm-cov/html/index.html`；JSON 工件列入本次变更供审查，
不依赖这个本机 HTML 路径。

测量 revision 为 `3965cf5157adfee8ba05fbbde9916e8bd78609d6` 加本轮未提交改动。
测量前后 Rust/manifest SHA-256 一致：
`3e5cffb53b628cdf8c4d40a553be37f36ceef844249d07bfb125ca460fd0ea77`。
范围为全 workspace、
默认 features、cargo-llvm-cov 默认源文件过滤，无额外文件排除，不测 coverage doctest。
平台为 macOS arm64；Linux 和 Windows 未测。

五个既有 ignored 用例涉及真实 Codex/DeepSeek 模型、凭据或外置 worker；未为本轮额外跳过
测试，也没有执行付费模型调用。普通测试与插桩测试的结果分别记录，测试数量不作为覆盖率。

没有在本轮修改前重新测量 HEAD。历史同口径的
[Agent/Session 报告](server-agent-session.md)为 40,948 / 50,205，81.5616%；当前工作区指标
低 0.0227 个百分点。历史报告的源码校验值与本轮 HEAD 不同，这个差值只作历史参考，
不能全部归因于本次重组；各 crate 的历史行数和百分比也保存在 JSON 的 baseline 中。

新增能力模块没有未覆盖行。现有独立 server 缺口集中在 filesystem 的本机 Forge/Checkout、
Worktree 实现及 metadata 的 Directory RPC 等模块；后续应按这些模块补充边界和故障用例，
并在 Linux/Windows 运行平台回归。本次没有扩大这些业务实现的测试范围。

```sh
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-session-cov-target cargo llvm-cov clean --workspace --profraw-only --offline
RUST_TEST_THREADS=1 CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-session-cov-target cargo llvm-cov --no-clean --workspace --html --offline --no-fail-fast -j1
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-session-cov-target cargo llvm-cov report --json --summary-only --output-path /tmp/ait-capability-groups-coverage-raw.json
```

仅复用编译缓存，开始测量前已清理 profiles，433 份 profiles 均晚于本轮源码快照；
`--profraw-only` 与 `--workspace` 同用产生兼容性
warning，命令成功完成，后续全 workspace 测试与报告生成均成功。
