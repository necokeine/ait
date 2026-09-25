# 独立 server：Codex 原生文本执行

实现 [ADR-032](../decisions/adr-032-server-native-provider-execution.md)，在现有七个 server crate 内接通
Agent 创建、恢复、纯文本发送、取消与等待结果；已实现 capability 从 102 增至 107。
沿用固定 Paseo commit `2c8e8a826810337492cc5a38bb0bbd705b6fb632`，并核对本机
`codex-cli 0.153.4` 通过 `codex app-server generate-json-schema --out /tmp/ait-server-codex-schema`
导出的 initialize、thread/start/resume/read、turn/start/interrupt 与终态通知 schema。

## 实现与边界

- `server-provider` 拥有协议、解码、原生 Codex adapter、会话/turn 生命周期和独立 worker。
  原生创建与恢复取得 thread ID 后落盘；归档恢复仅 thread/read，不启动交互式 writer。
- `server-api` 继续拥有 WS 接纳、wait 并发预算、响应与 shutdown；finish.wait 期间同一连接可以
  发送 cancel，响应通过 request_id 关联。断线不取消已接纳的 native turn。
- Manager 合并最新 durable record，避免恢复、完成或关闭覆盖并发元数据；terminal 落盘失败保留
  pending event 重试。archive/delete 关闭相关 live writer，删除不会被 close 重新插入。
- 默认从 PATH 找 codex；可通过 AIT_SERVER_CODEX_BIN 覆盖。JSON/RPC/命令队列均有预算；Unix
  关闭原生进程组并 wait，worker 保持实例 lease。未复用旧 Ait provider、Run 或 Session 实现。

第一片只支持 read-only Codex 和已有活动 Workspace；未指定 workspaceId 时按 cwd 复用。
附件、权限、MCP、动态配置、模型发现、排队/steer、幂等重放、原生历史导入和 timeline 仍未接通，
未支持的参数明确拒绝。lastMessage 是当前进程最近完成 turn 的最后文本，不是持久化消息时间线。
宿主 15 秒 shutdown deadline、Windows 进程树与强制退出后的原生对账限制详见 ADR。

## 验证

新增离线协议 peer 和回归覆盖创建/恢复/read-only history、发送/忙时拒绝、取消及 completion 竞争、
进程退出、RPC 超时/错 ID/畸形/超大帧、权限请求拒绝、terminal 写失败重试、元数据合并、live 上限、
同连接 wait/cancel、wait 预算与断线释放、重启恢复、archive/delete、直接子进程及工具进程组回收。
所有新执行测试均使用本地模拟进程，没有调用真实模型或外部付费 API。

完整 workspace 普通回归：846 passed、0 failed、5 ignored；另 1 项 doctest 通过（70 个普通测试 target，
23 个 doctest target）。最终源码的 provider/API/binary 专项：116 passed、0 failed、0 ignored。
`cargo fmt --all --check`、`git diff --check`、workspace Clippy `-D warnings` 均通过。
最后的标题 UTF-16 边界修正和 wait 释放更新由最终专项回归及完整覆盖率测试重新验证。

## Test coverage

| 范围 | 已覆盖 / 总行数 | 行覆盖率 | 相对上一轮 |
| --- | --- | --- | --- |
| Workspace | 39096 / 48155 | 81.1878% | +0.3066 个百分点 |
| 七个 server package | 15669 / 18024 | 86.9341% | +0.4611 个百分点 |
| server-provider | 2761 / 3045 | 90.6732% | +2.2919 个百分点 |
| server-api | 1678 / 1780 | 94.2697% | -0.6978 个百分点 |
| server-bin | 369 / 383 | 96.3446% | -0.2365 个百分点 |

可审查工件：[逐文件覆盖率、命令、原始尝试与补跑结果](server-native-provider-coverage.json)。
基线：[server-provider 拆分工件](server-provider-coverage.json)，workspace 38015/47001（80.8813%）。
当前 revision 为 `00c8e82ef33c81cc092a2babd8789d8c2d39e630` 加前序拆分与本轮未提交修改；
最终 Rust/manifest SHA-256：`542d22905a00e5e8576d5ab2a3515286360f1950a30b8fa0c89620f1ce075e2a`。
协议 peer 和固定 Paseo 源文件的 SHA-256 也保存在工件中。

```sh
cargo fmt --all --check
git diff --check
CARGO_TARGET_DIR=/tmp/ait-phase10-workspace-target cargo clippy --workspace --all-targets --offline -- -D warnings
CARGO_TARGET_DIR=/tmp/ait-phase10-workspace-target cargo test --workspace --no-fail-fast --offline -- --test-threads=1
CARGO_TARGET_DIR=/tmp/ait-phase10-workspace-target cargo test -p server-provider -p server-api -p server-bin --offline --no-fail-fast -- --test-threads=1
CARGO_TARGET_DIR=/tmp/ait-phase10-workspace-target cargo test -p server-provider --offline -- --test-threads=1
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-phase10-cov-target cargo llvm-cov clean --workspace --offline
RUST_TEST_THREADS=1 CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-phase10-cov-target cargo llvm-cov --workspace --html --offline --no-fail-fast -j1
RUST_TEST_THREADS=1 CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-phase10-cov-target cargo llvm-cov --no-clean -p server-provider --html --offline --no-fail-fast -j1
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-phase10-cov-target cargo llvm-cov report --html
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-phase10-cov-target cargo llvm-cov report --json --summary-only --output-path /tmp/server-execution-coverage-raw.json
```

测量使用整个 workspace 的默认 features、cargo-llvm-cov 默认测试/build-source 过滤，无额外文件排除；
覆盖率不包含 doctest。环境为 macOS arm64、rustc 1.98.1、cargo-llvm-cov 0.8.4；Linux/Windows 未执行。
HTML 位于 `/tmp/ait-phase10-cov-target/llvm-cov/html/index.html`，共享审查使用上面的 JSON 工件。

最终生产源码的完整覆盖率尝试为 845 passed、1 failed、5 ignored：一个模拟 RPC error 的用例共用了
300ms deadline，在插桩运行下先超时。修正仅涉及测试：只有专门的 timeout 用例保留 300ms，其他
故障用例使用正常 10 秒预算。随后 Provider 普通测试与插桩测试均为 62 passed、0 failed；按每个
测试 target 的最终结果汇总，70 个 target 共 846 passed、0 failed、5 ignored。补跑前后所有
258 个文件的生产行数映射完全一致；没有合并不同生产源码布局的 profiles。

5 个忽略项均沿用基线：真实 Codex/Python、DeepSeek 实时 catalog、WF10 Codex、WF11 DeepSeek，
以及需外部 worker 的权限 replay。没有新增被忽略的原生执行测试。

尚未覆盖真实认证与付费模型调用、部分线程/runtime 创建或关闭通道失败、部分 native kill/write 与
registry I/O 故障，以及 Windows 进程树语义。新 adapter 已通过离线 stdio 与真实 WS/进程生命周期
验证；真实模型认证、原生历史对账和其他 Provider 应在对应能力接入后另做集成验证。
