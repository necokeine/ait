# ADR-015：Workspace 能力与本机适配器独立成 crate

- 状态：Accepted
- 日期：2026-09-19
- 修订：NEC-154 的 crate 布局、NEC-250 的遗留接口保留条款、NEC-253 的 port 所在位置
- 保持：NEC-150 v4、NEC-209、NEC-212、NEC-253 的行为与一致性语义

## 背景

Project 的规范路径、Git baseline、Session worktree、写入 lease 和默认目录创建是一组内聚的
Workspace 能力。它们原先与 ControlStore、Provider、Run execution 等横跨系统的协议一起放在
`ait-ports`，本机实现则使用较窄的 `ait-project-local` 名称。与此同时，旧的同步
`ProjectEnvironment`、`LocalProjectEnvironment` 和 `ProjectPathGuard` 已没有生产消费者，形成了
与异步 `ProjectWorkspace` 重叠的第二套边界。

Run 执行协议中的 `WorkspaceAgent`、`WorkspaceRunRequest` 和 `WorkspaceRunResult` 虽然也使用
Workspace 命名，但描述的是一次 Agent Run 的输入、进度和输出，不是文件系统 Workspace 能力。

## 决策

1. 新增 `ait-workspace`，拥有 `ProjectWorkspace`、`WorkspaceLease`、`GitBaseline`、
   `WorkspacePathFacts` 和 `ProjectDirectoryCreator` 契约，以及可供 adapter 复用的 contract kit。
2. `ait-project-local` 重命名为 `ait-workspace-local`。它实现 `ait-workspace` 的契约，封装本机
   文件系统、Git、Session worktree、跨进程 lease 和 Documents 目录分配。
3. application 依赖 `ait-workspace` 抽象；daemon 等 composition root 同时依赖
   `ait-workspace-local` 并注入具体实现。`ait-workspace` 不依赖 application、storage、provider、
   HTTP、IPC 或 UI。
4. 从 `ait-ports` 删除上述 Workspace 文件能力。ControlStore、Provider、Clock、worker 与 Run
   execution 协议继续留在 `ait-ports`；尤其保留 `WorkspaceAgent` 等 Run 协议，不因同名概念迁移。
5. 删除没有生产消费者的同步 `ProjectEnvironment`、`EnvironmentError`、
   `LocalProjectEnvironment` 和 `ProjectPathGuard`。不保留兼容 facade；所有生产 Workspace 操作继续
   使用 NEC-253 定义的异步、可取消、具名能力。

依赖方向为：

```text
application -> ait-workspace <- ait-workspace-local
                               <- daemon composition root

runtime/application -> ait-ports <- execution/storage/provider adapters
```

## 兼容性与后果

这是 Rust 内部 crate 与源码边界变更。公开 HTTP/CLI/Desktop contract、数据库 schema、持久化记录、
错误码、Git/worktree/lease 行为、Run 状态机和恢复语义不变。Rust 内部调用方需要将
`ait_ports::*Workspace*` 导入改为 `ait_workspace::*`，并将本机 crate 名改为
`ait_workspace_local`。

独立 crate 让 Workspace 能力可以按自身语义演进，也让 `ait-ports` 保持为真正跨能力的协议集合。
代价是 composition root 多一个显式依赖；这是所需的依赖可见性，而不是重复抽象。

## 验证

- `ait-workspace-local` 运行 `ait-workspace` 的 contract kit，并保留真实 Git、路径、lease、取消和
  deadline 测试。
- application 集成测试继续通过 fake `ProjectWorkspace` 验证授权、事务顺序、恢复和 retained path。
- 交付执行 workspace format、build、clippy、tests 与 `cargo llvm-cov --workspace --html`。
