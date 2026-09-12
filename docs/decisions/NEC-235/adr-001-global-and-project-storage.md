## ADR-001：全局目录与 Project 历史物理拆分

- 状态：Accepted
- 日期：2026-09-12
- 来源：NEC-235
- 基线：NEC-150 v4、NEC-146 双层 SQLite 设计、NEC-224 按记录持久化

## 存储边界

daemon 使用 `SplitSqliteControlStore`。`--database` 及 Desktop 现有全局路径保持兼容，
现在只定位全局 catalog；本次不同时迁移用户的全局目录位置或凭据后端。
原单文件 adapter 保留给内存测试和旧格式迁移测试，不用于 daemon 的持久化路径。

| 位置 | 持久化内容 |
| --- | --- |
| `--database` | Project registry、Agent、Provider、Provider credential reference、Cron、Settings、revision、实体路由索引和事件游标索引 |
| `<project>/.ait/project.sqlite3` | Message、Session、Run、Run credential reference、workspace journal、progress checkpoint、Project 事件正文和非秘密 Run 配置快照 |

全局路由索引只保存实体种类、ID 和 Project ID。Project 事件正文在项目库中，全局只保存
cursor、kind、entity ID、时间和 Project ID；Project/Agent/Provider/Cron/Settings 目录事件仍在全局。
公开 SSE cursor 连续且保持原有保留窗口；项目库的历史事件不自动 GC。
凭据仍由现有凭据端口管理，SQLite 只保存 reference，不引入秘密值副本。

Project DB 相对路径固定为代码常量 `.ait/project.sqlite3`，不存入数据库，也不作为配置项；
运行时由 Project 根目录拼接得到。项目存储目录统一为根目录下的 `.ait/`；`project.sqlite3` 与 ADR-013 规定的
`<session-id>/` 工作树共用此目录。数据库名及其 `-wal`、`-shm`、`-journal` 文件名保留，
不能用作 Session ID，以免工作树占用存储路径。本次目录更名不提供旧目录探测、搬迁或兼容回退。

每个 Project 库包含唯一 `project_identity`、协调器 ID、独立 `application_id=AIP1` 和
`user_version=1`；全局使用 `AIG1`。未知或更高格式拒绝写入。
Project 表外键指向本库 identity，Message parent 和 workspace journal 的 Run 引用保留本库约束；
Message 更新/删除仍由 trigger 拒绝。应用层领域校验保持不变。

首次注册时验证 Git root，将 `/.ait/` 追加到 Git 的 `info/exclude`，保留已有规则，
拒绝已跟踪的 `.ait` 文件及指向别处的存储符号链接。新目录在 Unix 上使用 `0700`。
已注册 Project 的数据库缺失时不自动重建。不同 Project ID 或不同全局协调器不得接管现存库。

## 跨文件提交与恢复

继续保留 `ControlStore::read/apply` 的全局 revision CAS，避免改动 Session/Run/Cron 的业务协议。
不同进程通过规范化全局路径旁的 `.lock` 文件串行化访问；不同文件各自使用 WAL + FULL synchronous。
不使用 `ATTACH`，也不声称跨 WAL 文件有单一原子事务。

1. 在锁内检查 revision，并验证全部全局变更。
2. 在各 Project 库中验证完整本地事务（包括外键），回滚验证，再持久化 prepared batch。
   batch 保存本地记录、事件和 checkpoint；准备阶段不修改业务记录。
3. 全局持久化唯一提交决定：operation ID、目标路径、全局变更、实体路由和无正文的项目事件索引。
4. 各 Project 在本地事务中应用 batch，并写入 `last_operation`、删除 prepared batch。
5. 全局事务提交目录/路由/事件索引与新 revision，并删除提交决定。

每次通过存储端口读写前都先恢复已决定的提交。第 3 步前中断不提交业务变更；第 3 步之后
必须继续完成。`last_operation` 使第 4 步重放不重复插入 Message、事件或推进状态。
提交已决定但项目暂不可访问时，拒绝通过端口暴露部分提交；恢复目录/访问权限后重新打开即可续完。
不要把这类失败当作“肯定没有写入”并手工重复触发业务。

此实现以一个本地协调器串行提交为取舍，尚不提供独立 Project 写并发或分布式事务。
不支持两个不同全局 catalog 同时写同一 Project。目录复制及 Project 在线备份可独立用 SQLite
读取历史；跨 catalog 使用应用现有导出/导入，在新的目标目录创建 Project。
原地移动/rebind 和直接接管复制库不是本次新增的公开操作。

## 旧数据迁移

旧单文件库先通过 SQLite Online Backup 生成旁路 `*.pre-split.sqlite3` 备份。
旧 `control_state` 先完成已有 schema 升级，再按 Project 生成本地 batch。
迁移通过相同提交协议保留 revision、Message/Session/Run ID、credential reference、journal、
progress 和原事件 cursor。仅所有项目提交成功后，才在全局事务中移除旧项目业务表并标记迁移完成。
迁移失败保留源记录和备份，修复路径/权限后可重试；首次拆库需要所有源 Project 路径可用。
迁移备份及 SQLite 旧页可能仍包含迁移前的历史，本次不做安全擦除或自动删除备份。

完成迁移后按需打开项目。目录/Agent 查询不扫描项目库；启动恢复逐 Project 扫描，单个项目
不可用时返回并记录该 Project 的错误，其余项目继续恢复。跨项目显式 list/export 或全局事件回放
若需要缺失项目的数据，仍报错，不静默返回不完整结果。

## 备份与验收

全局使用 `backup_global_to`，单个项目使用 `backup_project_to`。两者均为独立的 SQLite
Online Backup；全局备份不包含项目历史。整机恢复应停止写入并保存一套全局及项目备份，
不能任意混合不同时间点的路由索引、项目库和待提交决定。既有归档 export/import 保持其原有范围。

回归覆盖物理表隔离、ID/Project/祖先/关系路由、Project 事件与 progress、Git 排除规则、
独立备份、迁移重试、游标继承、未知格式/身份拒绝、跨连接 CAS，以及准备完成、提交决定、
部分项目提交和全部项目提交后的恢复窗口。daemon、CLI 与持久化应用验收使用新的拆分存储。
