## ADR-001：Project 编辑与独立展开的 Session 导航

- 状态：Accepted
- 日期：2026-09-16
- 来源：NEC-294
- 修订：NEC-233 第 3 条与「只展示当前 Project 的 Sessions」限制

Project 的显示名称与默认 Agent 使用 `UpdateProject` / `POST /v1/project/update` 在同一个
记录事务中保存。`project_id` 与非空 `name` 必填；名称去除首尾空白；省略或传 null 的
`agent_id` 保留已有默认值，显式 Agent 必须是可用的具名 preset。校验失败不写入任何字段。
每次更新递增 Project revision 并发出 `project.updated`。目录、Git 基线、Message 历史和已有
Session 绑定保持原值。CLI 对应 `project update --project-id … --name … [--agent-id …]`。

Desktop 的 Project 设置入口同时编辑两个字段，保存成功后更新 ProjectCatalog。默认 Agent
继续仅作为新 Session 的建议。没有可用 Agent 时仍可修改名称。

侧栏每个 Project 持有独立的窗口内展开状态，初始收起；点击名称选择并展开，再次点击当前
Project 名称收起，箭头可单独展开/收起。切换其他 Project 和刷新数据不重置这些状态。
新建 Project 或 Session 会展开其所属 Project。

新增受限 bridge `project.sessions(projectId)`，只通过既有
`GET /v1/session/list?project_id=…` 加载一个 Project 的 Session 摘要，不读取 Messages、Runs
或 progress。renderer 以 Project ID 缓存摘要，generation fencing 避免旧请求覆盖新响应；
展开项在 Session/Run 生命周期事件与重同步时刷新，失败保留摘要并显示 Retry。
摘要仅供导航，实际打开 Session 时重新加载该 ProjectView，成功后一起提交选择与对话，失败
保留此前可见的 Session 和发送目标。Message/Run/progress 仍只属于当前 ProjectView。

展开状态不跨窗口或应用重启保存；Project 配置由 Rust/SQLite 持久化，不引入新存储表。
