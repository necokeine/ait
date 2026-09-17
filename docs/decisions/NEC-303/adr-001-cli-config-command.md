# ADR-001：CLI 设置入口统一为 config

- 状态：Accepted
- 日期：2026-09-17
- 关联：NEC-303
- 修订：NEC-241 类型化实体 CLI

## 决策

CLI 将顶层 `settings get|set|reset` 改为 `config get|set|reset`，不保留旧入口的隐藏别名。
三个动作继续分别映射到 application 的 `GetSettings`、`SaveSettings` 和 `ResetSettings`；完整 values、
revision CAS、默认值与权限快照语义保持不变。

本次只调整 CLI 命令名。HTTP `/v1/settings` 路由、application Command、持久化记录、事件名以及
Desktop 的 Settings 用语不变。

## 验证

参数映射测试覆盖 `config get|set|reset`，递归帮助必须列出 `config`，旧 `settings get` 必须解析失败。
真实 CLI 到 HTTP/SQLite 的设置保存、冲突拒绝、重启恢复与重置流程全部使用新入口继续验收。
