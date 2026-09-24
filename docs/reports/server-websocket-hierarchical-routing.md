# 独立 server 的 WebSocket 层级路由

- 日期：2026-09-23；分支：`new`；基线：`b262455`。
- 设计边界：[ADR-026](../decisions/adr-026-canonical-paseo-websocket-surface.md)。

WebSocket 方法现在按 dotted prefix 建成一次初始化的只读树。连接收到完整 method 后沿各段查找一次；
叶子记录消息方向、协商 capability 和已实现方法的处理器。树节点可以既是方法又有子节点，因此早期
`project.list` 与 Paseo `project.list.request` 仍有各自语义；共享前缀也不强迫 Git diff、Forge PR 或
Workspace 标签使用同一个业务处理器。

路由树先登记 188 个规范 Paseo 方法（包括占位叶子），再将已有 104 个实现方法绑定到处理器，
另加 `server.status.unsubscribe` 对 `server.status.subscribe` 的协商兼容入口。重复处理器登记或
消息方向冲突会在构建时失败。连接层不再分别搜索方法 catalog、分组表和文件方法清单；文件帧仍
由连接持有上传状态，业务 DTO 和服务边界没有改变。响应后激活标签/Diff 订阅和发送 Workspace 事件的
顺序也没有改变。

路由测试现在逐一检查已实现方法恰有一个处理器、全部 199 个公开名称有叶子、catalog 方向一致，
并覆盖同前缀不同处理器、占位方法、原版旧名称及订阅取消兼容入口。真实 WebSocket 回归由
`server-api` 和 `server-bin` 的工作区测试继续覆盖。这个变更只调整路由定位，不解决同一连接上慢
请求阻塞后续消息、全局业务任务单槽串行化等并发问题。

## Test coverage

普通整仓测试 `CARGO_TARGET_DIR=/tmp/ait-phase10-workspace-target cargo test --workspace --no-fail-fast -j1`
通过；整仓 build、`cargo clippy --workspace --all-targets -- -D warnings`、`cargo fmt --all -- --check`
和 `git diff --check` 均通过。定向路由测试 3 项通过。插桩整仓运行 72 个普通测试 target：
**795 通过、0 失败、5 忽略**；不包括普通回归的 doc test。

测量对象是 `b262455` 加本报告对应的工作区改动。命令：

```sh
CARGO_TARGET_DIR=/tmp/ait-phase10-cov-target cargo llvm-cov --workspace --json --summary-only --output-path /tmp/ait-ws-hierarchy-coverage.json --no-fail-fast -j1
CARGO_TARGET_DIR=/tmp/ait-phase10-cov-target cargo llvm-cov report --html
```

范围为 Cargo workspace 默认 features、232 个生产 Rust 文件；工具默认过滤测试与 build script，
没有额外排除文件，Linux 与 Windows 未运行。可评审的
[coverage artifact](server-websocket-hierarchical-routing-coverage.json)记录工具链、命令、范围、原始行数
和上一版基线；本机 HTML 在 `/tmp/ait-phase10-cov-target/llvm-cov/html/index.html`。

| 范围 | 覆盖 / 总行数 | 行覆盖率 | 上一版同口径 |
| --- | ---: | ---: | ---: |
| 整个 workspace | 37,787 / 46,763 | **80.81%** | 80.73% |
| 独立 server 的 8 个 package | 14,337 / 16,632 | **86.20%** | 86.17% |
| `server-api` | 4,334 / 4,994 | **86.78%** | 86.61% |
| WebSocket 路由模块 | 248 / 271 | **91.51%** | 88.42% |

路由模块未覆盖的主要是重复登记/方向冲突的防御断言及不应由通用业务分发器接收的文件处理器分支；
连接层异常 socket 写入和二进制帧错误仍需故障注入。上述覆盖率只证明当前已实现行为的测试范围，
不代表 95 个占位 Paseo 方法具有对应业务行为。
