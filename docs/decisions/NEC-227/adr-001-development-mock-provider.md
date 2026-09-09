# ADR：开发构建专用 Mock Provider

- 状态：Accepted
- 日期：2026-09-09
- 来源：NEC-227
- 修订：NEC-203 的生产 Provider 身份清理

## 决策

新增非默认 Cargo feature `dev-mock-provider`，并要求它与 Rust `debug_assertions` 同时成立。
只有桌面开发启动路径显式开启该 feature；打包应用使用的 daemon 和 release 构建即使误开 feature
也不包含 Mock。可信的 Electron main 边界还会在 production 模式下剔除 daemon payload 中的 Mock
Provider 及引用它的 Agent，Settings、Agents 页面与 composer 只接收过滤后的目录。`ProviderKind::Mock`、`builtin-mock`
目录项及其执行分支都在同一个编译边界内，因此 production 二进制的领域枚举、HTTP DTO、
Provider 目录和 archive 反序列化类型均不包含 `mock`。

Mock 提供固定模型 `mock-local` 和固定答复 `Mock assistant response.`。它不接受凭证、模型发现、
刷新或目录写入，也不能导入或导出 portable Project archive。开发端只能选择内置目录项，不能通过
公共保存接口创建另一个 Mock 连接。

## Run 与 Message 语义

提交输入仍先按正常事务追加 human user Message、推进 Session 并创建 queued Run。后台 supervisor
照常认领 Run、进入 running/settling，并通过统一 finalization 路径追加 assistant Message、推进
Session、释放 active Run 后写入 completed。唯一替换的是 provider invocation：Mock 分支直接返回
本地确定性 `WorkspaceAgentResponse`，不接触 Codex workspace harness、远端 Provider gateway、凭证或网络。
首轮交互后的自动 Session 标题请求也在 application 层识别 Mock：保留 renderer 已写入的本地临时标题并
标记生成流程完成，不调用 Codex title generator。

这不是恢复 NEC-203 删除的 `apply_run_mode`：Mock 不在命令提交阶段伪造 Run 状态，也不绕开统一
终态持久化。测试在未注入任何真实 adapter/harness 的服务上执行完整交互，关闭 service 和文件型
SQLite store 后重新打开同一路径，验证 user/assistant Message、Session 指针和 completed Run 可重新读取。

## 生产与归档边界

默认 feature 集合就是 production 契约。默认构建对包含 `"kind":"mock"` 的 HTTP payload 与 archive
在反序列化阶段失败，fresh Provider 目录只含既有 `builtin-codex`。即使开发构建能够表达 Mock，
application 仍拒绝保存、发现、刷新以及包含 Mock 的 archive 导入/导出，避免将开发身份变成可携带配置。

开发 profile 中产生并引用 Mock 的本地运行历史只保证由同 feature 的开发 daemon 读取；它不是可迁移的
生产数据。desktop 默认将开发 daemon/数据库固定为 `127.0.0.1:7315` 与
`ait-development.sqlite3`，production 则使用 `127.0.0.1:7314` 与 `ait.sqlite3`。两种模式都拒绝复用
端口上已有且无法验证构建身份的 daemon，避免反向连接到错误 profile；清理开发数据只能删除开发数据库
及其 WAL/SHM companion，不能改名或复制到 production 路径。
