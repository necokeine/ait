# 独立 server：Terminal PTY 与完整方法分组

实现 [ADR-033](../decisions/adr-033-server-terminal.md)，新增 `server-terminal` crate，接通
Paseo Terminal 分组全部十个规范方法及二进制流。协议来源固定为
`getpaseo/paseo@2c8e8a826810337492cc5a38bb0bbd705b6fb632`。

## 交付行为

- 创建、列举、改名、捕获和关闭真实 PTY；按 active Workspace/Project 校验 cwd，支持字面命令参数。
- 列表与输出订阅、专用 unsubscribe、通用 subscription release；同连接重订阅替换旧订阅。
- JSON event 与 binary input/resize，连接私有 slot、resize claim/update 所有权和能力协商检查。
- 输出增量、JSON cells 快照、ANSI visible/full restore、live 模式与落后 observer 恢复；支持
  常用 ANSI、RGB/索引颜色、宽字符、鼠标和 alternate screen。屏幕、历史、输入和输出均有预算。
- 断线保留进程并允许重连；自然退出和显式 kill 排出尾部输出后发布 exit；批量关闭接入
  `agent.items.close.request`。Workspace/Project 归档或移除后清理 PTY，宿主 shutdown 回收进程。

`server-terminal` 只依赖 `server-metadata` 的 registry 端口；协议、服务、PTY 端口、本机 adapter
和屏幕解析留在能力包内。Tokio、连接状态和 backpressure 留在 `server-api`；宿主在
`server-bin` 组装。Provider、旧 daemon 和领域 Message/Session/Run 没有新增 Terminal 依赖。

本轮自身增加十个已实现方法；工作区同时存在 ADR-034 的五个 Agent/Session 方法，因此完整
catalog 的已实现数量为 122/195，而非将并行修改归入本轮。覆盖率也测量这一共同工作区。

## 验证

Terminal 的独立子文件单元测试与本机 PTY 测试共 17 项，覆盖参数/帧验证、placement、registry
失败、资源上限、resize 所有权、UTF-8 分片、颜色、鼠标、全屏/主屏恢复、输出溢出、capture
负索引、关闭后排尾以及 shell 已退出时后台进程组回收。三个真实 server/WebSocket 场景覆盖十个方法、能力协商、slot 跨连接隔离、
列表变更、重订阅/释放、断线恢复、自然退出、批量关闭、归档清理和 shutdown PID 回收。
依赖守卫检查 Terminal 没有向 HTTP、Tokio、SQL、Provider 或旧 Ait crate 反向依赖。

最终定向回归 `server-terminal`、`server-api`、`server-bin`：78 passed、0 failed、0 ignored。
普通 workspace 回归为 879 passed、0 failed、5 ignored，另 1 项 doctest 通过。该轮构建早于
最后的后台进程组回收修复；修复后由上述 78 项定向回归和最终完整覆盖率回归验证。
`cargo fmt --all --check`、`git diff --check`、workspace Clippy `-D warnings` 通过。
本轮没有真实模型调用或付费 API 请求。

## Test coverage

最终覆盖率回归：71 个测试 target，880 passed、0 failed、5 ignored。

| 范围 | 已覆盖 / 总行数 | 行覆盖率 | 相对上一轮 |
| --- | --- | --- | --- |
| Workspace | 40946 / 50205 | 81.5576% | +0.3698 个百分点 |
| server-terminal | 855 / 978 | 87.4233% | 新包，无同包基线 |
| server-api | 2293 / 2451 | 93.5537% | -0.7160 个百分点 |
| server-protocol | 122 / 145 | 84.1379% | -3.8320 个百分点 |
| server-bin | 374 / 388 | 96.3918% | +0.0471 个百分点 |

八个 server package 合计 17519/20074（87.2721%）。可审查工件：
[逐文件覆盖率、源码指纹、Paseo 来源与测试记录](server-terminal-coverage.json)。
基线为 [上轮原生 Provider 覆盖率](server-native-provider-coverage.json)：workspace
39096/48155（81.1878%）。Workspace 与共用 API 的变化包含并行 Agent/Session 工作，不能全部
归因于 Terminal；新包没有同包基线。

测量 revision 为 `e9261b8b5a74b39eebaaef1983495f1892e8d389` 加本轮 Terminal 与并行 ADR-034
未提交修改；Rust/manifest SHA-256 为
`3f8b432108a20ee07d1e7d8c7e1d30cf7a7da4082bcdc7651b0ac18a66413a02`。
使用 workspace 默认 features、cargo-llvm-cov 默认测试/build-source 过滤，无额外文件排除；
覆盖率不包含 doctest。平台为 macOS arm64，rustc 1.98.1、cargo-llvm-cov 0.8.4；Linux/Windows
未执行。HTML 在 `/tmp/ait-phase10-cov-target/llvm-cov/html/index.html`，共享审查使用上述 JSON 工件。

```sh
cargo fmt --all --check
git diff --check
CARGO_TARGET_DIR=/tmp/ait-terminal-target cargo clippy --workspace --all-targets --offline -- -D warnings
CARGO_TARGET_DIR=/tmp/ait-phase10-workspace-target cargo test --workspace --offline --no-fail-fast -j2 -- --test-threads=1
CARGO_TARGET_DIR=/tmp/ait-terminal-target cargo test -p server-terminal -p server-api -p server-bin --offline -- --test-threads=1
RUST_TEST_THREADS=2 CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-phase10-cov-target cargo llvm-cov --workspace --html --offline --no-fail-fast -j1
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-phase10-cov-target cargo llvm-cov report --json --summary-only --output-path /tmp/ait-terminal-coverage-raw.json
```

此前覆盖率尝试为 878 passed、1 failed、5 ignored：旧 Daemon 的
`macos_gui_path_reaches_codex_in_the_worker` 在 discover-models HTTP 请求中超时；普通 workspace
运行同一用例通过。随后完成最后的 Terminal 后台进程组修复，并清空旧 profiles 后重新执行
整个 workspace；本报告使用重跑结果，不合并修复前后的 profiles。
该 Daemon 用例在最终测量中通过。5 个忽略项沿用基线：真实 Codex/Python、DeepSeek 实时
catalog、WF10 Codex、WF11 DeepSeek，以及需要外部 Worker 的权限 replay，没有新增忽略项。

仍未覆盖部分 PTY 分配、线程启动、读写/kill 系统调用失败、锁中毒恢复，以及 API 队列耗尽、
序列化与关闭竞争的错误分支。后续应通过 native adapter 故障注入及 Linux/Windows CI 补齐；
本轮已经用真实本机 PTY 与 WebSocket 验证正常行为和主要生命周期边界。

## 限制与后续

十个 Terminal 方法均已实现；这不等于整个 Paseo xterm/automation 生态完整移植。`vt100` 的
扩展键盘、部分样式、光标样式、wrap/reflow 与 xterm 仍有差异。`activity` 为 null，未安装
shell/Agent activity hook 或 HTTP token route。setup/script executor 的逻辑 terminalId 尚不
对应本包的真实 PTY；自动 profile、proxy/health 与 teardown 保持既有边界。

终端仅在本次 server 实例内保留，服务重启不恢复 shell。自然退出的 capture 保留至淘汰；显式
kill 后 capture 为空，原 observer 仍可排尾。Unix 清理已知 child/foreground process group，
不承诺回收自行脱离 session 的守护进程。Windows 使用 portable-pty ConPTY，尚未实机验证。
完整参数和运行方式见[操作说明](../operations/independent-server.md#terminal-pty)。
