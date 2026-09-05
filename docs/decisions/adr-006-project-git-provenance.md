# ADR-006：Project Git 基线与 User Message 提交快照

- 状态：Accepted
- 日期：2026-09-05
- 修订：补充 ADR-001 v4 的 Project/Message Git 语义

## 背景

Message 树记录交互历史，但此前不能回答“这条用户输入基于哪个代码版本”。同时 Project 只保存本地目录，无法保留成员声明的远端 fork 来源。若在有未提交变更时接受输入，单一 HEAD 也不足以复现当时工作区。

## 决策

1. `Project.base_commit` 是必填、不可变的完整 Git object id，创建 Project 时从工作目录 HEAD 捕获。
2. 对尚无 HEAD 的新仓库，Project 注册流程创建一个不包含工作区文件的 manager-owned 空初始提交，再将其记录为 `base_commit`。
3. `Project.fork_repo_url` 是可选的声明型来源字段。它不参与 Project 身份，也不取代实际 Git remote 的刷新与校验。
4. 每个 `role=user, message_kind=standard, origin=human` Message 必须包含 `git_commit`。ToolResult、scheduler/system 输入和 assistant/system Message 不得复用该字段。
5. Send Message 与携带新 human 输入的 Fork Session 在任何领域写入前执行 Git 前置条件：index、tracked worktree 和 untracked files 全部为空，并读取完整 HEAD。
6. 为缩小外部 Git 并发造成的竞态，应用在 status 检查前后读取 HEAD；两次结果不一致则拒绝并要求重试。数据库乐观锁重试会重新执行 Git 检查。
7. Git 命令只存在于 application/adapter 边界。`domain` 仅拥有 `GitCommit` 值对象和聚合不变量，不依赖 Git、Tokio、SQL、HTTP 或 UI。

## 失败语义

- 工作区或 index 不干净：`PROJECT_GIT_DIRTY`。
- HEAD 不存在、不可读、格式无效或检查期间变化：`PROJECT_GIT_HEAD_UNAVAILABLE`。
- 上述失败不得追加 Message、创建 Run 或推进 Session。

## 兼容与归档

传输 DTO 对旧运行态快照保留反序列化默认值，以便读取并给出明确校验错误；Project archive format 升级到 v2，新 Project、v2 导出归档和新 human user Message 必须包含有效提交。导入 Project 时，目标本地仓库当前 HEAD 成为新的本地 `base_commit`，归档内不可变 Message 的 `git_commit` 保持原值。
