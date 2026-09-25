# 公共 Context 与具体 crate 分发

实现 [ADR-037](../decisions/adr-037-server-model-context.md)。Context 不再归 API，也不再通过
Host trait 回调 API；四个能力 crate 直接接收公共 Context、自己的具体 State 与连接状态。

## 变更

新增 `server-model`，承载 Request/Context、共享 Tokio Runtime、稳定错误/响应、公共服务器
元数据以及有界 Outbound。该 crate 不依赖任何业务 crate、API、protocol 或 axum。原公共
类型在 metadata/protocol 路径继续重导出，业务错误转换归各自业务 crate。

API 的 Shared 只组装四个具体 State、同一个 Runtime 及认证/连接准入信息。File/checkout、
Terminal、Session/labels/daemon 和 Agent 的原连接处理分别迁入所属 crate，API 保留 HTTP/WS
入口、鉴权、握手、物理连接循环与跨能力收尾。文件 HTTP 下载保留在 API，分块读取属于
filesystem；公共队列的 Text/Binary Frame 仅在 socket 写入时转换成 WebSocket Message。

分发路径为 API 准入检查 → 所属 crate → 能力组 → 具体 RPC/连接处理。删除四份 Host trait、
四份 API Host 实现、Operation 类型别名及 callback-only 测试，不使用 async-trait/boxed future
或类型擦除来替代它们。新增 server-model 依赖边，能力 crate 直接使用 Tokio；model 禁止
反向依赖业务/传输，domain 继续保持纯领域边界。

跨能力收尾用具体结果描述：metadata 返回订阅释放意图，API 释放各 crate 连接资源并回应；
provider 返回 Agent 关闭结果和 Terminal IDs，API 调用 terminal 完成关闭并发送合并结果。
Terminal 能力缺失仍在 Agent 关闭前拒绝。Workspace update、初始 status、标签与 checkout
订阅激活仍在响应成功入队后发生。全部连接订阅计入同一个预算，替换规则由所属 crate 处理。

普通阻塞 RPC 复用公共 Runtime.run；工作一旦移入 blocking 池，permit 与任务跟踪跟随工作
本身。响应取消不会提早释放预算或使 shutdown 漏等任务。Terminal、Agent wait 保留各自的
预算和追踪，模型/协议/持久化字段、方法及 capability 安装规则不变。

## 验证

格式、workspace Clippy `-D warnings` 和 server-model/server-api/server-bin 的 71 项测试通过，
0 failed、0 ignored。该组包含公共上下文、队列、真实 WebSocket 与 server 进程集成回归。
完整 workspace 回归通过：899 passed、0 failed、5 ignored，包含 1 项 doctest。
覆盖率运行的测试结果单独记录，不与上述次数相加计算不同测试总数。

公共 RPC/队列测试从 API 迁入 model；增加取消响应后等待已开始阻塞任务的真实调度回归，
以及 admission 的缺失服务、预算耗尽、draining 分支。依赖守卫验证 model 无反向边并检查
Tokio 的允许范围。旧 Host 的 RecordingHost 测试随被删除的接口一起移除。

验证命令：

```sh
cargo fmt --all --check
git diff --check
CARGO_TARGET_DIR=/tmp/ait-agent-session-target cargo clippy --workspace --all-targets --offline -- -D warnings
CARGO_TARGET_DIR=/tmp/ait-agent-session-target cargo test -p server-model -p server-api -p server-bin --offline --no-fail-fast -- --test-threads=1
CARGO_TARGET_DIR=/tmp/ait-agent-session-target cargo test --workspace --no-fail-fast --offline -- --test-threads=1
```

完整普通回归和覆盖率测量串行执行。较早接口草稿曾并行运行这两项，固定 `recovery-project`
测试身份争用系统账户共享锁；该草稿的测量不作为本次结果，最终源码重新独立验证。

## Test coverage

最终 workspace 行覆盖率为 **81.62%（41,035 / 50,277）**。与上一版 Host 分发实现的
81.56%（40,918 / 50,169）相比增加 **0.0575 个百分点**。server packages 合计为
87.40%（17,607 / 20,146），增加 0.1079 个百分点。各 crate 的代码归属发生迁移，不能直接
把单个 crate 的覆盖率变化解释为相同代码的测试增减；新增 model 没有独立的历史基线。

| 范围 | 已覆盖 / 总行数 | 行覆盖率 |
| --- | ---: | ---: |
| server-model | 194 / 206 | 94.17% |
| server-api | 799 / 824 | 96.97% |
| server-metadata | 5,143 / 5,876 | 87.53% |
| server-filesystem | 6,615 / 7,884 | 83.90% |
| server-provider | 3,058 / 3,368 | 90.80% |
| server-terminal | 1,246 / 1,422 | 87.62% |
| server-protocol | 57 / 57 | 100.00% |
| server-domain | 121 / 121 | 100.00% |
| server-bin | 374 / 388 | 96.39% |
| 公共 Context | 47 / 47 | 100.00% |
| API 顶层 dispatch | 57 / 57 | 100.00% |
| 四个 crate 的 dispatch | 353 / 374 | 94.39% |

测量源码为基于 `3965cf5157adfee8ba05fbbde9916e8bd78609d6` 的当前工作区，包含之前的
crate capability 声明和本次公共 Context 重组。源码 SHA-256 为
`74e64b7dd4f1fb32c397d96b1eabe11b691e0f16eb0d615d502e97a95e403ee0`，测量前后相同。
哈希按路径排序，对根 Cargo.toml/Cargo.lock、crates/bins 下的 .rs 与 Cargo.toml 依次拼接
相对路径、NUL、内容、NUL。对比基线的源码哈希为
`8087d63ce04458c96fbd68402cffed6d437f180e5458b79fbac2ee7ed17a92be`，工件为
[上一版测量](server-crate-dispatch-coverage.json)。

完整 instrumented workspace 测试为 **898 passed、0 failed、5 ignored**；清理旧 server
构建对象后的 instrumented server 回归为 **407 passed、0 failed、0 ignored**。这些是分开
执行的次数，不相加计算不同测试总数。普通 workspace 的 899 项通过包含 1 项 doctest，
本次覆盖率不包含 doctest。两次完整运行均跳过既有的 5 项测试：4 项真实模型/凭据测试和
1 项外部 worker 测试。使用默认 features 和 cargo-llvm-cov 默认源码过滤，没有额外排除文件。
测量平台为 macOS arm64，未执行 Linux/Windows 平台验证。

完整测量命令：

```sh
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-session-cov-target cargo llvm-cov clean --profraw-only --offline
RUST_TEST_THREADS=1 CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-session-cov-target cargo llvm-cov --no-clean --workspace --html --offline --no-fail-fast -j1
```

第一次导出发现缓存的旧 server 二进制仍包含 14 个已删除 API 文件的零命中映射。因此保留
当前源码生成的 workspace profiles，清理并重建所有 server 包对象，再运行对应测试。
这一步没有用文件排除规则修正结果。最终导出中的所有源码路径均存在，481 个 profiles
均生成于本次源码快照之后。

```sh
cargo clean --target-dir /tmp/ait-agent-session-cov-target/llvm-cov-target -p server-api -p server-model -p server-metadata -p server-filesystem -p server-provider -p server-terminal -p server-protocol -p server-domain -p server-bin
RUST_TEST_THREADS=1 CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-session-cov-target cargo llvm-cov --no-clean --html --offline --no-fail-fast -j1 -p server-api -p server-model -p server-metadata -p server-filesystem -p server-provider -p server-terminal -p server-protocol -p server-domain -p server-bin
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-session-cov-target cargo llvm-cov report --json --summary-only --output-path /tmp/ait-model-context-coverage-raw.json
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-session-cov-target cargo llvm-cov report --html --offline
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-session-cov-target cargo llvm-cov report --show-missing-lines --offline
```

可审查工件为 [server-model-context-coverage.json](server-model-context-coverage.json)，包含
逐文件行数、聚合、命令、源码/原始导出哈希、测试结果和基线。完整本地 HTML 位于
`/tmp/ait-agent-session-cov-target/llvm-cov/html/index.html`，作为补充查看入口。

公共 Context 已全覆盖；Runtime 为 48/49，Outbound 为 62/63。主要剩余缺口是迁移后稳定
错误消息的部分枚举分支、超过队列总字节预算的单个 binary frame，以及连接处理中的部分
无效订阅、预算/激活失败、文件传输和 Terminal 错误分支。Runtime 的差额来自覆盖率聚合，
missing-lines 导出未定位到完全未覆盖的具体行。后续补充测试应优先验证订阅预算和错误响应
交互、超大 binary frame 的拒绝行为，避免仅为提高数字测试简单计数访问器。
