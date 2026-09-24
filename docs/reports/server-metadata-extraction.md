# server-metadata 拆分与旧 Project 退役

- 日期：2026-09-24。
- 代码基线：`00c8e82ef33c81cc092a2babd8789d8c2d39e630` 加本次工作区改动。
- 范围：`bins/server`、`crates/server-*`；决策见 [ADR-029](../decisions/adr-029-server-metadata.md)。

独立 server 现在只有 Paseo Project/Workspace registry 一套项目模型。删除了早期
`project.open/list/get/close` 及其 DTO、路由、UUID Project、根 Message 初始化、SQLite Project
adapter、目录/身份租约和 owner epoch。Project/Workspace 通过现有 `*.request` 方法操作；
新建 ID 继续为 `prj_` / `wks_`，同一个 Project 可拥有多个独立 Workspace。

新增 `server-metadata`，完整拥有 metadata 的业务协议、记录、校验、端口、服务、RPC 处理与文件
存储。迁移包括 directory、labels、Project config/icon、daemon config/status/diagnostics、持久
server-id、应用层 ping，以及 Workspace worktree/setup/recovery 的 DTO。标签跨文件事务、恢复
journal、缓存与 observer 实现保持在同一 crate。新 crate 不依赖任何 workspace crate；原有调用者
已直接使用它的模型与端口，通用 JSON registry 引擎供 Agent runtime snapshot adapter 复用。

HTTP/WS、鉴权、队列、连接订阅所有权、blocking 调度和实例锁继续由宿主负责。标签 handler 通过
注入的事件 sink 返回订阅句柄，宿主在响应入队后激活；restart/shutdown 由 metadata 产生意图，
宿主执行 drain。Git、worktree、GitHub CLI、shell 和 Agent session 的实际执行保留在运行模块。
`server-protocol` 聚合 metadata 的公共类型，因此会间接编译文件存储依赖；这是单 crate 纵向封装
的明确取舍，纯 `server-domain` 的依赖约束保持有效。

## 数据与接口兼容性

- 新体系的 wire 字段、JSON schema、文件路径和已有字符串身份不变；协议版本仍为 1.0。
- 旧四个 Project 方法不再参与协商，已握手连接调用时返回 `method_not_found`。
- 历史 Project SQLite 文件及旧 catalog 中的历史表保留在磁盘，业务不再读取或维护；不做自动
  数据转换。现有目录可以通过 `project.add.request` 注册为新体系的 Project。
- 新 `catalog.sqlite3` 只创建 Agent preset/revision/default/receipt 表；既有 v1 → v2 Agent
  schema 升级继续先备份，保留历史表和记录。
- 生产 binary 公布 195 个可协商名称、102 个已实现 capability；188 个 Paseo canonical methods
  中仍有 93 个占位。`session.heartbeat` 只迁移方法归属，仍未实现，不新增 heartbeat 文件。

## Test coverage

普通整仓测试和插桩整仓测试各为 **810 通过、0 失败、5 忽略**（72 个普通测试 target）。
普通运行还完成 25 个 doc-test target，其中 **1 个测试通过、0 失败**。`server-metadata`
自身 98 项测试、真实 server process target 的 23 项测试全部通过。整仓严格 Clippy、格式
检查和 `git diff --check` 均通过。

回归包括：旧方法拒绝、新 Project/Workspace 重启持久化、历史项目文件保留、Agent-only
catalog、新 metadata 依赖方向、协议错误码和重试语义、ping 参数兼容、标签响应后激活及断连
释放，以及迁移前已有的 schema fixture、标签事务恢复、配置/图标和真实 WebSocket 测试。

| 测量范围 | 覆盖 / 总行数 | 行覆盖率 |
| --- | ---: | ---: |
| 整个 workspace | 37,705 / 46,729 | 80.69% |
| 独立 server 的 9 个 package | 14,278 / 16,598 | 86.02% |
| `server-metadata` | 3,879 / 4,514 | 85.93% |
| `server-api` | 3,426 / 3,861 | 88.73% |
| `server-application` | 1,729 / 2,005 | 86.23% |
| `server-storage` | 441 / 487 | 90.55% |
| `server-workspace` | 4,163 / 5,067 | 82.16% |

与拆分前同口径的 [基线 artifact](paseo-github-project-provisioning-coverage.json) 相比，
整仓从 38,382 / 47,438（80.91%）变为 37,705 / 46,729（80.69%），下降 **0.22 个百分点**；
独立 server 从 14,954 / 17,307（86.40%）变为 14,278 / 16,598（86.02%），下降 **0.38 个百分点**。
本次删除旧切片并迁移模块，覆盖行与总行分母都发生变化；`server-metadata` 没有独立的拆分前
crate 基线。以上是整个 crate 的生产行覆盖率，不是本次新增行的单独测量。

测量对象为上述 `00c8e82` 加本次工作区改动；artifact 记录了 Rust 源码与 Cargo manifest/lock
的 SHA-256，以明确未提交工作区的测量版本。范围是 workspace 默认 features 的 233 个生产
Rust 文件，使用 cargo-llvm-cov 默认的测试/build source 过滤，无额外文件排除。平台为
macOS / arm64（Darwin 25.6.0），rustc 1.98.1、cargo-llvm-cov 0.8.4；Linux 和 Windows 未运行。
插桩运行不包含 doctests，普通运行已独立验证。5 个既有 ignored 测试需要真实 Codex/DeepSeek
凭据和付费模型访问，或独立构建的外部 worker；具体名称与原因保存在 artifact。

执行命令：

```sh
cargo test --workspace --no-fail-fast --offline -- --test-threads=1
cargo clippy --workspace --all-targets --offline -- -D warnings
cargo fmt --all --check
RUST_TEST_THREADS=1 CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-phase10-cov-target cargo llvm-cov --workspace --html --offline --no-fail-fast -j1
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-phase10-cov-target cargo llvm-cov report --json --summary-only --output-path /tmp/server-metadata-coverage-raw.json
git diff --check
```

首轮沙箱执行因本机 TCP 监听及进程操作被拒绝而失败，随后以必要权限重跑；上述结果全部来自
成功的重跑，未合并失败轮次。可评审的 [coverage artifact](server-metadata-coverage.json)
包含总数、各 server crate、逐文件行数、命令、基线与测试统计。本机 HTML 为
`/tmp/ait-phase10-cov-target/llvm-cov/html/index.html`。

未覆盖的主要分支：directory RPC 的部分错误投影、筛选/排序组合与非法图标响应；图标的
JPEG/GIF/WebP 解析和文件失败；server-id 等 adapter 的操作系统权限/持久化失败。
后续可通过格式样本和故障注入补齐。`session.heartbeat`、其余占位方法和未接入的运行态行为
没有因为本次迁移而获得业务实现。
