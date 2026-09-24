# Server CI 恢复与 Linux 进程组清理

## 原因和修复

[首次失败](https://github.com/necokeine/ait/actions/runs/36046063159)发生在 `cargo fmt` 加载
workspace 时：提交包含已有文件的修改和删除，却遗漏了 `server-model` 等 48 个新增文件。
`b3d83a9` 补回源码、测试、ADR 和覆盖率工件。恢复后的源码哈希与之前完成验证的版本
一致；仅从 Git 暂存区导出的干净副本也通过了 `cargo metadata` 和格式检查。

[补全后的 CI](https://github.com/necokeine/ait/actions/runs/36047101918)通过格式和 Clippy，
随后在 provider 的子进程组清理测试超时。Linux 容器中的隔离复现确认两个问题：

- `/bin/kill -KILL -66` 返回成功，目标进程仍在运行；显式使用 `--` 分隔选项与负数
  进程组 ID 后，信号才可靠地发给该进程组。provider 的 close/Drop 与 metadata 的
  TERM/KILL 调用均补上分隔符。
- 已退出的孤儿进程可能暂时保留 zombie 状态，`kill -0` 仍返回成功。测试改用 `ps` 状态
  区分运行中进程与 zombie，并在触发清理前断言子进程确实在运行。provider 回归同时验证
  显式 close 和直接 Drop；Terminal 仍验证 shell 已退出后的后台进程清理。

没有增加超时、忽略 CI 测试或修改 CI 的测试命令。生命周期、响应和业务协议保持不变。

## 验证

macOS 下 metadata/provider/terminal 三个 crate 的普通测试通过：196 passed、0 failed、
0 ignored。workspace Clippy `-D warnings`、格式与 diff 检查通过。

补充 Linux 容器验证为 Rust 1.94.0、amd64 模拟运行：14 个相关本地适配器测试通过，包括
Session close/Drop、Terminal 进程组和 workspace 脚本启停。容器整组运行曾在既有的
missing-executable 断言中返回 Failed 而不是 Unavailable，未计作全量通过；定向检查过滤了
这个用例。原生 GitHub Linux CI 继续运行未过滤的完整 workspace 测试。

```sh
cargo fmt --all --check
git diff --check
CARGO_TARGET_DIR=/tmp/ait-agent-session-target cargo clippy --workspace --all-targets --offline -- -D warnings
CARGO_TARGET_DIR=/tmp/ait-agent-session-target cargo test -p server-provider -p server-terminal -p server-metadata --lib --offline
```

## Test coverage

最终 workspace 行覆盖率为 **81.63%（41,040 / 50,277）**，比同范围的上次测量
81.6178%（41,035 / 50,277）增加 **0.0099 个百分点**。完整插桩回归为
**898 passed、0 failed、5 ignored**；测试次数与覆盖率分开记录。

| 相关 crate | 已覆盖 / 总行数 | 行覆盖率 |
| --- | ---: | ---: |
| server-metadata | 5,143 / 5,876 | 87.53% |
| server-provider | 3,063 / 3,368 | 90.94% |
| server-terminal | 1,246 / 1,422 | 87.62% |
| server-model | 194 / 206 | 94.17% |
| server-api | 799 / 824 | 96.97% |

测量基于 `b3d83a900af530982baf5838df0f6eb1c5731f37` 加本次四个 Rust 文件的修复。
源码 SHA-256 为 `51826e3caeda5138446ddae9f1226da9d5e299be3ab3b2371f04fc63397ea855`，
测量前后相同。哈希包含根 Cargo.toml/Cargo.lock、crates/bins 下的 .rs 与 Cargo.toml，
按路径排序并拼接相对路径、NUL、内容、NUL。

测量平台为 macOS arm64，使用默认 features、cargo-llvm-cov 默认源码过滤和 4 个测试线程，
没有额外排除文件，不包含 doctest。跳过的 5 项均为既有的真实模型/凭据或外部 worker 测试，
名称见工件。Linux 有上述定向测试与 GitHub CI，但没有 Linux 覆盖率；Windows 未验证。

```sh
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-session-cov-target cargo llvm-cov clean --profraw-only --offline
RUST_TEST_THREADS=4 CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-session-cov-target cargo llvm-cov --no-clean --workspace --html --offline --no-fail-fast -j1
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-session-cov-target cargo llvm-cov report --json --summary-only --output-path /tmp/ait-ci-process-coverage-raw.json
```

[覆盖率工件](server-ci-recovery-coverage.json)记录逐文件结果、命令、测试日志哈希、源码哈希
和[对比基线](server-model-context-coverage.json)。本地 HTML 补充入口为
`/tmp/ait-agent-session-cov-target/llvm-cov/html/index.html`。发现需要修正生产信号调用后，
先停止了较早版本的覆盖率运行并清空 profiles，未将那次未完成的结果混入本次测量。

本次回归覆盖显式 close、Drop 及后台进程退出判定。剩余缺口主要为既有错误消息和连接
准入/传输失败分支。metadata 中 TERM 超时后升级为 KILL 的路径尚未直接覆盖，后续应增加
忽略 TERM 的脚本回归；Windows 进程清理仍需要在对应平台验证。
