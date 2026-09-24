# Paseo WebSocket 接口移植：GitHub 仓库发现与 Project 克隆

- 日期：2026-09-23；分支：`new`；基线：`d60e73c`。
- Paseo 固定来源：`2c8e8a826810337492cc5a38bb0bbd705b6fb632`。
- 决策：[ADR-028](../decisions/adr-028-github-project-provisioning.md)。

本次把 `workspace.github.search_repositories.request` 与 `project.github.clone.request` 从
`not_implemented` 接通为真实业务。前者在空查询时运行 `gh repo list`，非空查询时运行
`gh search repos`，根据 `gh` 配置输出 HTTPS 或 SSH clone URL；成功、缺少 CLI、未认证和
其他错误分别返回 Paseo 的状态与可用性字段。后者验证 repository，创建目标父目录，在临时目录
完成 Git clone 后发布 checkout，并将其注册为 Project；不会顺带创建 Workspace。目标已存在、
克隆失败和注册失败都返回内联错误，且能解析时保留 `checkoutPath`。真实 binary 的 WebSocket
用例以本地 Git 仓库和假 `gh` 脚本验证了搜索、未认证、克隆、Project 注册和冲突响应，不访问网络。

新功能只使用独立的 `server-protocol`、`server-ports`、`server-application`、
`server-workspace`、`server-api`、`server-bin`，不依赖旧 Ait 服务。`DirectoryDependencies`
集中组装这些 port；WS 路由树仍按规范 dotted 名称分发。此后 Paseo 188 个规范方法中已有
95 个真实实现，93 个仍为明确的 `not_implemented`；加上 11 个独立方法，生产 binary 的
`implemented_capabilities` 应为 106。此统计按 [第十二阶段报告](paseo-websocket-surface-phase-12.md)
的 93/95 基线，加本次 2 个方法计算，不代表其余占位已有 DTO 或业务行为。

与 Paseo 的差异：独立入口只接受 `github.com` HTTPS/SSH 或需显式 `cloneProtocol` 的
`owner/repo`，不接受原版 remote parser 可识别的其他 Forge host；对 URL 的 userinfo、query、
fragment、非默认端口及非 ASCII repository 片段有更严格的拒绝；CLI 错误返回固定安全文本，
不转发原版部分场景会带出的 stderr。克隆仍使用普通文件系统 rename；与其他进程同时创建目标
路径的竞态尚无跨进程原子 no-replace 保证。长达五分钟的克隆占用现有全局 business job lane，
后续应给长任务专用的受监督队列。其余 93 个占位接口未在本次获得业务实现；下一组应优先接通
AgentManager/AgentSession 到 agent 生命周期 WebSocket 方法及 Provider 查询，才能把已有
独立生命周期基础设施变成可用执行入口。

## Test coverage

测量范围为 Cargo workspace 默认 features 的 234 个生产 Rust 文件；`cargo-llvm-cov` 默认过滤
测试和 build script 源文件，无额外文件排除。平台为 macOS 26.6.2 / arm64、rustc 1.98.1、
cargo-llvm-cov 0.8.4；Linux 和 Windows 未运行。覆盖率运行的 72 个普通测试 target
**819 通过、0 失败、5 忽略**，不包含 doc tests。

| 范围 | 覆盖 / 总行数 | 行覆盖率 |
| --- | --- | --- |
| 整个 workspace | 38,382 / 47,438 | **80.91%** |
| 独立 server 的 8 个 package | 14,954 / 17,307 | **86.40%** |
| `server-application` | 3,047 / 3,463 | 87.99% |
| `server-api` | 4,402 / 5,072 | 86.79% |
| `server-workspace` | 4,546 / 5,540 | 82.06% |
| 新 GitHub adapter | 214 / 245 | 87.35% |

与上一份同口径的 [基线](independent-agent-session-manager-coverage.json)相比，整仓由
37,995 / 47,012（80.82%）提高约 **0.09 个百分点**，独立 server 由
14,567 / 16,881（86.29%）提高约 **0.11 个百分点**。crate 数字包含既有代码，
不表示本次新增行单独的覆盖率。

首次普通整仓回归的 72 个普通 target 为 816 通过、3 失败、5 忽略：两项 catalog/route 测试
仍写死旧的 104/95 方法计数，GitHub 克隆集成测试发现 macOS `/var` 与 `/private/var` 的响应
路径表示不一致。三处均修正；随后路由清单定向测试 1 项通过，真实 server 进程 target
23 项全部通过，最终上述整仓插桩回归 819 项全部通过。普通整仓命令进入第一个 rustdoc
target 后进程长时间无进展，已中止；本次未取得完整 doc-test 结果，未把它计入 819 项。
严格 clippy、整仓 build 和格式检查另行通过。

执行命令：

```sh
cargo test --workspace --no-fail-fast -j1 -- --test-threads=1
CARGO_INCREMENTAL=0 cargo test -p server-api --lib connection::routing::tests::hierarchy_routes_every_implemented_method_to_exactly_one_handler -- --exact
CARGO_INCREMENTAL=0 cargo test -p server-bin --test process -- --test-threads=1
RUST_TEST_THREADS=1 CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-phase10-cov-target cargo llvm-cov --workspace --json --summary-only --output-path /tmp/ait-github-projects-coverage-raw.json --no-fail-fast -j1
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-phase10-cov-target cargo llvm-cov report --html
CARGO_INCREMENTAL=0 cargo build --workspace -j1
CARGO_INCREMENTAL=0 cargo clippy --workspace --all-targets -j1 -- -D warnings
cargo fmt --all -- --check
git diff --check
```

可评审的 [coverage artifact](paseo-github-project-provisioning-coverage.json) 保存行数、
各范围、测试结果、基线和测量命令；本机 HTML 位于
`/tmp/ait-phase10-cov-target/llvm-cov/html/index.html`。未覆盖的关键分支包括 `gh` 或
Git 输出超限与超时、目标目录权限错误、跨进程同时占用 clone 目标的竞态；需要故障注入和
并发测试。Paseo 的实时 GitHub 登录与网络克隆未运行，使用假 CLI 和本地 Git 替代。
