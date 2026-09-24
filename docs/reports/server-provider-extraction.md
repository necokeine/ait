# server-provider 拆分实施报告

- 日期：2026-09-24。
- 基线：`00c8e82ef33c81cc092a2babd8789d8c2d39e630` 加上此前未提交的 metadata/filesystem 拆分。
- 决策：[ADR-031](../decisions/adr-031-server-provider.md)。
- 范围：独立 server；未修改 `ait-*`，未提交或推送。

## 实施结果

新增 [server-provider](../../crates/server-provider/src/lib.rs)，集中 Agent 的协议、端口、服务、
RPC 校验/投影、SQLite catalog 和 JSON runtime registry。纯 Agent 类型仍在 server-domain。

| 能力 | 已实现方法数 | 当前归属 |
| --- | ---: | --- |
| Agent preset 配置、查询、默认选择 | 5 | provider `protocol/agent`、`rpc/agents`、`service/agents`、`storage` |
| Agent runtime 目录与元数据生命周期 | 9 | provider `protocol/agent_lifecycle`、`rpc/agent_runtime`、`service/agent_runtime`、`storage/agent_runtime` |
| Workspace setup/script | 5 | metadata `protocol/rpc/service/ports/local` 下的 `workspace_automation` |
| Workspace clear_attention/mark_unread | 2 | metadata `protocol/rpc/service/ports` 下的 `workspace_state`，provider `service/workspace_attention` 适配 |

AgentManager 与 AgentClient/AgentSession 一并迁入 provider；未启用尚未接通的 Provider 执行接口。
协议仍公布 195 个可协商名称、102 个已实现 capability，未改变公共方法、JSON 字段或协议版本。

删除职责已迁完的 server-application、server-ports、server-storage、server-workspace，未留下转发壳。
独立 server 现在有七个 package：domain、metadata、filesystem、provider、protocol、api、bin。

## 依赖与兼容行为

- provider 的直接 workspace 依赖只有 domain 和 metadata；metadata/domain 均无 workspace 依赖。
  filesystem 仍只依赖 metadata。依赖守卫覆盖开发、构建、可选及平台条件依赖，并拒绝旧包重新加入。
- metadata 拥有 Workspace 自动化执行器，保留 `paseo.json`、信任准入、进程组、setup 顺序、
  输出限制和关闭回收行为；状态快照仍在内存中，不新增脚本恢复或持久化格式。
  setup 发布函数改用 `SetupProgress` 参数结构，Agent 查询改用 `QueryScope`，遵守最多 5 个参数的规范。
- Workspace state 通过 metadata 定义的窄端口调用 provider，metadata 不接触完整 Agent 记录。
  每次 clear-attention 批量请求只读取一次 Agent 列表，保留请求/registry 顺序、重复 Workspace、
  permission/internal/archived 过滤，以及中途失败前已经提交的 Agent ID。
- mark-unread 先验证 active Workspace，再由 provider 选择已完成的根 Agent；保留原有
  更新时间排序、单调时间、更新时候选复核和稳定错误。
- provider 与 Workspace state 使用同一 Agent registry 的共享句柄，未引入第二套缓存或文件 writer。
  `catalog.sqlite3`、`agents/agents.json`、schema、备份升级、不可变 revision 和回执保持兼容。
- `OwnedCatalog` 与 data-dir lease 继续由 binary 管理。API 保留 blocking 调度、HTTP/WS、
  鉴权、连接预算、队列、订阅、取消、drain 和 Worktree 创建后启动 setup 的协调。
- Agent preset 和 Paseo runtime record 继续区分；AgentManager 的创建/恢复/关闭只是既有基础，
  生产 binary 仍没有真实 Provider adapter。消息历史、执行和 skill 等原有占位能力未扩展。

## 验证

metadata/provider/protocol 的针对性单元测试共 161 项通过：metadata 101、provider 48、protocol 12。
新增窄端口单次扫描、批次顺序/部分提交、失效 Workspace、Agent 读取/更新故障、候选状态变化，
以及公开错误映射和依赖拒绝回归。既有真实脚本、WS、SQLite 升级/回执、registry 和 session 测试随模块迁移。

沙箱内首次运行的 4 个脚本/ setup 测试因本地端口分配被拒绝失败；在获准的本地 TCP/子进程环境
重跑后全部通过，没有跳过这些测试。Clippy、格式和 diff 检查通过。
完整 workspace 共 70 个普通测试目标：**831 passed / 0 failed / 5 ignored**；
23 个 doctest 目标：1 项通过、0 失败。独立覆盖率的完整运行同样为 831 passed / 0 failed / 5 ignored。
最终依赖清理和 setup 参数结构整理后，七个 server package 又完成 340 项回归和覆盖率补测，
均为 0 失败。API 清理未使用依赖后，又完成 53 项 API/宿主普通测试及覆盖率补测，
均通过；重复执行不累计为额外独立测试。

执行命令：

```sh
CARGO_TARGET_DIR=/tmp/ait-phase10-workspace-target cargo check --workspace --all-targets --offline
CARGO_TARGET_DIR=/tmp/ait-phase10-workspace-target cargo test -p server-provider -p server-metadata -p server-protocol --offline --no-fail-fast -- --test-threads=1
CARGO_TARGET_DIR=/tmp/ait-phase10-workspace-target cargo test --workspace --no-fail-fast --offline -- --test-threads=1
CARGO_TARGET_DIR=/tmp/ait-phase10-workspace-target cargo clippy --workspace --all-targets --offline -- -D warnings
cargo fmt --all --check
git diff --check
```

## Test coverage

本轮完整 workspace 行覆盖率为 **80.8813%（38,015 / 47,001）**。

| 范围 | covered / total | 行覆盖率 |
| --- | ---: | ---: |
| workspace | 38,015 / 47,001 | 80.8813% |
| server 七个 package | 14,588 / 16,870 | 86.4730% |
| server-api | 1,604 / 1,689 | 94.9674% |
| server-bin | 339 / 351 | 96.5812% |
| server-domain | 121 / 121 | 100.0000% |
| server-filesystem | 6,049 / 7,276 | 83.1363% |
| server-metadata | 4,578 / 5,286 | 86.6061% |
| server-protocol | 117 / 133 | 87.9699% |
| server-provider | 1,780 / 2,014 | 88.3813% |

相对可比基线，workspace 提升 **0.0860 个百分点**，server 总量提升 **0.2230 个百分点**。
可评审 artifact：[server-provider-coverage.json](server-provider-coverage.json)，包含逐文件计数、
crate 汇总、命令、测试结果、工具链和源码指纹。本地 HTML：
`/tmp/ait-phase10-cov-target/llvm-cov/html/index.html`。

测量基于本报告开头的 HEAD 加工作树；最终 Rust/manifest SHA-256：
`19018c88651e3e4a1fb240ed661e33671516736ce7dd3e5d4ab9ad54dd1aef6d`。

最初合并不同 setup 源码布局的 profile 时发现行映射重叠，已废弃该次计数，执行
`cargo llvm-cov clean --workspace --offline` 后重新采集完整 workspace。最后只移除 API 的
未使用 chrono 依赖，Rust 源文件未变化；补采 API/宿主并确认全部 251 个文件的覆盖率分母
与干净采集逐一相等。报告只使用干净采集及这次相同源码布局的补测。

比较基线为 [filesystem 拆分覆盖率](server-filesystem-coverage.json)：workspace
37,918 / 46,931 = 80.7952%，server packages 14,490 / 16,800 = 86.25%。
provider 没有同名独立基线；模块迁移改变 crate 分母，因此同时比较整个 workspace 和 server 总量。

测量命令：

```sh
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-phase10-cov-target cargo llvm-cov clean --workspace --offline
RUST_TEST_THREADS=1 CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-phase10-cov-target cargo llvm-cov --workspace --html --offline --no-fail-fast -j1
RUST_TEST_THREADS=1 CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-phase10-cov-target cargo llvm-cov --no-clean -p server-api -p server-bin --html --offline --no-fail-fast -j1
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-phase10-cov-target cargo llvm-cov report --html
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/tmp/ait-phase10-cov-target cargo llvm-cov report --json --summary-only --output-path /tmp/server-provider-coverage-raw.json
```

范围为全 Cargo workspace、默认 features；未显式排除生产文件，coverage 运行不含 doctest。
平台为 macOS arm64，Linux/Windows 未执行；原有外部模型凭据/独立 worker 测试保持 ignored。


主要未覆盖部分包括一些 SQLite/文件系统 I/O 故障、Agent 目录可选筛选/RPC 拒绝路径，
Workspace setup 错误投影和平台相关进程失败组合；已有 Git/Forge 超时与回滚等分支也仍有缺口。
后续可用故障注入补充这些分支。没有真实 Provider adapter 或 hosted GitHub/外部模型联调；
测试使用本地临时目录、仓库、模拟 CLI 和 Provider fake。
