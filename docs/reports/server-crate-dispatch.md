# 独立 server：crate 优先的请求分发

实现 [ADR-036](../decisions/adr-036-server-crate-dispatch.md)，继续整理上一轮能力分组重构。

## 变更

请求链路为：API 协商/方向检查 → 选择 crate → crate 的 dispatcher → Host 调度或连接处理 →
crate 内具体 RPC/service。统一请求分发只有 metadata、filesystem、provider、terminal 四个分支。
各 crate 提供不依赖传输类型的 Host 端口；普通 RPC 的具体函数由所属 crate 选择。

API 删除 agents、directory、forge、github_projects、workspace_automation、workspace_state、
workspace_recovery、worktrees 八个转发文件。普通 RPC 共用请求移动、任务调度、错误转换与
响应发送；有实际生命周期职责的辅助模块继续保留。

Session、文件、Terminal 和 Agent 完成长等待现在都先进入对应 crate，再接入其宿主处理。
Worktree 后续 setup 和 Agent/Terminal 批量关闭仍由宿主协调。普通响应后动作由明确的枚举
表达，删除 RouteResult 中内嵌错误和多个独立可空字段，保持先入队响应、再激活订阅/发送事件。

不新增 crate 依赖、Tokio/HTTP 反向依赖、boxed future 或动态路由注册。能力名称和安装规则
延续上一轮的 122/195；事件与二进制帧继续使用既有连接入口。

## 验证

新增 9 项专项测试已通过，覆盖四个 crate 的分组/错误传播、provider 长等待选择、公共 RPC
入参与错误映射、Workspace/Status 响应顺序和可选事件投影。
独立 server 六个 package 的完整回归已通过：391 passed、0 failed、0 ignored，包含 WebSocket、
真实进程、订阅和 Terminal 测试。完整 workspace 回归为 900 passed、0 failed、5 ignored，
其中包含 1 项 doctest；覆盖率运行单独为 899 passed、0 failed、5 ignored，不包括 doctest。
这些数字是各次运行的结果，不能相加作为不同测试的总数。

格式和全工作区 Clippy `-D warnings` 已通过。Clippy 指出的三个无等待回调已改为
`std::future::ready`，同时修正测试中的 unit pattern；没有通过新增 lint allow 规避诊断。

最初使用 `/tmp/ait-phase10-workspace-target` 的普通 workspace 构建未进入测试：rustc 长时间
停留在过程宏动态库的 `dlopen` / `dyld::Loader::mapSegments` / `fcntl` 调用，没有编译诊断。
终止该构建后，改用已通过 server 回归的 `/tmp/ait-agent-session-target` 重新执行完整 workspace。
没有更改系统安全设置、编译器或业务代码来绕过这个本机加载问题。

成功执行的验证命令：

```sh
cargo fmt --all --check
git diff --check
CARGO_TARGET_DIR=/tmp/ait-agent-session-target cargo clippy --workspace --all-targets --offline -- -D warnings
CARGO_TARGET_DIR=/tmp/ait-agent-session-target cargo test -p server-api -p server-bin -p server-metadata -p server-filesystem -p server-provider -p server-terminal --offline --no-fail-fast -- --test-threads=1
CARGO_TARGET_DIR=/tmp/ait-agent-session-target cargo test --workspace --no-fail-fast --offline -- --test-threads=1
```

## Test coverage

workspace 实测行覆盖率 **81.56%（40,918 / 50,169）**。
可审查工件为 [server-crate-dispatch-coverage.json](server-crate-dispatch-coverage.json)，包含逐文件
行数、各 package 汇总、源码校验值、原始导出校验值、验证命令、忽略测试及直接基线。

对照上一轮 [capability groups 实测](server-capability-groups-coverage.json)，workspace 从
81.5389%（40,895 / 50,154）升至 81.5603%，增加 **0.0215 个百分点**。
相关 crate 和新增分发模块如下；百分点差值只对已有可比统计范围计算。

| 范围 | 已覆盖 / 总行数 | 行覆盖率 | 相比上一轮 |
| --- | ---: | ---: | ---: |
| server-api | 2,160 / 2,315 | 93.30% | +0.0337 个百分点 |
| server-metadata | 4,830 / 5,543 | 87.14% | +0.1073 个百分点 |
| server-filesystem | 6,084 / 7,311 | 83.22% | +0.0460 个百分点 |
| server-provider | 2,929 / 3,222 | 90.91% | +0.0226 个百分点 |
| server-terminal | 870 / 993 | 87.61% | +0.0627 个百分点 |
| 四个 crate 的新 dispatch 模块 | 48 / 48 | 100.00% | 新模块，无直接基线 |
| 新 dispatch 模块及 API 宿主适配 | 376 / 402 | 93.53% | 新统计范围，无直接基线 |

本次测量 revision 为 `3965cf5157adfee8ba05fbbde9916e8bd78609d6` 加工作区中的能力分组与
本次请求分发修改。源码 SHA-256 为
`8087d63ce04458c96fbd68402cffed6d437f180e5458b79fbac2ee7ed17a92be`；测量前后相同。
校验范围为根 `Cargo.toml`/`Cargo.lock` 与 `crates`、`bins` 内所有 Rust 源码和 Cargo 清单，
按相对路径排序，连接“路径 + NUL + 内容 + NUL”。基线源码校验值为
`3e5cffb53b628cdf8c4d40a553be37f36ceef844249d07bfb125ca460fd0ea77`。

测量命令：

```sh
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-session-cov-target cargo llvm-cov clean --profraw-only --offline
RUST_TEST_THREADS=1 CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-session-cov-target cargo llvm-cov --no-clean --workspace --html --offline --no-fail-fast -j1
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-agent-session-cov-target cargo llvm-cov report --json --summary-only --output-path /tmp/ait-crate-dispatch-coverage-raw.json
```

范围为完整 workspace、默认 features、macOS arm64，以及 cargo-llvm-cov 默认源文件过滤；
没有增加文件排除规则，没有测量 doctest，也未验证 Linux/Windows。测量前清除旧 profiles，
本次 433 个 profiles 均晚于源码快照；已删除的八个 API 转发文件未出现在导出中。
HTML 位于 `/tmp/ait-agent-session-cov-target/llvm-cov/html/index.html`，仓库中的 JSON 为可共享工件。

5 个既有忽略项为 Codex/DeepSeek 真实模型测试（需要凭据并会调用外部模型）及需要
`AIT_TEST_WORKER_EXECUTABLE` 的外部 worker 权限重放测试；具体测试名记录在 JSON 中。

新公开分发函数均有覆盖。API 宿主适配仍未覆盖 Labels 订阅额度耗尽、非法 release ID、
Checkout 取消订阅的非法/不存在 ID、Checkout 订阅额度耗尽，以及 base 的防御性未知方法
分支。后续应优先补充订阅资源边界测试；不绕过入口校验来人为触发不可达的未知方法分支。
workspace 中其他既有 provider/tool 未覆盖路径仍然存在，本次测量不代表这些路径已验证。
