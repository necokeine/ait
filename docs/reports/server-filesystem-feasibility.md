# server-filesystem 拆分可行性分析

- 日期：2026-09-24。
- 代码基线：`00c8e82ef33c81cc092a2babd8789d8c2d39e630` 加已完成、尚未提交的 `server-metadata` 拆分工作区。
- 范围：独立 `server`、`bins/server`、`crates/server-*`；不迁移旧 daemon 或 `ait-*`。
- 状态：实施前调研快照；后续已按 [ADR-030](../decisions/adr-030-server-filesystem.md) 实施，当前结果见 [实施报告](server-filesystem-extraction.md)。下文“当前”指调研时的代码；链接指向迁移后的对应文件。

## 结论

可行，建议拆成完整的纵向能力 crate：`server-filesystem` 拥有 Git、Forge/PR、通用文件目录、
上传下载、worktree 和 skill 文件管理的业务契约、协议、服务、RPC 处理与本机实现。
其中 Git、Forge/PR、文件目录已有完整实现；skill 目前只有占位方法，实际实现属于后续新增工作。

建议允许 `server-filesystem -> server-metadata`，保持 metadata 不反向依赖 filesystem。
Worktree、恢复和 clone 需要使用 Project/Workspace 记录及 registry，这条单向依赖可以保留现有
协作行为。无需改现有 JSON schema、工作目录、上传路径或客户端 wire 方法名称。

规模高于简单改包名：核心 20 个生产源码文件共 **9,689 个物理行**；加上 Worktree、GitHub
provisioning、目录 adapter 等 11 个文件后，31 个候选文件共 **12,713 行**。这包含仍应留在 API
的 transport glue，不是精确净迁移量；尚未计入混合文件的局部拆分、测试、fixture、host 和路由。

## 现有能力与建议归属

| 能力 | 当前实现 | 建议处理 |
| --- | --- | --- |
| Git checkout | 20 个方法：status/diff/history、branch、commit、merge、pull/push、stash、discard | DTO、端口、服务、RPC 投影和 Git adapter 整组迁入 |
| Forge / PR | 10 个方法：search、PR create/merge/status/timeline、auto-merge、checks | 整组迁入；目前实际 adapter 是 GitHub `gh`，不是完整多 Forge 实现 |
| 文件和目录 | 11 个方法：suggestions、explorer、订阅、写入、create/rename/duplicate/delete、download token、upload | DTO、业务状态、二进制帧、文件实现整组迁入 |
| Worktree | list/create/archive 3 个方法 | 协议、协调服务、端口与 adapter 迁入，registry 仍由 metadata 拥有 |
| Workspace recovery | inspect/restore 2 个方法，与 Agent attention 混在一起 | 拆出 recovery 部分迁入；clear-attention / mark-unread 留在 Agent 协调用例 |
| GitHub repository discovery / clone | 2 个方法，目前跨 metadata Directory 与 workspace adapter | 协议、GitHub 端口、搜索/clone 用例和 adapter 一起迁入；通过 metadata 注册 Project |
| Skill | 5 个方法，仅 catalog 占位 | 迁移方法归属，保持 `not_implemented`；补安装、选择和存储是另一步实现 |

建议迁移范围对应 **48 个已有实现方法 + 5 个 skill 占位方法**。纯拆分后整个 server 的
195 个可协商名称、102 个已实现 capability 应保持不变；不能把占位方法计为新增实现。

数量来源：[checkout](../../crates/server-filesystem/src/protocol/checkout.rs)、
[Forge](../../crates/server-filesystem/src/protocol/forge.rs)、[files](../../crates/server-filesystem/src/protocol/files.rs)、
[worktrees](../../crates/server-filesystem/src/protocol/worktrees.rs)、
[recovery / attention](../../crates/server-metadata/src/protocol/workspace_state.rs)、
[GitHub provisioning](../../crates/server-filesystem/src/protocol/github_projects.rs)、
[skill 方法目录](../../crates/server-protocol/src/methods.rs)。

## 需要迁移的代码链

| 层次 | 当前主要落点 |
| --- | --- |
| Wire DTO / capability | `server-protocol/src/{checkout,forge,files,file_transfer}.rs`；metadata 的 `worktrees`、`github_projects` 和 recovery DTO |
| 业务解码、错误投影、返回值 | `server-api/src/{checkout,forge,files,worktrees,workspace_state}.rs`；metadata `rpc/directory.rs` 的 GitHub 部分 |
| 用例 | `server-application/src/{checkout,forge,files,worktrees}.rs`；`workspace_state.rs` 的恢复部分；metadata Directory 的 GitHub 用例 |
| 端口与运行数据 | `server-ports/src/{checkout,forge,files,worktrees,workspace_recovery}.rs`；metadata 的 `ports/github_projects.rs` |
| 本机实现 | `server-workspace/src/{checkout,forge,files,worktrees,github_projects,provisioning,git}.rs` 及 `files/{search,upload}.rs` |
| 传输和进程组装 | API 的连接、队列、HTTP download、blocking jobs；`bins/server/src/host.rs`，保留宿主职责并更新组装 |

当前大部分业务数据已经在对应 port 模块中，通常无需迁移 `server-domain` 的 Agent 类型。
`server-workspace` 也不能直接整体改名：其中还有 `LocalWorkspaceAutomation` 的 setup/script
进程管理。建议本轮保留这个有实际消费者的模块，后续按执行能力边界处理，不新增空转发壳。

## 与 metadata 的分界

### Project/Workspace 持久化仍属于 metadata

`projects.json`、`workspaces.json`、label journal、Project config/icon、daemon config 和 server-id
继续归 metadata。它们虽然写文件，但其格式、校验和事务属于对应业务，不是通用文件 API。

Project/Workspace descriptor 中的 checkout、Git/Forge 状态投影也可以继续作为 metadata 的
读模型；filesystem 消费这些类型。不要让 descriptor 为引用实际 Git 服务而反向依赖 filesystem。

### 本地目录检查通过 metadata 的消费端口注入

[DirectorySource](../../crates/server-metadata/src/ports/provisioning.rs) 是 metadata 创建/打开
Project、Workspace 所需的目录观察契约。建议该契约保留，`LocalDirectorySource` 实现迁入
filesystem，由 binary 注入。如此 `project.add.request`、`workspace.open.request` 和
`project.create_directory.request` 的 Project 用例可以继续在 metadata 协调，而实际检查、
mkdir、空目录回滚和 Git inspection 都来自 filesystem。

通用 `fs.entry.create.request` 与 `project.create_directory.request` 不是同一事务：后者还要
注册 Project，失败时尝试删除新建空目录；不能迁移时合并为一个普通文件创建调用。

### GitHub clone 从 Directory 中单独抽出

现有 [Directory](../../crates/server-metadata/src/service/directory.rs) 持有
`GithubProjectsRuntime`，并承担搜索、clone、URL 校验和 clone 后 Project 注册。
若只搬 `LocalGithubProjects`，完整 GitHub 业务仍会散落在 metadata。

建议抽出 filesystem 的 GitHub provisioning 服务，迁移对应请求/响应、端口和 RPC 分支，
删除 metadata Directory 的 GitHub 字段与专用方法。新服务使用 metadata 的 Project 注册能力，
通过同一个 registry 实例发布记录。clone 成功但注册失败时仍保留 checkout，并返回现有的
`checkoutPath` / `project=null` 语义。

当前 clone URL 校验与 Project key 推导共享 remote 解析辅助逻辑。应保留 Project 身份算法
在 metadata，按需暴露纯解析/规范化接口，避免搬走共享辅助函数后产生反向依赖或改变 Project key。

### Worktree 与 recovery 保留跨能力失败语义

[Worktrees](../../crates/server-filesystem/src/service/worktrees.rs) 已有“Git 创建后注册 Workspace，
注册失败则回滚”的流程；归档还会检查是否仍有活跃 Workspace 引用，才删除实际 worktree。
这些协调逻辑适合随 Worktree 用例一起迁入，并依赖 metadata 的 registry 端口。

[WorkspaceState](../../crates/server-application/src/workspace_state.rs) 同时处理 Agent attention
和归档恢复。恢复只需要 Project/Workspace registry 与本机恢复能力，可以独立拆出；不应把
Agent attention 类型、Agent registry 或整个 `server-application` 拉入新 crate。
恢复必须继续使用保存的精确 branch/path，保留 registry 更新顺序和现有失败行为。

## 传输与业务状态必须分开

这里比 metadata 的普通请求迁移更复杂：

- **二进制协议**：[file_transfer.rs](../../crates/server-filesystem/src/protocol/file_transfer.rs) 属于文件
  能力，应迁入 filesystem。现有实现直接使用 protocol 的 `valid_id` / `ErrorCode`；需改成
  独立的帧校验和 `FrameError`，由宿主映射到公共错误，避免 `protocol <-> filesystem` 环。
  opcode、ID 长度约束、Begin JSON 布局、256 KiB chunk 和非法帧行为保持兼容。
- **上传状态**：[FileConnection](../../crates/server-api/src/files/connection.rs) 将状态机与
  socket glue 混在一起。声明长度、Begin/Chunk/End 状态、临时 writer、完成提交和失败清理迁入
  filesystem；状态对象仍由每个物理连接单独持有。API 负责传入帧、任务调度、错误发送和断连 drop。
- **文件与 diff 订阅**：版本/指纹比较、业务事件构造归 filesystem；Tokio timer、TaskTracker、
  CancellationToken、发送背压和连接订阅预算由 API 驱动。保持“初始响应入队后再启动更新”，
  同 ID 替换、release、断连与 drain 的原有行为；本次不把 200 ms polling 说成 native watcher。
- **下载**：[Files](../../crates/server-filesystem/src/service/files.rs) 的 60 秒一次性 grant、canonical
  target 校验、reader 与 chunk/revision 逻辑归 filesystem；[HTTP endpoint](../../crates/server-api/src/files/transfer.rs)、
  Host/Origin、独立 token 认证、响应 header 和 Body streaming 仍归 API。
- **blocking 调度**：新 crate 可以维持阻塞接口及 `std`/本机实现，无需依赖 Axum、Tokio、SQL
  或 Provider。API 继续在 tracked blocking job 中调用，不在 async reactor 直接执行 Git/文件 I/O。

## Skill 的实际现状

独立 server 仅登记以下请求，尚无专用 DTO、service、安装器或 skill 文件存储：

```text
agent.skills.get_status.request
agent.skills.reconcile.request
agent.skills.uninstall.request
agent.skills.save_selection.request
agent.skills.import_legacy_selection.request
```

daemon config 中已有 `skills: Option<Value>`，reload 分类为 `agents.skills`；这是配置载体，
不是安装实现。首轮可以迁移方法声明/归属并保留占位，不能从旧 `ait-*` 引入另一套 skill 业务。

后续实现 skill 时，需要进一步对齐请求 schema、selection 配置、来源发现、目标目录布局、
安装 ownership、覆盖/卸载规则、legacy 导入及 Provider-specific 目标。skill 目录扫描、内容读取、
安装、reconcile 和卸载适合归 filesystem；Agent 选择上下文与运行时消费通过注入接口提供。
全局 `config.json` 仍只有 metadata 的配置存储拥有，filesystem 不另开一条独立写入路径。

## 建议依赖和目录

```text
server-api / server-application / server-bin -> server-filesystem -> server-metadata
server-protocol -> server-filesystem                 (公共目录和业务 DTO 聚合)
server-metadata -> 无其他 workspace crate
```

filesystem 不依赖 server-api、server-application、server-protocol、server-ports 或旧 `ait-*`。
本能力所需 port 随实现迁入；metadata 的消费端口例外保留在 metadata，由 filesystem 实现。
总 Hello/RPC envelope、公共错误和 transport 能力仍在 protocol，由宿主聚合业务能力声明。
必须更新 [依赖守卫](../../bins/server/tests/dependencies.rs)；当前守卫会拒绝新增 crate 和依赖边。
protocol 经 filesystem 间接编译本机适配依赖，是纵向单 crate 方案的明确取舍。

```text
server-filesystem/src/
  protocol/     # checkout、forge、files、file_transfer、worktrees、recovery、github、skill 声明
  model/        # 本能力的数据与错误
  ports/        # Git、Forge、文件、Worktree、skill 边界
  service/      # 用例、下载 grants、上传状态机、GitHub provisioning
  rpc/          # 业务解码、投影、错误分类、订阅快照/更新
  local/        # 本机 Git/gh、路径、搜索、文件与上传实现
```

目录仅表示目标职责；skill 未实现前不创建无消费者的存储或空服务。

## 风险与实施顺序

主要风险是连接状态和跨能力副作用，不是 Cargo 本身。需要保留：

1. 文件 lexical/realpath 范围检查、symlink 处理、revision 优先写入、读后 revision 验证和权限。
   现有实现仍有外部写入及祖先目录替换的竞态；重构不等于新增跨进程 CAS 或完整 OS sandbox。
2. upload 的每连接 8 个 pending / 64 MiB 限制、空闲过期、异常及断连时 RAII 清理；download
   token 一次消费、失效和路径固定；不要把连接状态放入一个全局共享文件服务。
3. Git/gh 的参数、环境、输出上限和超时。现有 checkout/worktree 写预算为 120 秒，clone 为
   300 秒；全局 business job lane 与 15 秒 host drain 的协调仍是宿主问题，拆 crate 不自动改善它。
4. clone、worktree、恢复与 registry 的提交/回滚顺序；同一个 registry 缓存、observer、freeze
   语义不得因为重新组装而变成同一路径上的多个独立实例。
5. 精确 method 路由、协商能力、公共错误码、null/缺省字段、文件帧及响应后事件顺序。

建议实施为四个可分别验收的步骤：

1. 确定边界 ADR，先迁移 checkout/Forge/files 的 DTO、port、服务、adapter 与测试，更新依赖守卫。
2. 抽出 API 中的业务 handler、上传状态机、帧和快照投影；保留 transport 调度并验证连接隔离。
3. 迁移 Worktree、recovery、GitHub provisioning，拆开 metadata Directory 与 WorkspaceState，
   更新 host、capability 和精确路由；保留 metadata 的记录与配置所有权。
4. 迁移 skill 占位方法归属，完成纯拆分验收。skill 实际安装/选择持久化另行实现与验收。

正式实施需新增 ADR 修订 ADR-029 的能力归属、更新 docs 索引；保持旧 client wire 及现有文件路径。
已有 Git/Forge 的本地 remote、假 gh、真实 WebSocket、上传/下载、recovery 与 rollback 测试应原样
迁移，再补依赖方向、跨连接帧、取消后不再入队和失败回滚的针对性回归。

## Test coverage

本轮仅调研并新增本文，未修改 Rust、Cargo 或运行行为。**not applicable — no Rust behavior changed**。
未重跑编译、测试或覆盖率，也没有把上轮的 810 项测试或覆盖率作为本次测量。
已静态核对实现入口、capability 数量、依赖和候选文件规模；完成文档差异检查。

后续实施应运行 format、严格 workspace Clippy、workspace tests 和 `cargo llvm-cov --workspace --html`，
并提交可评审的覆盖率摘要。当前 [metadata 拆分覆盖率 artifact](server-metadata-coverage.json)
可作为后续同口径比较基线；skill 占位、真实远端认证、Windows 行为和现存文件竞态需单独说明，
不以搬迁测试通过代表这些行为已实现。
