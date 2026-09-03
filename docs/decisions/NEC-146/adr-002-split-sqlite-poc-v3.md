## ADR-002：系统目录 + Project `.metafab` 双层 SQLite POC

- 状态：Proposed，待确认
- 修订：v3
- 日期：2026-09-02
- 替代：NEC-146 v1/v2 持久化附件
- 领域基线：NEC-150 `adr-001-core-domain-model-v4.md`
- 配套 DDL：`schema-global-poc-v3.sql`、`schema-project-poc-v3.sql`
- 来源：NEC-146
- 配套验证：`schema-split-poc-v3-smoke.sql`

## 1. 决策摘要

1. POC 不再使用一个 SQLite，而是一个系统级 SQLite + 每个 Project 一个 SQLite。
2. 系统级目录固定为用户 Documents 下的 `metafab`，保存 Project registry、Agent/revision、Cron/fire 和全局 config/secrets。
3. 每个 Project 根目录下创建 `.metafab/project.sqlite3`，保存该 Project 的 Message、Session、Run、ToolExecution、附件、checkpoint 和事件。
4. Project DB 不依赖系统 DB 才能读取历史：它保存 `project_identity` 和每个 Run 的非秘密 Agent revision snapshot。
5. 跨 SQLite 没有外键，也不尝试用一次跨库事务伪造强一致。Cron → Project Run 使用可重试、幂等的 saga；崩溃后可以确定性补偿。
6. 全局连接 secrets 存在 `Documents/metafab/secrets.toml`，不进入任一 SQLite。目录权限为 `0700`，secret 文件为 `0600`。
7. POC 仍不做任何数据/备份 GC。两个层级分别用 SQLite Online Backup API 生成单文件快照。

## 2. 文件布局

```text
<Documents>/metafab/
  metafab.sqlite3                 # 全局 catalog/config state
  config.toml                     # 全局非秘密 config
  secrets.toml                    # Agent/provider connection secrets
  backups/
    global-2026-09-02.sqlite3
    global-2026-09-03.sqlite3

<project-root>/
  .git/
  ... project files ...
  .metafab/
    project.sqlite3               # 本 Project 全部对话与运行历史
    backups/
      project-2026-09-02.sqlite3
      project-2026-09-03.sqlite3
```

创建/注册 Project 时：

1. 保证 Project root 是 Git top-level；必要时 `git init`。
2. 创建 `.metafab`，权限默认 `0700`。
3. 创建 `.metafab/project.sqlite3` 并写唯一 `project_identity`。
4. 在系统 `metafab.sqlite3.projects` 注册 `project_id + root_path`。
5. 将 `/.metafab/` 写入 `.git/info/exclude`，默认不污染 Project tracked files，也不把频繁变化的 SQLite 提交进 Git。

普通文件系统复制 Project root 时 `.metafab` 随目录一起移动，历史仍在；Git clone 默认不带 `.metafab`，会得到一个没有旧 Session 历史的新本地 Project 实例。这两个语义需要在 UI 中明确区分。

## 3. 系统级边界

系统数据库位置：

```text
<Documents>/metafab/metafab.sqlite3
```

表：

| 表 | 作用 |
|---|---|
| `schema_migrations` | 全局 schema version/checksum |
| `projects` | Project ID、名称、描述、root path、状态和 `.metafab` DB 相对路径 |
| `agents` | 全局 Agent identity/enabled |
| `agent_revisions` | driver、connection name、model、capability、parameters、tool policy |
| `project_agent_defaults` | Project 默认 Agent |
| `crons` | 固定 Project + base Message + Agent 的调度配置 |
| `cron_fires` | `(cron_id,scheduled_at)` 幂等 claim 与本地 Run 结果 |
| `global_events` | Project/Agent/Cron 的全局审计事件 |

这里不保存 Message、Session、Run 或 ToolExecution，也不保存任何 secret 值。

`projects.project_db_relative_path` 在 POC 固定为 `.metafab/project.sqlite3`。绝对 `root_path` 只用于当前机器定位；Project 移动后通过本地 `project_identity.project_id` 重新绑定 registry path。

## 4. Project 级边界

Project 数据库位置：

```text
<project-root>/.metafab/project.sqlite3
```

表：

| 表 | 作用 |
|---|---|
| `schema_migrations` | Project schema version/checksum |
| `project_identity` | 单行 Project ID 与本地格式版本 |
| `project_instruction_revisions` | Project 指令来源与 render snapshot |
| `attachments` | 本 Project 附件 BLOB |
| `messages` | append-only Message forest，内容直接存 `content_json` |
| `message_attachments` | FileRef → in-DB attachment 约束投影 |
| `sessions` | 可移动 Message ref + version CAS |
| `runs` | base/last Message、Agent snapshot、状态、预算、queue version |
| `run_attempts` | initial/retry/recovery 记录 |
| `run_queue_items` | Run 内追加工作与 dedupe |
| `run_checkpoints` | 本地恢复 state BLOB |
| `run_branch_conflicts` | Session CAS 冲突保留分支 |
| `tool_executions` | ToolUse/ToolResult 控制与审计 |
| `run_events` | durable Run event stream |

Project DB 内没有 `projects`、`agents`、`crons` 或 secrets 表。

### 同 Project 约束

由于一个 Project DB 物理上只容纳一个 Project，Message parent、Session ref、Run base/last、ToolExecution 和附件引用只可能指向本文件中的行。同 Project 不变量由存储边界本身保证，不再需要每张表重复 `project_id`。

`project_identity` 恰有一行：

```text
singleton=1, project_id=<global registry ID>, format_version=1
```

打开 Project 时先比对系统 registry ID；不一致直接拒绝写入，并提供“重新绑定/作为新 Project 导入”两个显式操作。

## 5. Message、Session 与 Run

Message 仍直接是一张表：

```text
messages {
  id, parent_message_id,
  role, message_kind, origin,
  content_json, content_digest,
  created_by_session_id?,
  run_id?, run_seq?,
  tool_result_call_id?, tool_result_status?,
  metadata_json, created_at_ms
}
```

核心约束保持不变：

- parent 必须在 child insert 前存在；
- parent edge 创建后不可更新，Message 不可更新/删除；
- 根必须是 System Message；
- 旧图不可改边，新节点只能指向旧节点，因此无环；
- Session pointer 只能用 `version+1` CAS 前进到直接子节点；
- partial unique index 保证一个 Session 只有一个非终态 Run；
- Run 输出必须是 `coalesce(last_message,base_message)` 的直接子节点，`run_seq` 连续唯一；
- Session CAS 冲突时保留 Message 与 conflict record，但不移动 Session/Run head；
- ToolExecution 校验 assistant Message JSON 中的 index/call_id/tool_name；
- 每个 ToolUse 最多一个最终 ToolResult user Message。

## 6. Agent 跨库快照

Agent 定义和 revision 位于系统 DB，但 Run 位于 Project DB。Run 创建时：

1. 从系统 DB 读取并校验 enabled Agent revision；
2. 只解析非秘密字段：agent_id、revision、driver、connection_name、model、capabilities、parameters、tool policy；
3. 生成稳定 `agent_snapshot_json + digest`；
4. 在 Project DB 的 Run 创建事务中保存 snapshot；
5. provider 真正调用时再从全局 config/secrets 解析 connection。

这样复制 Project 目录后，即使接收方没有原系统 DB，也能理解历史 Run 当时用了哪个 Agent 配置；但因为没有 secrets，不能自动获得原机器的 provider 权限。

Snapshot 严格使用字段 allowlist，不得包含 API key、token、header 或完整外置 config block。

## 7. 全局 config 与 secrets

系统级 config 都放在：

```text
<Documents>/metafab/config.toml
<Documents>/metafab/secrets.toml
```

建议内容：

```toml
# config.toml：可读的非秘密全局配置
[storage]
global_database = "metafab.sqlite3"
project_database = ".metafab/project.sqlite3"

[backup]
enabled = true
hour = 3

[connections.openai_main]
driver = "openai_compatible"
base_url = "https://api.openai.com/v1"
secret_name = "openai_main"

[runtime]
max_parallel_runs = 4
```

```toml
# secrets.toml：不进入 SQLite、不进入 Project
[connections.openai_main]
api_key = "replace-with-real-secret"
```

安全规则：

- `Documents/metafab` 创建后立即设为仅当前用户访问；POSIX 为目录 `0700`、`secrets.toml` 为 `0600`。
- `config.toml` 可以备份或同步；`secrets.toml` 默认不进入普通 SQLite 备份、不随 Project 分享。
- 若用户要备份整个系统 Metafab 目录，必须明确提示其中含 secrets；推荐生成加密 archive，而不是明文复制到云盘。
- 日志、Agent snapshot、错误、Run event 和 crash dump 都不能序列化 secret。
- 允许 secrets 值改用 `${ENV_VAR}`，但 config 文件仍留在同一个 Metafab 目录。

## 8. 跨数据库一致性

SQLite 外键不能跨数据库文件。即使运行时 `ATTACH`，WAL 下也不能把多文件提交当作可靠的统一原子事务。因此正常业务使用两个独立连接和显式协议，而不是跨库 FK/transaction。

### 创建手动 Run

```text
read global Agent revision
  -> build non-secret snapshot
  -> BEGIN Project DB
       validate Session/base Message
       insert Run(snapshot, dedupe_key)
       claim Session
     COMMIT
```

系统 DB 没有对应的 Run 行，因此不需要双写。

### Cron fire saga

```text
1. Global TX:
   INSERT cron_fires(cron_id, scheduled_at, state=claimed)
   -- UNIQUE(cron_id,scheduled_at) 是唯一 claim

2. Validate:
   open registered Project DB
   project_identity == Cron.project_id
   base_message_id exists
   Agent enabled; build non-secret snapshot

3. Project TX:
   INSERT Run(
     id=<deterministic/local id>,
     dedupe_key="cron:<cron_id>:<scheduled_at>",
     trigger=cron, base_message_id, agent snapshot
   )

4. Global TX:
   UPDATE cron_fires SET state=started, local_run_id=<id>
```

崩溃恢复：

- 第 1 步后崩溃：扫描 `claimed`；Project DB 无 dedupe key 时继续创建。
- 第 3 步后崩溃：扫描 `claimed`；按 dedupe key 找到已有 Run，只补第 4 步。
- Project 不可用：fire 记 `blocked/failed`，不静默换 Project 或 Message。
- 重复 scheduler：全局 fire PK 和 Project Run dedupe 两层都阻止重复执行。

### Project 创建/移动

创建时先完整创建 Project DB 与 identity，再注册系统 DB。若注册前崩溃，`.metafab` 是“未注册 Project”，下次打开时可发现并注册。

移动时不修改 Project DB；打开新路径后读取 identity，通过系统 DB CAS 更新 `root_path`。旧路径同时存在时必须让用户选择哪个副本继续，不能两个目录同时以同一 Project ID 写入。

## 9. 备份

两个 SQLite 分别备份：

```text
每天 03:00
  <Documents>/metafab/metafab.sqlite3
    -> <Documents>/metafab/backups/global-YYYY-MM-DD.sqlite3

  每个已注册且可访问的 <project>/.metafab/project.sqlite3
    -> <project>/.metafab/backups/project-YYYY-MM-DD.sqlite3
```

统一使用 SQLite Online Backup API/`.backup`，完成后执行：

- 正确 `application_id` 与 `user_version`；
- `integrity_check=ok`；
- `foreign_key_check` 无结果；
- Global：Project/Cron/Agent 引用审计；
- Project：identity、Message cycle、Session/Run/Tool 不变量审计；
- fsync 临时文件与父目录，再原子 rename。

POC 不自动删除任何备份。

### 一致恢复

Project 历史恢复只需恢复对应 Project snapshot；其 Agent snapshot 足以读取历史。恢复后重新校验/绑定 global registry。

整机恢复时，global snapshot 与各 Project snapshot 不保证是同一个原子时间点。恢复完成后运行 reconciliation：

- registry path/identity 对齐；
- `cron_fires.started.local_run_id` 在对应 Project DB 存在；
- `claimed` fire 按 saga 续跑；
- Project 中找得到 cron dedupe Run、Global 尚未 started 时补写 fire；
- 不可修复引用进入 blocked report，不删除任何记录。

### 配置备份

每日 SQLite backup 不包含 `config.toml/secrets.toml`。`config.toml` 可普通复制；`secrets.toml` 如果要备份，使用单独的加密 archive。复制整个 `<Documents>/metafab` 目录会包含 secrets，UI/CLI 必须明确警告。

## 10. 可移植性与分享语义

### 分享 Project

生成 Project DB snapshot 或复制包含隐藏目录的 Project root，即可带走：

- 全部 Message/Session/Run/Tool/Cron-triggered Run 历史；
- 附件、checkpoint 和 event；
- 每个 Run 的非秘密 Agent snapshot。

不会带走：

- 全局 Project 名称/描述以外的 registry 状态；
- Agent catalog 当前版本；
- Cron 配置与 fire catalog；
- `config.toml/secrets.toml` 与 provider 执行权限。

接收方打开 Project 时读取 identity，若系统 DB 没有该 ID，则执行显式“注册已有 Project”。

### 分享系统目录

`metafab.sqlite3` 本身没有 secrets，但缺少 Project DB 时只能看到 registry/Agent/Cron 上层信息。整个 `<Documents>/metafab` 目录包含 secrets.toml，不应作为普通可分享数据包。

与之前一样，“没有系统凭证”不代表 Message 内容公开安全：用户输入、工具结果、附件、路径仍可能敏感，分享前需要内容确认或 secret scan。

## 11. Migration

全局和 Project DB 有不同的 `application_id`：

```text
Global:  "MFG1" / schema-global migrations
Project: "MFP1" / schema-project migrations
```

两套 `user_version + schema_migrations` 独立向前迁移：

- daemon 启动先迁移 global DB；
- Project 第一次打开时按需迁移该 Project DB；
- Project migration 前在该 Project `.metafab/backups` 生成 snapshot；
- 新程序遇到更高版本 DB 拒绝写入；
- 不支持 down migration，降级通过恢复备份；
- 已发布 migration checksum 不可修改。

这样一个长期未打开的 Project 不会阻塞系统启动，也不会因为 global schema 升级被无条件批量改写。

## 12. POC 不做 GC

- Global Project/Agent/Cron/event 不自动删除；
- Project Message/Session/Run/Tool/attachment/checkpoint/event 不自动删除；
- archive 只改状态；
- global/project backups 都不自动清理；
- 不做 VACUUM reclaim、retention、quarantine 或 payload purge。

空间使用会分别反映到 global DB 和各 Project `.metafab`。先用真实 POC 数据测量，再决定附件外置、备份保留和 GC。

## 13. 验证结果

`schema-split-poc-v3-smoke.sql` 已通过本机 SQLite：

1. 创建 Global DB：Project registry + Agent revision；
2. 创建 Project DB：identity + System/User/ToolUse/ToolResult Message + Session + Run + attachment/event；
3. `ATTACH` 仅用于测试审计，验证 Global Project ID 与 Project identity 一致；
4. 模拟 Cron `claimed → Project dedupe Run → started` saga；
5. 两个 DB 的 `foreign_key_check` 均为空，`integrity_check` 均为 `ok`；
6. 分别生成两个 `.backup` 文件并独立重新打开，Global registry 与 Project Session 历史可正确读取。

进入实现仍需增加：Project 移动/重复副本处理、Cron 每个崩溃窗口、并发 Session CAS、Project DB 按需 migration、Windows/macOS/Linux Documents 路径解析、config 权限和 secrets 泄漏测试。

## 14. 当前取舍

- 系统级数据与 Project 历史物理分离，符合“上层 catalog 全局、运行历史随 Project 走”。
- 牺牲跨库外键/原子事务，换取 Project 可复制性；用 identity、snapshot、dedupe 和 reconciliation 补足。
- secrets 放用户指定的 Metafab config 目录，部署简单，但比 OS keychain 更依赖文件权限和加密备份；POC 明确接受并记录这一风险。
- `.metafab` 默认 Git-excluded，但普通目录复制会保留；Git clone 与目录复制的历史语义不同。
- POC 不引入 GC、FTS 或外置附件库，先验证基本产品模型。
