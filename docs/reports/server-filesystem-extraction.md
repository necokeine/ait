# server-filesystem 拆分实施报告

- 日期：2026-09-24。
- 基线：`00c8e82ef33c81cc092a2babd8789d8c2d39e630` 加上一轮尚未提交的 server-metadata 拆分。
- 决策：[ADR-030](../decisions/adr-030-server-filesystem.md)。
- 范围：独立 server；未修改 `ait-*`，未提交或推送。

## 实施结果

新增 [server-filesystem](../../crates/server-filesystem/src/lib.rs)，迁入 48 个已实现方法对应的
协议 DTO、二进制文件帧、端口和运行数据、服务、RPC 校验/投影及 Git/gh/本机文件实现。
旧模块已删除并更新全部调用方，没有留下转发壳。

| 能力 | 已实现方法 | 主要入口 |
| --- | ---: | --- |
| Checkout/Git | 20 | `protocol/checkout`、`rpc/checkout`、`service/checkout`、`local/checkout` |
| Forge/PR | 10 | `protocol/forge`、`rpc/forge`、`service/forge`、`local/forge` |
| 文件/目录、订阅、上传下载 | 11 | `protocol/files`、`protocol/file_transfer`、`rpc/files`、`service/{files,uploads,transfer}`、`local/files` |
| Worktree | 3 | `protocol/worktrees`、`rpc/worktrees`、`service/worktrees`、`local/worktrees` |
| Workspace recovery | 2 | `protocol/workspace_recovery`、`rpc/workspace_recovery`、`service/workspace_recovery` |
| GitHub 仓库搜索/clone | 2 | `protocol/github_projects`、`rpc/github_projects`、`service/github_projects`、`local/github_projects` |

5 个 skill 方法名归 `protocol/skills` 所有，通用方法目录引用这些声明；仍返回 `not_implemented`。
本轮没有补建此前不存在的 skill 安装/选择实现。整体仍为 195 个可协商方法、102 个已实现能力。

`server-filesystem` 唯一的 workspace 依赖是 `server-metadata`，不依赖 API、通用协议、Agent、
application 或 Tokio/HTTP/SQL。依赖回归守卫已覆盖这些限制，包括开发与平台条件依赖。
metadata 仍不依赖任何 workspace crate。

## 协作边界和兼容行为

- Project/Workspace 记录和 JSON、labels、配置、图标及 server-id 仍由 metadata 持有。
  Worktree/recovery 复用同一组 registry；GithubProjects 使用共享 Directory adapter 的副本注册 Project。
- DirectorySource 的本机检查、mkdir 和空目录回滚迁入 filesystem，外层 Project 注册协调留在 metadata。
  纯 remote identity parser 留在 metadata，避免复制 Project key 的解析规则。
- Workspace recovery 已从 Agent attention 服务拆开；attention 仍在 application。
  Worktree 创建的注册失败回滚、归档时检查其他活跃引用、恢复保存的分支和嵌套目录均保留。
- GitHub clone 后注册失败仍保留已完成 checkout，响应保留 checkoutPath，Project 为空。
  Forge check 查询改用命名参数结构，wire 字段不变。
- 上传状态机、8 个并行槽、64 MiB 上限、600 秒过期及临时文件 RAII 清理归 filesystem；每个连接单独持有状态。
  一次性下载 token 的 60 秒期限、256 个上限和固定 canonical target 校验不变。
- 文件/diff 快照比较、预览校验、分块读取和结束时 revision 校验归 filesystem。
  API 保留 blocking 调度、timer、连接订阅所有权、队列、HTTP/WS 发送与取消。
  订阅响应先入队，之后激活；Worktree/recovery 更新事件仍跟在响应之后。
- `server-workspace` 只保留 setup/script 的实际进程运行模块；本轮不拆 Agent/Provider 执行能力。

未改变公共方法名、JSON schema、文件路径或协议版本；不改写历史数据。
现有多 Forge 支持、外部 Provider、skill 占位和文件操作跨进程并发限制没有扩展。

## 验证

最终全 workspace：73 个普通测试目标，**820 passed / 0 failed / 5 ignored**；
26 个 doctest 目标，1 项 doctest 通过。新 filesystem crate 的 121 项单元测试全部通过。
`cargo fmt --all --check`、全 workspace/all-targets Clippy `-D warnings` 与 `git diff --check` 全部通过。
依赖守卫确认 `server-filesystem -> server-metadata`，metadata 无 workspace 依赖。
宿主路由和真实进程回归复跑 61 项全部通过；最终全 workspace 又完成一次完整通过的运行。

执行命令：

```sh
CARGO_TARGET_DIR=/tmp/ait-phase10-workspace-target cargo test --workspace --no-fail-fast --offline -- --test-threads=1
CARGO_TARGET_DIR=/tmp/ait-phase10-workspace-target cargo clippy --workspace --all-targets --offline -- -D warnings
cargo fmt --all --check
git diff --check
```

测试覆盖原有 Git/gh、协议序列化、Worktree 回滚/恢复、文件权限/路径/写冲突、上传下载和
真实 HTTP/WebSocket 生命周期；新增 diff/file 去重、分块读取最终校验、上传跨连接隔离与
丢弃清理、skill 方法归属和 filesystem 反向依赖拒绝。
Git/Forge 集成测试使用临时本地仓库、bare remote 和模拟 gh CLI，不访问真实 GitHub 账户。
本地 TCP/WS 监听与子进程测试在获准的系统执行环境运行。

## Test coverage

实测结果：

| 范围 | 覆盖行 / 总行 | 行覆盖率 |
| --- | ---: | ---: |
| 整个 workspace | 37,918 / 46,931 | **80.7952%** |
| 全部 10 个 server packages | 14,490 / 16,800 | **86.2500%** |
| server-filesystem | 6,049 / 7,276 | **83.1363%** |
| server-metadata | 3,672 / 4,250 | 86.4000% |
| server-api | 2,253 / 2,447 | 92.0719% |
| server-application | 1,054 / 1,204 | 87.5415% |
| server-protocol | 116 / 130 | 89.2308% |
| server-workspace | 444 / 533 | 83.3021% |
| server-bin | 337 / 349 | 96.5616% |

相对上一轮 metadata 基线，workspace 增加 **0.1066 个百分点**，server 总量增加
**0.2276 个百分点**。模块移动改变了单 crate 分母；filesystem 没有同名独立基线。

可评审 artifact：[server-filesystem-coverage.json](server-filesystem-coverage.json)，包含逐文件计数、
各 crate 汇总、工具链、基线、命令、测试结果和本轮 Rust/manifest 源码指纹。
本地 HTML：`/tmp/ait-phase10-cov-target/llvm-cov/html/index.html`。

覆盖率首轮完整 workspace 运行遇到两处已编译的旧测试：GitHub 路由归属断言与 recovery
能力协商。修正后用 `--no-clean` 补跑整个 server-api/server-bin（61 passed），保留其余 workspace
profile，并重新生成完整 workspace HTML/JSON。重复执行的测试不作为新增独立测试计数。
最终普通测试的 820 项通过来自独立的完整 workspace 复跑。

测量命令：

```sh
RUST_TEST_THREADS=1 CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-phase10-cov-target cargo llvm-cov --workspace --html --offline --no-fail-fast -j1
RUST_TEST_THREADS=1 CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-phase10-cov-target cargo llvm-cov --no-clean -p server-api -p server-bin --html --offline --no-fail-fast -j1
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-phase10-cov-target cargo llvm-cov report --html
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-phase10-cov-target cargo llvm-cov report --json --summary-only --output-path /tmp/server-filesystem-coverage-raw.json
```

范围为整个 Cargo workspace 默认 features，未显式排除生产文件；coverage 运行不包含 doctest。
平台为 macOS arm64；Linux/Windows 未执行。5 项原有外部凭据/真实模型/独立 worker 测试保持 ignored。
基线为 [server-metadata 覆盖率 artifact](server-metadata-coverage.json)：workspace
37,705 / 46,729 = 80.6887%，server packages 14,278 / 16,598 = 86.0224%。
新 filesystem crate 没有同名独立基线，因此同时比较整个 workspace 与 server 能力总量。

主要未覆盖分支包括部分 Git/gh 超时、输出上限与操作系统错误，Worktree/recovery 的部分
registry 写失败及二次回滚失败，以及可选 Forge 字段和部分 RPC 错误投影。后续可针对这些
失败路径增加注入测试；本轮没有真实 GitHub 登录/多 Forge 联调，未使用外部模型凭据。
