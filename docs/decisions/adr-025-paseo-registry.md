# ADR-025：先移植 Paseo Project / Workspace 模型与 registry

- 状态：Accepted，落实用户对“复刻 Paseo server”的方向纠正。
- 日期：2026-09-22。
- 来源：`getpaseo/paseo@2c8e8a826810337492cc5a38bb0bbd705b6fb632`。
- 范围：新 `server`；旧 daemon、旧 Ait crate 与数据不变。

## 决策与前序修正

新 server 以 Paseo 的实体、字段、关系和可观察行为为移植基准，Rust 是实现语言。
ADR-022 中“只参考架构职责、继续采用 Ait 业务模型”的方向不再作为新功能的基准。
先移植类型与 registry，再接目录发现、投影和协议处理；此前的 Session 固定 worktree 切片暂停。

Project 是项目记录；Workspace 有自己的身份，以 `projectId` 归属 Project。
一个 Project 可对应多个 Workspace；同一 cwd 也可以有多个不同 Workspace。
Project 支持 `git | non_git`；Workspace 支持 `local_checkout | worktree | directory`。
Paseo 持久化记录不加入 `base_commit`、`root_message_id` 或 `owner_epoch`。

本决策修订新 server 的 Project/Workspace 模型选择，不改变现有 Message 历史，也不实现或
重定义 Session、Message、ToolUse、ToolResult、Run。仓库关于不可变历史和 Run 完成屏障
的要求继续适用于已存在及未来实际实现的执行功能；不能用目录 registry 冒充执行状态。

## 类型落点

| Paseo 源定义 | Rust 位置 |
| --- | --- |
| `PersistedProjectRecord`、`PersistedWorkspaceRecord`、kind、`UntrustedWorkspaceSource` | `server-domain::registry` |
| `ProjectRegistry`、`WorkspaceRegistry`、mutation/context | `server-ports::registry` |
| `FileBackedProjectRegistry`、`FileBackedWorkspaceRegistry` | `server-storage::registry` |
| `WorkspaceProjectDescriptorPayload`、`ProjectPlacementPayload`、wire Project kind | `server-protocol::project`（从独立 protocol workspace 模块导出） |
| `WorkspaceDescriptorPayload`、checkout 联合类型、script/git/forge runtime 等嵌套结构 | `server-protocol::workspace` |

domain 的新依赖 Serde 只用于纯数据编解码，不引入运行时、SQL、HTTP 或 provider。
protocol 保持独立，不把持久化 record 直接用作对外 DTO。既有 ADR-023 DTO 移到
`server-protocol::project_lease`，类型明确命名为 `ProjectLeaseSnapshot`，避免被误认为
Paseo Project。现有 WS 的 `project.open/list/get/close` 暂时继续返回原租约格式。

Project 的全部 10 个字段和 Workspace 的全部 18 个字段均移植，包括名称覆盖、图标、归档、
分支、精确 worktree root、比较基线、pin、labels 和不可信来源。`isPaseoOwnedWorktree`
保留原 JSON 名称，不擅自替换品牌字段。新分配 ID 是 `prj_` / `wks_` 加 8 个随机字节的
16 位小写十六进制；读取接受原 schema 的任意字符串 ID，包括 legacy ID。

时间戳仍是字符串，schema 不额外要求 RFC3339。字段 JSON 使用原 camelCase；未知对象字段
与 Zod 一样被丢弃。必填可空、可选不可空、可选可空、缺失补 null/false/空数组分别处理。
可选可空的 wire 字段用 `Option<Option<T>>` 保留缺失/null/值三态，不能统一折叠为空值。
checkout 的条件约束和 `worktreeRoot` / `workspaceDirectory` 回退在反序列化中完成。

DTO 包含暂未实现功能的字段只表示类型已移植，不代表对应 capability 已实现或可调用。

## Registry 行为

适配器读写 JSON record 数组，保持插入顺序，同 ID 重复项最后一份内容生效而位置不变。
文件路径由宿主传入；将来组装采用 Paseo 对应的 `<data-dir>/projects/projects.json` 与
`workspaces.json`。本批不自动创建这些生产文件，也不迁移现有 SQLite catalog。

- 初始化、list、get 不创建缺失文件；无项目路径检查、Git 调用或隐式 Message 创建。
- 一个 registry 实例及其 clones 共享串行 mutation；暂存新集合，原子替换文件成功后发布 cache。
  失败不改变已提交 cache、不发通知，同一实例可重试。宿主负责跨进程排他。
- `get_or_create_active_by_root` 在同一串行边界内选择最早 active 项，刷新 kind/projectKey，
  保留原 ID、名称覆盖和创建时间。仅有 archived 项时分配新 ID；生成 ID 冲突时重试。
- 根路径比较是 lexical：处理 dot segment、分隔符、Windows namespace/case，不解析 symlink。
  它与 ADR-023 打开项目时的 realpath 去重是两套有不同语义的操作，不可互相替代。
- Project 重复 archive 是无操作；Workspace 重复 archive 更新 archive/updated 时间。
  Workspace 的自动归档 URL 在没有非空新值时保留。
- 提交后发通知。Project observer 失败返回“已提交但通知失败”；Workspace observer 失败
  只记安全错误日志，不把已经提交的 mutation 判为失败。drop subscription 解除订阅。
- Workspace freeze 拒绝后续写入但允许读取；重新构造实例恢复写入。

Rust 接口是阻塞接口，未来 API 应使用已有受监督 blocking 调度，不在 reactor 中读写文件。
observer 同步执行且位于写锁之外；调用方不应把 observer 当成 durable outbox。

## 明确差异与本批终点

1. Paseo 在 registry 加载失败时记录错误并继续为空集合；Rust 返回 `InvalidFile/Io`，保留
   文件且不允许后续写入覆盖它。这是明确的数据保护差异，不宣称错误路径完全兼容。
2. 正常 ISO 时间按真实时刻比较；日期形式 `YYYY-MM-DD` 也支持。任意非 ISO 字符串仍可
   保存，但排序不模拟 JavaScript 所有宽松 `Date.parse` 规则；无法解析时按 ID 排序。
   同时间 ID 使用 Rust 字符串顺序，不模拟依赖宿主 locale 的 `localeCompare`。
3. 使用同目录 tempfile、文件 sync 和原子 rename；未承诺断电后的目录项持久化。
   任意路径的多个独立实例和跨进程写入仍需宿主互斥，不支持外部程序同步改写缓存文件。
4. 本批实现公开 registry CRUD/分配/通知接口与 freeze；Workspace labels 的跨文件 batch
   journal、历史 agent bootstrap、目录扫描、placement reconciliation、descriptor 聚合和
   Paseo request/response handler 是后续移植范围。
5. 现有 binary 仍使用 ADR-023/024 的服务组装。这个提交建立可测试的数据和 registry 基础，
   不宣称整个 server 已与 Paseo 协议兼容；后续必须替换 handler/组装并处理既有新服务数据，
   不能仅把旧接口返回值改名或加字段。

## 验证

`scripts/paseo-registry-fixtures.mjs` 从固定 commit 的原始 Zod 定义生成输入、是否接受和
归一化输出；fixture 记录 commit、源文件 SHA-256、Zod 版本。Rust 测试读取静态 fixture，
无 Node/Paseo 安装依赖。registry 另测并发分配/更新、归档差异、重开、通知、冻结与失败回滚。
许可与修改说明保存在 `third-party/paseo/`。实际检查与覆盖率见
[移植报告](../reports/paseo-registry-port.md)。
