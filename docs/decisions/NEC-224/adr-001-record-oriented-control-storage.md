## ADR-001：按记录持久化控制面状态并移除 Workspace Snapshot

- 状态：Accepted
- 日期：2026-09-08
- 依赖：NEC-150 核心领域模型 v4、NEC-146 双层 SQLite 设计
- 替代：NEC-152、NEC-162、NEC-166 中公开 Workspace Snapshot 的读接口，以及运行时单体 `control_state.body_json`
- 来源：NEC-224

## 决策

1. 控制面存储 port 改为 `read(filters)` 与 `apply(expected_revision, changes, events)`。
   application 每次只读取命令的目标记录与被引用记录；只有明确的 list/export 操作扫描记录族。
   Session/Run/Cron 关系通过有界选择器读取，Agent Provider 更新只扫描引用该 Provider 的 Agent，
   Agent prompt 只读取 `base_message_id` 的祖先链，不再把“单个 Project”当作读取聚合边界。
2. SQLite 使用具名 STRICT 表保存 Project、Agent、Provider、credential reference、Session、Message、Run、
   Run credential、workspace journal、Cron 和 Settings。Project 范围记录带 `project_id` 与索引；JSON 只作为单条记录的
   payload，不再作为包含所有集合的数据库根对象。
3. Message 行保持 append-only。已存在 Message 的更新和删除由数据库 trigger 拒绝；parent 外键在同一事务结束前校验。
4. 行级变更、全局 revision CAS 与 durable event outbox 在一个 SQLite 事务中提交。冲突仍返回稳定的 `Conflict`，
   不以最后写入覆盖并发变更。
5. 删除 `Snapshot` command、`WorkspaceView`、`GET /v1/workspace/snapshot`、CLI `snapshot` 与桌面 IPC snapshot。
   读取改为 Projects、Agents、AgentProviders、Sessions、Messages、Runs、Crons 的独立 list operation；
   Messages/Runs 必须给出 Project，桌面只为当前 Project 读取历史与运行记录。
6. 打开旧数据库时，若存在 `control_state`，先把整个 blob 升级到当前 Agent/Provider/Run schema，再在同一个迁移事务中
   把各集合拆成具名记录、继承 revision，最后删除旧表。禁止先拆旧 schema、再依赖某次 application 局部读取顺带升级，
   因为那会永久形成新旧记录混合状态。迁移不把 secret 明文写入数据库；现有 credential reference 边界保持不变。

## 与 NEC-146 的关系

本次实现采用 NEC-146 的所有权划分：Project registry、Agent/Provider、Cron/Settings 属于全局目录，
Message、Session、Run、run credential 与 workspace journal 都带明确 Project 归属。这个记录边界使后续把
Project 范围表移动到 `<project>/.ait/project.sqlite3` 时无需再次拆解单体 JSON。

当前 daemon 的 `--database` 仍指定一个物理 SQLite 文件；本次不伪造跨文件原子事务，也没有宣称已经完成
NEC-146 的全局文件 + 每 Project 文件路由。物理拆库需要按 NEC-146 的 saga、project identity、独立备份与
secret 文件协议单独交付。在那之前，逻辑边界、查询边界和表边界已经与该设计对齐。

## 迁移与兼容性

- 旧 `control_state` 只在数据库首次打开时读取一次；schema 升级、记录拆分与旧表删除全部成功后才提交。
- 公开 API 尚未发布，不保留 snapshot 兼容别名；旧客户端必须改用实体 list operation。
- 历史 Run 中的非秘密 Agent 配置仍保存在 Run 记录内；这属于历史事实快照，不是 Workspace 聚合接口。
- SQLite Online Backup、恢复、quick check、event retention 与 progress checkpoint 的既有能力保留。

## 验证

- SQLite 测试覆盖 Project/关系范围读取不会返回无关记录、Message 更新被拒绝、在线备份和事件裁剪竞态。
- application、HTTP、CLI 与 desktop 测试全部通过独立 list operation 观察结果，不依赖被删除的 Snapshot。
- 旧 blob 迁移测试验证多个旧 Agent 在拆表前同时完成升级；更新其中一个并重启后，所有 Agent 仍使用同一 schema，
  revision 保留且 `control_state` 被移除。
- application 回归测试验证同 Project 内无关坏记录不会阻断按 ID 的 Session 更新。
