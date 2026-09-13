# ADR-001：ProjectWorkspace 本地基础设施边界

- 状态：待代码审查
- 日期：2026-09-13
- 依赖：ADR-001 v4、NEC-154 ADR-002、NEC-209、NEC-212、ADR-013

## 决策

新增 `ait_ports::ProjectWorkspace`，由 `ait_project_local::LocalProjectWorkspace`
实现。已有同步 `ProjectEnvironment` 服务于指令读取/早期 ProjectService，继续兼容；
不将控制面 lease、Session worktree 和异步取消语义塞入该同步接口。

`LocalControlService` 的两个构造器显式接收 `Arc<dyn ProjectWorkspace>`，daemon
composition root 注入本地实现。application 的 production dependencies 不包含
project-local；storage/provider 也不因本次改动依赖具体 Project adapter。

| adapter 负责 | application 负责 |
| --- | --- |
| canonical Git root 准备与复核、空初始 commit、完整 HEAD/index tree、clean baseline、Git dir/symbolic HEAD | Project identity、重复注册、Session/Message 选择、事务 CAS 与重试 |
| `.ait/<session>` linked worktree 的检查与初次创建 | Session id/所属 Project/Message baseline 的选择、失败路径提示 |
| canonical root、最近已存在 ancestor 的 canonical 事实；不可解析、dangling symlink、非 UTF-8 拒绝 | lexical containment、拒绝 `..`、比较 canonical 事实、Run 权限与管理员上限 |
| 不透明 `Arc<dyn WorkspaceLease>`；本进程队列和 `.git/ait/locks/workspace-write.lock` advisory lock | 准入时机、Run/恢复期间所有权、释放时机、integration gate 和 publication fencing |

适配器返回 `DomainError` 的稳定 code/retryable，application 转换为 `ApiError`。
没有通用 shell/Git 命令执行 port，调用方只能使用具名 Project 操作。
`ProjectDirectoryCreator` 同时改为异步 port；本地实现保留同步兼容入口，生产调用
通过下述阻塞边界执行 Documents 解析和独占 mkdir。

## 安全与一致性

Git root 在初始化前后验证；human Message baseline 继续读取 HEAD/index → status →
HEAD/index → HEAD tree。HEAD 变化返回可重试 `PROJECT_GIT_HEAD_UNAVAILABLE`，
index 变化返回可重试 `PROJECT_GIT_DIRTY`，普通 dirty/unborn 保持原错误。
CAS 冲突重新读取记录和基础设施事实；拒绝 baseline 时不写 Message、Run 或 Session。

同一 canonical Project 的别名共享 advisory lock，不同 adapter/service/process
不能绕过锁；不同 Project 可以并发。共享 lease 最后一份引用释放才关闭文件锁。
阻塞 worktree 操作持有自己的 lease 引用，请求 future 丢弃不能提前释放它。
已存在 Session worktree 只检查、不 reset；仅新建、manager-owned 的 linked worktree
允许按已验证 baseline 填充。部分目录和 worktree 保留，失败不做自动 cleanup/rollback。

Run journal、operation_id/lease_epoch、worker receipt、checkpoint、integration gate、
Run ref 和补偿发布算法仍沿用 NEC-209/NEC-212；本票仅迁移控制面本地能力。
Git 对账歧义仍仅中断所属 Run，不猜测用户工作区状态。

## 阻塞、取消和限制

- 所有新异步 Project 操作在 project-local 的 `spawn_blocking` 边界运行；进程级
  semaphore 最多允许 4 个已接纳操作，独立 adapter 实例共享该预算。
- 单次操作从排队起计算 30 秒 deadline。workspace 同进程队列也有 30 秒上限；
  跨进程 file lock 使用 `try_lock`，竞争立即返回可重试 `PROJECT_WORKSPACE_BUSY`。
- Future drop 取消排队或标记已启动操作；worker 在启动/每条 Git/轮询/返回前检查。
  已启动 worker 保留 permit 与 lease 到实际退出。不可中断的 OS 文件 syscall 无法
  承诺硬性墙钟上限，因此不以 async timeout 提前释放资源。
- Git 禁用交互输入、hooks 和 fsmonitor。stdout/stderr 使用文件捕获，轮询大小，
  每路最多读取 1 MiB，超限失败；轮询间短暂写入可能超出该阈值。
- 超时/取消会终止并 reap Git；Unix 使用独立 process group 同时终止仍在运行的
  子进程组，其他平台保证直接 Git child 的 kill/reap。本票不是 worker sandbox，
  不新增对任意 filter 派生进程的跨平台隔离承诺。
- 路径事实是时点观察，不替代实际文件操作的 no-follow/dir-handle 防护；与原设计
  一样，授权不能抵御检查后恶意外部进程重新替换 symlink。工具/worker 的强制边界
  继续由对应 sandbox 实现。

## 验证

- ports 的 `contract-tests` feature 提供可复用 contract kit，由真实临时 Git adapter 运行。
- project-local 验证 unborn/staged/dirty、既有 worktree 保留、canonical alias、真实
  子进程竞争、dangling/non-UTF-8 路径、HEAD/index 确定性竞态、Git deadline 和
  future drop 后 lease 持续到阻塞调用排空。
- application 使用 fake facts 验证授权上限、canonical/lexical escape、CAS 重新取事实
  和 baseline 拒绝前后的持久化顺序；原 lease/recovery/approval 故障注入继续保留。
- 交付检查：`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、
  `cargo test --workspace`。实际结果记录在 PR/issue。
