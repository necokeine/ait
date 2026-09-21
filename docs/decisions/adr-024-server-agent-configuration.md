# ADR-024：独立 server 的 Agent 配置与不可变 revision

- 状态：Accepted，落实已授权的 M1 Agent 配置切片。
- 日期：2026-09-22。
- 前置：[ADR-022](adr-022-independent-server.md)、[ADR-023](adr-023-server-project-opening.md)、
  [ADR-001 v4](NEC-150/adr-001-core-domain-model-v4.md)。仅适用于新 server。

## 1. 边界与能力

继续在全新 server 系列内部实现，不引用旧 Ait crate、Provider adapter 或旧 fixture。
新增生产代码分别归入 domain/ports/application/storage/protocol/API/binary；没有为了占位而
创建 execution/provider crate。Agent preset 属于本机 catalog，Project 不依赖 catalog 读取历史
的原则保持不变：未来创建 Session/Run 时仍需把实际使用的非秘密配置冻结到 Project。

本批新增 `agent.configure/get/list` 与 `agent.default.get/set`。配置 schema 首先支持 `codex`，
这只表示可以存储该类型配置，不表示存在可执行 adapter、已验证模型或已建立 provider 会话。
不发布 provider discovery、执行 capability，也不把测试 fake 加入生产 driver 类型。

## 2. 类型和不可变配置

Agent 有稳定 UUID 和当前 revision 指针；每个 `(agent_id, revision)` 保存不可变的完整配置：

- `name`：1–255 字节 UTF-8，拒绝控制字符和全空白名称。
- `driver_type`：当前只接受 `codex`。
- `model`：显式填写的 1–128 字节 ASCII 标识符，仅字母、数字、`-_.:/`；不访问模型目录。
- `credential_ref`：可空，只接受 `env:AIT_SERVER_CREDENTIAL_<NAME>`；后缀 1–64 字节，
  首字符为大写字母，其余为大写字母、数字或下划线。拒绝服务 token、任意变量和凭据正文。
- `enabled`：显式布尔值，表示允许选择；不代表执行 readiness。

所有输入对象拒绝未知字段。暂不引入 endpoint、任意参数 JSON、命令行、工具策略、推理等级
或系统提示词；这些字段需在有实际消费者时形成受约束的 revision schema。
credential_ref 的值不被解析，环境变量是否存在不影响配置保存。真实执行接入时必须在进程边界
解析为秘密类型、检查缺失/能力，并避免进入日志和持久化记录；当前不加载 `.env`。

创建时省略 `agent_id` 和 `expected_revision`，服务端分配 ID 并写 revision 1。修改必须同时
提供两者，以观察到的 revision 作 CAS，成功时追加下一 revision。新 key 即使提交相同配置也
产生新 revision；不提供原地修改、删除或隐式覆盖。时间为服务端 Unix 毫秒，不作为顺序依据。
旧 revision 通过精确读取保持可见；SQL trigger 拒绝 UPDATE/DELETE。

## 3. 显式默认选择

全局默认保存 Agent ID 和独立 selection version，初始为 `null / 0`。创建第一个 Agent 不改变
默认，读列表也没有隐式选择。set 必须提供 `agent_id`（可显式 null）与 `expected_version`；
遗漏目标会被拒绝，不能被理解成清空。新操作每次递增版本，包括同值选择。

默认只能指向当前启用的 Agent。禁用默认 Agent 必须先显式清空或选择另一个 Agent；配置写入
在同一事务检查该约束。默认指向 preset 身份，配置改版不递增 selection version；未来消费者
需单独固定具体 revision，不能把默认选择版本当作配置版本。

## 4. 事务与回执

每个配置操作在单个 catalog 事务里完成：读取回执 → 验证 head/默认约束 → 写新 revision 和
head → 写 receipt。默认操作同样将 selection 与 receipt 原子提交。失败不留下半个配置或
已变更却没有回执的默认选择。不同连接编辑同一 revision 只能有一个成功。

key 按 catalog + 方法去重；配置 fingerprint 使用规范化 ID、expected revision 和完整配置，
默认 fingerprint 使用目标 ID/null 和 expected version。不包含 key 本身、时间、request_id、
连接身份和凭据值。相同 key/业务参数返回相同 operation 与历史结果；参数不同则冲突。
已提交回执先于当前 revision/默认状态检查，因此重启后重试旧操作不会回退 head 或重新选择
已经清空的默认。返回回执后，当前状态通过 get 查询；不回收去重记录。

列表按 Agent UUID keyset 分页，每页 1–50，默认 20，只返回当前 head。每项字段有字节预算，
无需为列表读取完整历史。get 可选择当前 head 或具体 revision；不存在的 Agent 与 revision
有不同错误码。当前仍没有通用 `operation.get`，结果不明时重试原方法和 key。

## 5. Catalog 升级与监督

Catalog application_id 保持不变，schema 从 1 升至 2；Project schema 仍为 1。
全新 catalog 在单个初始化事务中建立 v2。已有独立 v1 catalog 在持有 data-dir 进程锁时，
先通过 SQLite backup API 创建 `catalog-v1-backup-<随机名>.sqlite3`，同步文件与 Unix 父目录，
然后在单个事务里添加 Agent 表和更新 user_version。备份失败就不升级；升级失败回滚并保留
已完成备份，重试可能产生多个备份。备份不自动回收，旧 catalog 的项目/回执表不改写。
foreign family、更高版本或 symlink 仍被拒绝。不存在旧 Ait 数据格式的转换。

已升级 catalog 不能用仅支持 v1 的旧 server 打开；回退到备份会丢失备份之后的 catalog
配置和回执，应停服后作为显式恢复处理，不自动降级。

Agent 与 Project 共享每实例一个短阻塞任务的准入预算，争用返回 `resource_exhausted`。
业务任务在连接断开后仍被追踪。Agent catalog adapter 持有 data-dir lease，数据库先关闭、
lease 后释放；关闭排空预算超时也不能提前让其他实例接管仍在写入的目录。
此处沿用 ADR-023 的阻塞 I/O 不能强制取消限制，不引入执行任务或后台自动恢复。

使用示例见 [操作说明](../operations/independent-server.md)，验证见
[Agent 配置报告](../reports/independent-server-m1-agents.md)。
