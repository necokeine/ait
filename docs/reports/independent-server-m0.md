# 独立 server M0 实现与验证

- 日期：2026-09-22（Asia/Shanghai）。
- 分支：`new`。
- 基线：`77ab2f5ffab094cd8ee0f7ef2c6a5690584b6575`；本报告对应其上的 M0 工作区改动。
- 设计：[ADR-022](../decisions/adr-022-independent-server.md)。
- 使用：[启动、HTTP/WS 协议和预算](../operations/independent-server.md)。

## 实现

| 新 package | 实现职责 | 内部生产依赖 |
| --- | --- | --- |
| `server-bin`，产物 `server` | CLI/环境/TOML 配置、日志、目录锁、身份、信号与主进程关闭 | `server-api` |
| `server-api` | Axum、凭据/来源检查、握手、连接身份、RPC、临时状态订阅、背压与 drain | `server-protocol` |
| `server-protocol` | 独立 wire DTO、v1.0/capability 协商、稳定错误、预算 | 无 |

新 server 未依赖或调用任何旧 Ait component。原 daemon/worker 和旧业务库的 Rust 源码未改动。
根 workspace 自动纳入三个新 package；`Cargo.lock` 记录新增第三方依赖与 feature 解析。
没有创建未被使用的 domain、ports、application 或 adapter 空 crate。

稳定 `server_id` 与每次启动的 `instance_id` 分离，身份文件使用临时文件、sync 和原子发布。
OS 文件锁在正常 HTTP/WS drain 结束后释放，同一路径的规范化别名也不能重复接管。初始化前
先绑定端口，绑定失败不创建 server 状态文件。所有测试数据位于临时目录。

每次 WS upgrade 在接纳锁内登记 TaskTracker token，再等待 HTTP upgrade；关闭接纳与关闭
tracker 串行，避免遗漏正在升级的连接。reader/writer 在同一受跟踪 future 内 join，写入、
最后 flush/close 和整个进程 drain 都有独立上限。队列字节许可持续占用到 socket write 结束。

## 验证范围

- 配置默认值与 CLI > env > TOML 优先级、远程地址拒绝、缺失/弱凭据拒绝、错误日志脱敏。
- 同 data-dir 排他、路径别名、稳定身份/新实例、损坏身份拒绝、状态文件符号链接拒绝、
  启动失败释放资源、端口失败不初始化、真实子进程 SIGTERM 和重启。
- health/info、凭据、Host/Origin、URL query 拒绝、版本区间与 required capability。
- 同 client ID 的不同物理连接、request ID 回传、未知业务方法、错误参数、订阅归属与限额。
- JSON/二进制/重复 hello 拒绝、hello 超时、单帧与分片累计超限、连接限额、待 hello 连接关闭。
- 发送消息数/字节双预算、在途写入占用字节预算、资源释放、draining 状态与持续连接关闭。
- 依赖声明守卫，覆盖 optional、平台条件、dev/build 与 rename；未来新 crate 的依赖方向也有白名单。

## Test coverage

行覆盖率来自本次 `cargo llvm-cov` 实测，测试通过数量另列，二者不互相替代。

| 测量范围 | 已覆盖 / 总行数 | 行覆盖率 |
| --- | --- | --- |
| Workspace | 24,059 / 30,803 | 78.11% |
| `server-bin` | 229 / 242 | 94.63% |
| `server-api` | 375 / 381 | 98.43% |
| `server-protocol` | 49 / 49 | 100.00% |
| 三个新 crate 合计 | 653 / 672 | 97.17% |

Workspace 尚未达到工程规范建议的 80%，新 crate 的合计覆盖率为 97.17%。没有本次改动前、
相同工具链及测试范围的可比覆盖率基线，因此不计算提升值。

可评审产物：[覆盖率 JSON 摘要](independent-server-m0-coverage.json)，包含 workspace/new crate
统计、全部 155 个生产源码文件的行计数、命令、跳过项和本批代码的 SHA-256。
详细本机 HTML 位于 `target/llvm-cov/html/index.html`；JSON 随改动交付，报告不只依赖本机路径。

代码快照：基线 commit 加 `new` 工作区；指纹为
`a19503e46a3fb5f95064e4a8076e57761a85f990a9916ba30d0ffce1db65bc8f`。
指纹覆盖根 Cargo manifest/lock 与三个新 package 的全部文件；逐文件 hash 见 JSON。
平台为 macOS 26.6.2 / Darwin 25.6、arm64；工具链为 Rust 1.98.1、cargo-llvm-cov 0.8.4。
使用 workspace 默认 features，未传入 `--features` / `--all-features`。

执行与导出命令：

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo llvm-cov --workspace --html -- --skip command_approval_secrets_never_reach_durable_or_reconnected_views
cargo llvm-cov report --json --summary-only --output-path /tmp/ait-server-m0-coverage-summary.json
```

测试执行结果：格式、Clippy 均通过；新增三个 crate 的 28 个用例通过。
普通 `cargo test --workspace`：520 通过（519 个普通测试与 1 个 doc test）、0 失败、5 ignored，
包含前述审批用例；66 个普通测试 target 与 19 个 doc-test 组均完成。
覆盖率运行完成 66 个测试 target：518 通过、0 失败、5 ignored、1 filtered out。

覆盖率专用排除项为既有测试
`command_approval_secrets_never_reach_durable_or_reconnected_views`：它在普通 workspace 测试中
通过，但在插桩运行、单线程复跑和隔离复跑中反复超过 `pending_approval` 的 3 秒等待。
未修改旧业务实现或该测试的超时。该项排除仅影响覆盖率命令，普通完整回归包含它。
没有主动排除生产源码文件；默认报告不含测试源码或 build script，未编译的其他平台 cfg
分支也不在分母内。Doc tests 由普通 workspace 测试执行，本次稳定工具链的覆盖率命令未插桩它们。

保留既有 5 个 ignored 测试：

- `codex_native_tools_create_and_verify_python_hello_world`
- `deepseek_live_default_catalog`
- `wf11_real_deepseek_python_hello_world`
- `wf10_create_project_with_real_codex_and_commit`
- `permission_change::replay_permission_change_with_external_worker`

新代码仍有未覆盖分支：真实 socket 写阻塞触发的 5s 超时、全局 15s 关闭超时、系统信号读取
失败、部分文件系统 I/O 故障和非 UTF-8 环境变量错误。消息数/字节背压、正常连接 drain、
SIGTERM、身份损坏与排他锁已覆盖。后续补受控 I/O 故障注入与长时慢客户端压测；行覆盖率
不代表这些异常分支或其他平台已经验证。

## 未包含与后续

M0 只提供本机服务骨架；Project、Agent、Session、Run、SQLite、Provider、worker 模式和业务
持久事件均未实现，不在 capability 中声明。状态订阅是临时连接资源，没有 durable cursor。
下一步按 M1 建立全新 domain/ports/application/storage/workspace，先用离线 fake 验证
Message 不可变、Session CAS、Run 队列终止屏障、receipt 和事务 outbox。

本机验证不代表 Linux/Windows 已通过实测，也不证明未来 provider 子进程树的回收能力。
真实 provider 相关的现有忽略测试没有启用；新代码没有请求用户的 Provider 或打开现有会话。
内存/带宽压力下 TCP 内核缓冲与长时间慢客户端的系统级基准留待持续负载测试；当前验证
应用层队列的确定性预算和超限行为。
