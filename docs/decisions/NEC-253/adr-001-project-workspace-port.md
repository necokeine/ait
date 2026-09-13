# ADR-001：ProjectWorkspace 本地基础设施边界

- 状态：待代码审查
- 日期：2026-09-13
- 依赖：ADR-001 v4、NEC-154 ADR-002、NEC-209、NEC-212、NEC-252、ADR-013

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
结合 NEC-252，注册/导入先读取 canonical path facts，按规范路径查重，再执行一次准备。
`prepare_git_root` 的 `expected_root` 在任何 Git 写入前拒绝被重绑定的已查重目标；
`verify_git_root` 只读确认规范路径仍是 exact Git root，不初始化或修复。
CAS 冲突重新读取 typed context、权限和基础设施事实，核对 preparation key 与冻结的
Project root/HEAD；目录/worktree 准备不进入重试循环。index 通过只读 `diff-index`
与 HEAD tree 比较，不使用 `write-tree`；Git 设置 `GIT_OPTIONAL_LOCKS=0`，不刷新 index。
拒绝 baseline 时不写 Message、Run 或 Session。导入遇到保留的旧 HEAD worktree 会拒绝复用。

同一 canonical Project 的别名共享 advisory lock，不同 adapter/service/process
不能绕过锁；不同 Project 可以并发。共享 lease 最后一份引用析构时先显式 `unlock`，再释放进程内队列与文件描述符。
不能仅依赖 close：并发 fork/复制描述符可以继续持有相同 open file description。
阻塞 worktree 操作持有自己的 lease 引用，请求 future 丢弃不能提前释放它。
已存在 Session worktree 只检查、不 reset；仅新建、manager-owned 的 linked worktree
允许按已验证 baseline 填充。部分目录和 worktree 保留，失败不做自动 cleanup/rollback。

Run journal、operation_id/lease_epoch、worker receipt、checkpoint、integration gate、
Run ref 和补偿发布算法仍沿用 NEC-209/NEC-212；本票仅迁移控制面本地能力。
Git 对账歧义仍仅中断所属 Run，不猜测用户工作区状态。

## 阻塞、取消和限制

- 所有新异步 Project 操作在 project-local 的 `spawn_blocking` 边界运行；进程级
  semaphore 最多允许 4 个已接纳操作，独立 adapter 实例共享该预算。
- 每次 public port 调用进入时创建一个 absolute deadline（30 秒），贯穿 capacity
  queue、canonicalize、同进程 lease queue、file lock 和全部 Git/文件子阶段。
  `ensure_session_worktree` 隐式获取 lease 也复用这个 deadline；每次等待只消费剩余
  预算，不重新计时。跨进程 file lock 使用 `try_lock`，竞争立即返回可重试
  `PROJECT_WORKSPACE_BUSY`。此预算限定单次 port 调用，不是整个 application 事务。
- 超时返回 `details.reason = "timeout"`，按 public 操作映射稳定错误：root 创建/只读复核/worktree
  创建为 `PROJECT_GIT_INIT_FAILED`，HEAD/baseline/branch/Git dir 为
  `PROJECT_GIT_HEAD_UNAVAILABLE`，lease 为 `PROJECT_WORKSPACE_BUSY`，path facts 为
  `PROJECT_PATH_NOT_FOUND`，Documents mkdir 为 `PROJECT_DIRECTORY_CREATION_FAILED`。
  未开始业务 mutation 的超时可重试；取消使用 `RUN_CANCELLED` 且不可重试。
- 创建前记录路径与 intent，成功后更新确认状态。任何后续失败（包括返回前跨越
  deadline）附加 `details.retained_paths = [{path, state}]` 并设 `retryable = false`。
  `*_started` 表示可能留下部分状态，不能宣称已完成；`directory_created`、
  `git_initialized`、`initial_commit_created`、`worktree_created`（尚未填充）、
  `worktree_populated` 表示对应阶段已成功。`.ait` 和 Git exclude 的修改也记录。
  可读 message 同时包含路径/状态，让未传递 DomainError details 的 API 仍能提示
  用户检查。已明确失败的独占 mkdir 不归因为本次创建；可安全复用的 lease 锁文件
  不属于业务 retained artifact。不会自动清理、复用或重置部分 worktree。
- Future drop 和 deadline 共用 RAII 取消路径：对 `QUEUED` 原子置为 `CANCELLED`
  并 abort Tokio task，同时从共享槽位取出待执行 closure，立即释放其 lease/permit。
  不能只依赖 token 或 detached JoinHandle，也不能等待饱和 blocking pool 销毁 closure。
  `STARTED` 赢得资源所有权后由 worker 独占，future drop 只请求 cooperative cancellation；
  worker 在启动/每条 Git/轮询/返回前检查。
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
  future drop 后已启动 worker 的 lease 持续到阻塞调用排空；`max_blocking_threads(1)`
  饱和队列测试验证 public future drop 在不释放 blocker 前就归还 queued closure 的
  lease clone、真实文件锁和自定义 permit。在复制描述符仍存活时，由独立子进程验证
  drop 前互斥、drop 后可加锁；最终排空后没有迟到写盘。
- public port 测试验证全部操作的 timeout code/retryable、跨子阶段累计预算、实际
  capacity/lease queue 等待、隐式 lease 预算复用、运行中 Git 超时；通过仅测试可用
  的时钟偏移和边界注入，验证实际 mkdir/init/commit/worktree 落盘后跨 deadline 的
  retained path/state。application 集成断言保留路径可见且没有注册记录。
- application 使用 fake facts 验证授权上限、canonical/lexical escape、CAS 重新取事实
  和 baseline 拒绝前后的持久化顺序；原 lease/recovery/approval 故障注入继续保留。
- NEC-252 的真实 Git 故障测试保留：CAS 后 HEAD/root 变化、规范目标替换、别名查重、
  准备一次与无重复事件。公开 adapter 测试另验证写入前目标重绑定拒绝、只读 baseline
  不刷新 index 的字节/mtime。
- NEC-205 的异步接纳验证恢复 500ms 原阈值；application 进度测试注入即时 workspace
  事实以隔离并行 Git 进程调度，继续以未释放的 Agent 信号量证明接纳不等待 turn 完成。
  原有真实 adapter 集成、lease/fencing/recovery 回归继续执行。
- 交付检查：`cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、
  `cargo test --workspace`。实际结果记录在 PR/issue。
