# ADR-043：独立 server 的 Skills 选择与文件安装

- 状态：Accepted
- 日期：2026-09-25
- 上游基线：Paseo `2c8e8a826810337492cc5a38bb0bbd705b6fb632`

Skills 的五个 `agent.skills.*` 接口由既有独立 `server-filesystem` 能力包拥有。协议、服务、存储端口和本地文件适配器分层；`server-api` 仅组装和分发，`server-bin` 决定目录。复用独立 server 的公共阻塞任务预算与串行服务锁，不引入旧 Ait domain/application/provider 组件，不改变 ADR-001 v4 的 Message、Session 或 Run 边界。

源目录由 `AIT_SERVER_SKILLS_BUNDLE` 指定，默认 `<data-dir>/skills-bundle`；目标固定为 `AIT_SERVER_SKILLS_HOME`（默认 HOME）下的 `.agents/skills`、`.claude/skills`、`.codex/skills`。WS 参数不能指定路径。目录规范化后拒绝根目录重叠，运行时拒绝符号链接及越界路径。

选择存放于 `<data-dir>/skills-state/selection.json`，不借用旧 daemon 配置或数据库。请求先恢复事务，再读取状态。删除由保存请求的 `confirmedRemovals` 确认；reconcile 只安装和更新，uninstall 为显式整体卸载。三个目标通过各自目录内的暂存与备份、独立状态目录内的事务日志协调；中断后下一次 Skills 请求恢复。外部文件冲突保留日志和备份，返回错误，不丢弃现场。

这不是三个目录同时可见的文件系统事务，也没有跨进程安装锁；同一宿主内串行操作。暂不自动更新启动时发现的安装，不内置含 Paseo CLI 调用的技能文本。应用应部署适配本 server 的技能包后再调用安装接口。完整行为、上游差异和验证记录见 [实施报告](../reports/server-skills.md)。
