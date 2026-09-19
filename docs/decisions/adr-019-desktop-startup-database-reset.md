# ADR-019：桌面启动失败时删除旧本地数据库

- Status: Accepted
- Date: 2026-09-20

## 背景

旧 catalog 导致 daemon 以 `LEGACY_RECOVERY_REQUIRED` 退出时，Desktop 原先只显示内部错误
和重新编译提示。用户选择放弃旧数据库，从空库重新开始，不要求备份、迁移或兼容旧数据。

## 决策

Desktop 增加仅用于启动恢复的 `startup.recovery`、`startup.reset-database` 和
`startup.retry` IPC。恢复信息与删除操作不依赖 daemon 成功启动，不提供 HTTP 数据删除端点。
只有本 Desktop 启动的子进程关闭且报告旧格式错误后，主进程才开放删除入口；其他启动错误
仍提供重试和错误详情。失败状态保留，普通请求不会反复启动已知不兼容的 daemon。

主进程根据当前 profile 固定目标为 `userData/ait.sqlite3`（发行版）或
`userData/ait-development.sqlite3`（开发版），renderer 不能传入路径。删除前由原生确认框
说明不可撤销、无备份及丢失范围，默认取消。确认前后都要求本应用无启动中或运行中的 daemon，
并检查当前 endpoint 无健康 daemon；确认和删除期间阻止其他启动及重复删除。

删除范围只包含 catalog 本体和对应 `-wal`、`-shm`、`-journal` 文件。先验证现有目标是普通
文件，拒绝符号链接和目录；不递归删除目录，不删除锁文件、其他 profile、项目 `.ait`、原生
Codex 数据或工作区文件。文件缺失允许继续，删除失败可重试；成功后重载界面启动空库。

这是对 ADR-004 的离线文件删除例外：Electron 不解析 SQLite、不读写业务记录、不实现迁移；
正常持久化、初始化和领域规则仍由 Rust daemon 负责。此操作放弃整个指定 catalog，包括其中
任何旧历史，不是修改 Message 内容。现有显式升级工具不受影响。

## 限制与后果

配置、Provider、Agent、项目注册及 catalog 内的历史会丢失。项目库留在原处，但旧格式项目
仍可能无法打开，删除原 catalog 也会放弃依赖它的旧格式转换路径，界面不承诺旧项目可恢复。

确认框要求先关闭其他 Ait 实例、daemon 和 worker。当前 endpoint 健康检查不证明其他端口或
旧版本进程已经停止，不能用于热删除正在使用的数据库。删除多个文件不是原子操作，中途失败
可能已删除部分文件；重试继续清理，成功前不启动新 daemon。不自动备份或修复旧数据。
