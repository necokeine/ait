# server-protocol 依赖清理

实现 [ADR-038](../decisions/adr-038-server-protocol-dependencies.md)：server-protocol 的内部
依赖由 model 与四个能力包收敛为仅 model。ADR 包含根据 Cargo metadata 核对的完整依赖图。

基础连接 CAPABILITIES 移到 server-model::server，metadata/protocol 保留原导出路径。
静态协议目录中的 Skills 与 heartbeat 使用规范字符串；文件/Skills 的目录一致性测试迁到
API，并补充基础能力与 heartbeat 的契约检查。metadata/provider 的错误转换测试迁到各自
rpc 子文件，保留原错误码、消息与 retryable 断言。

架构守卫限制 protocol 只能依赖 model，并验证不能通过开发/构建、optional、重命名或平台
条件依赖绕过边界。请求信封、JSON、195 个规范方法与 122 个已实现能力保持兼容。

## 验证

格式、workspace Clippy `-D warnings` 和定向回归全部通过。定向回归为 **75 passed、
0 failed、0 ignored**；完整 workspace 回归为 **901 passed、0 failed、5 ignored**，
包含 1 项 doctest。两次运行的测试数量分别记录，不相加计算不同测试总数。

```sh
cargo fmt --all --check
git diff --check
CARGO_TARGET_DIR=/tmp/ait-agent-session-target cargo clippy --workspace --all-targets --offline -- -D warnings
CARGO_TARGET_DIR=/tmp/ait-agent-session-target cargo test -p server-protocol -p server-api -p server-bin --offline --no-fail-fast -- --test-threads=1
CARGO_TARGET_DIR=/tmp/ait-agent-session-target cargo test --workspace --offline --no-fail-fast -j2 -- --test-threads=1
```

完整普通回归与覆盖率测试串行执行，避免固定测试身份争用系统账户锁。
ADR 的依赖图与 Cargo metadata 的 9 个 crate、22 条直接边逐条核对：21 条普通边、1 条开发边。
`cargo tree -p server-protocol --edges normal,build,dev --offline` 中的内部包仅有 protocol 和 model。

## Test coverage

本轮 workspace 行覆盖率为 **81.63%（41,040 / 50,277）**。与
[公共 Context 实现的历史基线](server-model-context-coverage.json)的
81.62%（41,035 / 50,277）相比增加 **0.0099 个百分点**；server packages 合计为
**87.42%（17,612 / 20,146）**。差异的 5 行均位于本轮未修改的
`server-provider/src/local/codex/transport.rs`；当前基线提交还包含历史测量之后的 Unix
进程组测试修复，因此不把该增量归因于本次依赖清理。

| 范围 | 已覆盖 / 总行数 | 行覆盖率 |
| --- | ---: | ---: |
| server-protocol | 57 / 57 | 100.00% |
| server-model | 194 / 206 | 94.17% |
| server-api | 799 / 824 | 96.97% |
| server-metadata | 5,143 / 5,876 | 87.53% |
| server-filesystem | 6,615 / 7,884 | 83.90% |
| server-provider | 3,063 / 3,368 | 90.94% |
| server-terminal | 1,246 / 1,422 | 87.62% |
| server-domain | 121 / 121 | 100.00% |
| server-bin | 374 / 388 | 96.39% |

迁回 metadata/provider 的错误转换所在 `rpc.rs` 分别为 14/14、17/17 行，均为 100%。
protocol、model、API、metadata 的已覆盖/总行数与历史基线一致。

测量源码为 `97f13722f2f17ca298074d9bdbd101562ac3a2ba` 加本次未提交的依赖清理，
源码 SHA-256 为 `7c944e4b7191cfe4924a09c4cc1171c0480908d1ba6d96cef765804a91ce8a0f`，
测量前后相同。哈希按路径排序，对根 Cargo.toml/Cargo.lock、crates/bins 下的 .rs 与
Cargo.toml 依次拼接相对路径、NUL、内容、NUL。

测量范围为完整 workspace、默认 features，使用 cargo-llvm-cov 默认源码过滤，未额外
排除文件。平台为 macOS arm64，rustc 1.98.1、cargo-llvm-cov 0.8.4；Linux/Windows 未执行。
带覆盖率插桩的测试为 **900 passed、0 failed、5 ignored**，不包含普通回归中的 1 项
doctest。5 项忽略测试沿用原设置：4 项需要真实模型/凭据，1 项需要外部 worker；完整名称
和原因记录在 JSON 工件中。测试通过数与行覆盖率分别统计。

为避免旧 server 构建映射混入报告，测量前清理所有 server 包对象及旧 profiles，再完成一次
完整 workspace 测量。最终 281 个源码路径均存在，434 个 profiles 均生成于本次源码快照之后。

```sh
cargo clean --target-dir /tmp/ait-agent-session-cov-target/llvm-cov-target -p server-api -p server-model -p server-metadata -p server-filesystem -p server-provider -p server-terminal -p server-protocol -p server-domain -p server-bin
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-session-cov-target cargo llvm-cov clean --profraw-only --offline
RUST_TEST_THREADS=1 CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-session-cov-target cargo llvm-cov --no-clean --workspace --html --offline --no-fail-fast -j1
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-session-cov-target cargo llvm-cov report --json --summary-only --output-path /tmp/ait-protocol-deps-coverage-raw.json
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-session-cov-target cargo llvm-cov report --show-missing-lines --offline
```

可审查工件为 [server-protocol-dependencies-coverage.json](server-protocol-dependencies-coverage.json)，
包含逐文件行数、各包聚合、源码与原始导出哈希、准确命令、测试结果和历史基线。本地完整
HTML 位于 `/tmp/ait-agent-session-cov-target/llvm-cov/html/index.html`，作为补充查看入口。

剩余缺口包括公共错误消息的部分枚举分支、单个 binary frame 超过队列总字节预算时的拒绝，
以及连接处理中的部分无效订阅、预算/激活失败、传输和 terminal 错误分支。Runtime 仍为
48/49，missing-lines 导出未定位到完全未覆盖的具体行。后续测试应优先覆盖订阅预算与
错误响应交互，以及超大 binary frame 的拒绝行为；本次只调整依赖和测试归属。
