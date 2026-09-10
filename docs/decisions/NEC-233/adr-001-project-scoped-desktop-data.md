## ADR-001：Desktop 读取拆分为全局目录与单 Project 投影

- 状态：Accepted
- 日期：2026-09-10
- 依赖：NEC-150 核心领域模型 v4、NEC-205 实时进度、NEC-224 记录化控制面
- 修订：完成 NEC-224 尚未彻底移除的 Electron `workspace.view` / `DesktopView` 聚合边界
- 来源：NEC-233

## 背景

NEC-224 已删除 daemon 的 Workspace Snapshot command 和 HTTP route，但 Electron main 仍会读取所有
Projects、Agents、Providers、Sessions 和 Run progress，再与当前 Project 的 Messages/Runs 组合成
`DesktopView` 交给 renderer。Project 切换、写后刷新和事件重同步仍因此依赖一个跨 Project 聚合。

## 决策

1. daemon 与 Desktop 的读取拆成三类独立契约：
   - `ProjectCatalog`：全局 Project 元信息，只用于导航、选择和 Project 配置；
   - `AgentCatalog`：全局 Agent 与 Agent Provider 目录；
   - `ProjectView(project_id)`：只包含一个 Project 的 Sessions、Messages、Runs、Run progress 和恢复提示。
2. `/v1/session/list`、`/v1/message/list`、`/v1/run/list`、`/v1/run/progress` 都要求
   `project_id`。application/store 的 Session 与 progress 读取也使用同一 Project 选择器；progress
   通过 `body_json.project_id` 的表达式索引读取。
3. Electron bridge 删除 `workspace.view`。公开读取方法为 `project.list`、`agent.catalog` 与
   `project.view`；renderer 可以在内存中组合当前屏幕状态，但该组合不跨 bridge，也不包含其他
   Project 的 runtime 记录。
4. Project 内写操作返回受影响的 `ProjectView`；Agent/Provider 写操作只返回 `AgentCatalog`；修改
   Project 元信息只返回 `ProjectCatalog`。Session 专属 Agent 配置同时返回 Project 与 Agent 两个受影响切片。
5. Project 切换和写返回继续使用 generation fencing。带其他 `project_id` 的 Run/progress/approval
   事件不作用于当前 Project；terminal event 只刷新当前 Project。cursor reset/renderer ready 无法确定
   丢失范围时，并行重读三个独立切片，而不是恢复 Workspace Snapshot。
6. daemon readiness 改用独立 `/v1/health`，不再借 Project list 探活。Project、Agent/Provider、Settings
   和 protocol/health 保持全局，是领域所有权决定的例外，不把它们伪造或复制进每个 Project。

## 兼容性与后果

- 当前 API 尚未公开，不保留无 `project_id` 的 Session/progress 读取或 `workspace.view` 兼容别名。
- CLI `session list` 的 `--project-id` 由可选改为必填，与 Message/Run list 一致。
- renderer 只展示当前 Project 的 Sessions；切换 Project 后再加载其 Session 列表。Project catalog 始终可见。
- durable event/SSE 的全局 cursor、回放和 ACK 协议不变；本次只改变事件触发的失效范围。
- 不实现 NEC-146 的物理拆库，也不引入分页或按 Session 懒加载。

## 验证

- application 双 Project 测试分别查询 Sessions、Messages、Runs，并断言不存在跨 Project 记录。
- progress application/store 测试同时保留两个 Project 的 checkpoint，分别读取时只返回目标 Project。
- HTTP 测试断言四个 Project runtime 读取缺少 `project_id` 时返回 `400 Bad Request`。
- Desktop 测试覆盖 Project read path、空/单 Project、失效记忆 ID、A/B Project 状态隔离、跨 Project
  event 过滤，以及现有 Project switch/terminal refresh/write response generation fencing。
