# ADR-028：GitHub 仓库发现与独立 Project 克隆注册

- 状态：Accepted。
- 日期：2026-09-23。
- 来源：`getpaseo/paseo@2c8e8a826810337492cc5a38bb0bbd705b6fb632` 的
  `session.ts`、`github-service.ts`、protocol schemas 与对应测试。
- 范围：新 `server` binary 和独立 `server-*` crate；不复用旧 Ait 组件。

## 决策

接通 `workspace.github.search_repositories.request` 与 `project.github.clone.request`。
`server-protocol::github_projects` 保存 Paseo request/result 字段，仍使用新 server 的统一
request/response envelope；Paseo 顶层 `requestId` 不复制到业务 payload。

`server-ports::github_projects::GithubProjectsRuntime` 隔离 `gh`、`git` 和本地文件系统。
`server-workspace::LocalGithubProjects` 在 host HOME 调用 `gh repo list`（空查询）或
`gh search repos`（非空查询），并根据 `gh config get git_protocol` 选择 SSH/HTTPS clone URL。
命令有 30 秒发现时限、输出上限和无交互环境。`gh` 缺失、未认证与其他失败分别映射为 Paseo
的 `unavailable`、`unauthenticated` 和 `error` 状态。

克隆只接受 `owner/repo`（需显式 cloneProtocol）以及 `github.com` 的 HTTPS/SSH URL。
应用层验证 owner/repo 片段，adapter 在已规范化的 parent 下用临时目录执行最多 5 分钟的
`git clone`，成功后改名为最终 checkout，已有目标绝不被主动替换；失败清除 staging 目录。
完整 checkout 建立后调用现有 Paseo Project registry 注册，**不创建 Workspace**。注册失败时
保留已克隆目录，并以非空 `checkoutPath` 和空 `project` 返回，这与 Paseo 的可观察行为一致。
在克隆失败或目标已存在时也返回预先解析的 `checkoutPath`；如果目标路径本身无法解析则为 null。

`Directory` 通过 `DirectoryDependencies` 组合 registry、文件和 GitHub port；宿主在启动时
组装全新的本地 adapter。两个方法只有生产组装完成才进入 `implemented_capabilities`。

## 已知差异

Paseo 的 remote parser 也可接受其他已识别 Forge host；本次 GitHub 专用入口只允许
`github.com`，且拒绝 URL 的 userinfo、query、fragment、非默认端口与非 ASCII repository
片段。CLI 响应中的 clone URL 必须与 owner/repo 匹配。失败原因使用固定安全文本，
不会回传 Git/gh stderr；Paseo 对部分 CLI 错误会回传裁剪后的 stderr。

当前 API 的 business job 仍由全局有界阻塞任务串行运行；5 分钟克隆会占用一个 job lane。
如果以后需要并行大文件/克隆操作，应将长任务迁到单独的受监督队列，并定义请求断线与停机
时的完成回执。Git clone 的 staging 改名与其他进程同时创建最终路径之间仍存在普通文件系统
竞态；adapter 在改名前检查目标，但不声明跨进程 no-replace 原子保证。
