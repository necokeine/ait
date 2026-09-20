# ADR-001：移除 Project JSON archive 接口

- 状态：Accepted
- 日期：2026-09-20
- 来源：NEC-344
- 修订：NEC-166 的 Project import/export HTTP 路由、NEC-241 的同名 CLI 命令与快捷入口
- 依赖：ADR-018 的 Project 独立恢复与运行期独占接管

## 背景

Project 历史、会话与运行状态的持久化边界已经收敛到 Project 目录内的
`.ait/project.sqlite3`。ADR-018 又定义了以稳定 Project ID、目录锁、owner epoch 和本机配置重绑
打开既有 Project 的流程。旧 JSON archive 只投影部分记录，不能表达运行恢复、Cron、附件、原生
Codex Thread、凭据或当前所有权，继续公开会形成第二套不完整的恢复协议。

现有 HTTP API 尚未公开发布，因此不需要保留兼容别名。CLI 与 HTTP 是同一个 application
service 的公开适配器；只删除适配器而留下不可达的 archive command 和 transaction 会制造无消费者
协议与长期维护负担。

## 决策

1. CLI 删除 `project export`、`project import` 及顶层 `export`、`import` 快捷入口。
2. HTTP router 删除 `POST /v1/project/export` 与 `POST /v1/project/import`；请求返回普通 404，
   不增加已退役错误信封或兼容路由。
3. contracts 删除 `ExportProject`、`ImportProject`、`ProjectExport`、archive format version 和旧格式
   反序列化升级器；application 删除对应 read plan、transaction、校验、导入 worktree 准备和事件。
4. 已生成的 JSON archive 不再是受支持输入，也不提供隐式转换。打开或恢复 Project 时使用其原目录、
   `.ait/project.sqlite3` 和 ADR-018 的既有目录打开流程；灾备使用全局库与 Project 库的一致备份。
5. Codex 原生 Thread 的发现、导入与同步不受影响；其中“导入”指将原生 Thread 投影为 Ait Session，
   不是 Project JSON archive。

## 兼容性与验证

这是有意的删除性边界变更：旧 CLI 子命令由 clap 拒绝，旧 HTTP 路由返回 404，旧 tagged Command
也不再能反序列化。测试覆盖四种 CLI 写法及两条 HTTP 路由的负向行为；CLI 的 Command 映射表和
HTTP 路由表继续穷尽校验现有接口。

历史 ADR 与重构报告保留原决策记录；当前行为以本 ADR、ADR-018 和实时路由表为准。
