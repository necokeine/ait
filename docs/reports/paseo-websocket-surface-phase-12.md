# Paseo WebSocket 接口移植：第十二阶段

- 日期：2026-09-23；分支：`new`；基线：`2918635`。
- Paseo 固定来源：`2c8e8a826810337492cc5a38bb0bbd705b6fb632`。
- 决策：[ADR-026](../decisions/adr-026-canonical-paseo-websocket-surface.md)。

## 全部方法的占位入口

固定 catalog 中的 191 个 Paseo 入站名称映射到 188 个唯一规范方法：178 个 request、9 个客户端 event、
1 个客户端 response。生产 binary 原有 93 个 Paseo 方法及 11 个独立方法保持真实实现；其余 95 个 Paseo
方法现在都可通过 hello 协商，并统一返回 `not_implemented`、`retryable:false`。因此 `info.capabilities`
公开 199 个唯一名称，`info.implemented_capabilities` 明确指出其中 104 个已有真实行为。后者在缺少
某个应用服务的测试 host 中按实际组装收缩；占位方法始终保留在 catalog 中。

Request 沿用 `type=request`、`request_id`、规范 method、`params`。9 个客户端通知使用 `type=event`，
浏览器执行结果回传使用 `type=response`，可以带原 server request ID。客户端 response 目前没有相应
服务端发起操作，故仍只得到明确的占位错误；event 错误无 request ID。方法的方向用 catalog 校验，
把 event 发成 request 或把 request 发成 event 返回 `invalid_message`。未知规范名称及原版下划线、斜杠
名称仍返回 `method_not_found`；未协商的已登记方法返回 `unsupported_capability`。这些请求错误保持连接
可用，不写入业务状态。

Hello 的 optional 和 required 列表仍各最多 64 个，所以客户端应按连接需要声明接口；
`server_info.info.capabilities` 中的 199 个名称不要求客户端一次全量协商。原有二进制文件帧继续受
`file.upload.request` 协商保护。

## 分发层

WebSocket 连接层只处理握手、消息方向、协商、实现状态、文件流入口和响应发送。其余真实请求在
`server-api::connection::routing` 的单一方法分组表中分派；处理器内部仍持有各自的 DTO 与业务逻辑。
连接层先返回占位错误，避免把未实现方法送到某个真实处理器产生误导性成功结果。原有标签、Diff 订阅
在 response 入队后激活，Workspace update 仍在相应 response 之后发出；文件订阅和上传继续绑定物理
连接。原有独立 `project.*`、`agent.*` 方法以及旧 status 取消入口也纳入入口识别，不因 Paseo catalog
过滤而丢失。

## 实现边界

这 95 个方法只有规范名称、消息方向、协商与明确错误；没有 Paseo 的方法专属 DTO、参数校验、业务
副作用或对应业务单元测试。后续每移植一个方法，就在其独立 server crate 内加入完整 DTO、use case、
adapter 与测试，并把它加入真实模块的 `CAPABILITIES`；生产 `implemented_capabilities` 随之自动增长。
占位错误不表示原版 Paseo 也会返回同样错误。客户端应该只针对
`implemented_capabilities` 中的方法依赖业务结果。

## Test coverage

普通 workspace 回归运行了 72 个测试 target 与 24 个 doc-test target：794 项通过、1 项首次失败、5 项按原
标记忽略。失败是未修改的 `ait-tools` 测试
`cancels_descendants_before_drain_returns_and_cleans_up_after_normal_exit`，断言临时目录中子进程的
残留文件不存在；使用同一已构建测试 binary 单独复跑 1 项通过。这个首次失败不归因于本次 server 修改，
也不被写成一次全绿。生产 server 的全部 95 个占位方法及原有真实业务用例已通过定向测试；早期定向
测试发现独立 Project/Agent 方法在 catalog 分类时被漏掉，已修正并复跑全部受影响 target。

完整 workspace 插桩回归运行 72 个普通 target：**794 通过、0 失败、5 忽略**；这些测试不包含普通回归另行
通过的 1 个 doc test。独立 server 303 项全部通过，比第十一阶段新增 6 项。整仓默认 features 的严格
clippy、3 个改动 server package 的 all-features 严格 clippy，以及 Rust 格式检查通过。

测量基线为 `2918635` 加本阶段变更，平台为 macOS 26.6.2 / arm64、rustc 1.98.1、
cargo-llvm-cov 0.8.4；范围为整个 Cargo workspace 的默认 features、232 个生产 Rust 文件。工具默认
过滤测试与 build script 文件，没有额外文件排除；Linux 与 Windows 未运行。

| 范围 | 覆盖 / 总行数 | 行覆盖率 |
| --- | --- | --- |
| 整个 workspace | 37,688 / 46,682 | **80.73%** |
| 独立 server 的 8 个 package | 14,262 / 16,551 | **86.17%** |
| 本阶段改动的可执行源文件合计 | 878 / 938 | **93.60%** |
| `server-api` | 4,255 / 4,913 | 86.61% |
| `server-protocol` | 623 / 744 | 83.74% |
| `server-bin` | 363 / 376 | 96.54% |
| 新 routing 模块 | 168 / 190 | 88.42% |

整仓与第十一阶段同口径的 37,641 / 46,634（80.72%）相比提高 **0.02 个百分点**；独立 server 与
14,213 / 16,503（86.12%）相比提高 **0.05 个百分点**。“改动的可执行源文件”包含新旧代码，不是
仅由本阶段新增行构成的覆盖率。

执行命令：

```sh
CARGO_TARGET_DIR=/tmp/ait-phase10-workspace-target cargo test --workspace --no-fail-fast -j1
# 从 crates/tools 目录复跑首次失败的已构建测试 binary：
/tmp/ait-phase10-workspace-target/debug/deps/shell_permissions-9439bfd7ee9ac34a cancels_descendants_before_drain_returns_and_cleans_up_after_normal_exit --exact --test-threads=1
CARGO_TARGET_DIR=/tmp/ait-phase10-workspace-target cargo clippy --workspace --all-targets -- -D warnings
CARGO_TARGET_DIR=/tmp/ait-phase10-workspace-target cargo clippy -p server-protocol -p server-api -p server-bin --all-targets --all-features -- -D warnings
cargo fmt --all -- --check
CARGO_TARGET_DIR=/tmp/ait-phase10-cov-target cargo llvm-cov --workspace --json --summary-only --output-path /tmp/paseo-ws-phase12-workspace-coverage-raw.json --no-fail-fast -j1
CARGO_TARGET_DIR=/tmp/ait-phase10-cov-target cargo llvm-cov report --html
git diff --check
```

可评审的 [coverage artifact](paseo-websocket-surface-phase-12-coverage.json) 随提交保存原始行数、各 crate
与路由源文件统计、基线、命令、跳过原因及测试结果。完整本机 HTML 位于
`/tmp/ait-phase10-cov-target/llvm-cov/html/index.html`，原始 JSON 位于
`/tmp/paseo-ws-phase12-workspace-coverage-raw.json`。

已查看未覆盖行：新 routing 模块剩余部分订阅失败、资源耗尽及防御性 fallback 分支；连接层仍有异常
binary 帧与 socket 写失败分支未覆盖。后续可通过故障注入和队列压力测试补足。95 个占位方法的专属 DTO、
Paseo 业务行为和原版对应单元测试仍须随真实实现逐项补齐；当前覆盖率不代表这些行为或其他平台已验收。
