# ADR-001：Codex Run 权限快照与原生审批边界

- 状态：Accepted
- 日期：2026-09-09
- 依赖：ADR-001 v4、NEC-162 ADR-004、NEC-198 ADR-001、NEC-205 ADR-001、NEC-209 ADR-001、NEC-212 ADR-001
- 修订：替代 Codex workspace Run 固定使用 `workspace-write` + `never` 的实现默认值

## 背景

桌面设置已经展示 sandbox 与 approval 选项，但 Codex workspace adapter 过去固定发送
`workspace-write` 与 `never`。这会使成员看到的策略和真实 Run 权限分离，也没有承载 Codex
app-server 原生 command、file change、permissions 审批请求的应用端口、受限 IPC 与可恢复 UI。

Codex 当前协议的通用 approval policy 只有 `untrusted`、`on-request`、`never`；原生审批响应
对 command/file 请求支持 `accept`、`acceptForSession`、`decline`、`cancel`，permissions 请求
则返回明确的 `permissions` 集合与 `turn` 或 `session` scope。因此旧 `always` 选项不能被忠实
映射，拒绝也不能借用批准响应表达。

## 决策

1. application 在创建 Run 前解析设置，并把 `RunPermissionProfile` 快照写入 Run。Codex adapter
   只读取这份快照：`read_only`（以及兼容别名 `strict`）映射 `read-only`，
   `workspace_write` 映射 `workspace-write`，显式 `full_access` 映射
   `danger-full-access`；`on_request` 与 `untrusted_only` 分别映射 `on-request` 与
   `untrusted`。运行中设置变化不得回写既有 Run。
2. `always` 在当前协议下不可忠实实现，Run 准入必须返回配置错误。未知设置值、缺失值、超过
   daemon `--max-sandbox` 上限及被 `--deny-session-approvals` 禁止的授权范围均 fail closed。
   这些检查在追加 user Message、创建 Run、取得写入租约或调用 Provider 前完成。
3. 原生审批使用 domain 的 kind/status/scope 值对象、application-owned `WorkspaceApproval`
   port、Codex adapter bridge、daemon entity-operation HTTP API、context-isolated Electron IPC 与
   桌面审批卡。renderer 只能提交 `approve|deny|cancel`，批准还必须提交
   command/file 的 `one_shot|session` 或 permissions 的 `turn|session`；main process 再次
   白名单和有界校验，不允许把一次授权扩大为整轮授权。
4. 每个审批记录 Run ID、原 JSON-RPC string/integer request ID、method、thread ID、turn ID、
   item ID、状态、决定时间、授权 scope，以及 permissions 请求中经过严格反序列化与大小检查的
   明确 filesystem/network 集合。未知字段、未知 special path、空权限集合和关联不匹配均在回答
   或外部操作前拒绝。凭证、环境变量与 provider secret 不进入记录。
5. Adapter 在独立 task 等待审批，协议事件循环继续分发其他 notification。原 request ID 原样用于
   回答；重复 pending/answered ID 至多得到一次失败响应；`serverRequest/resolved`、turn 终止、
   Run cancel 与 adapter cancellation 会中止 waiter，并把 durable 状态更新为 expired 或
   cancelled。桌面根据 cursor 事件刷新，同时每次 `workspace.view` 从 durable Run 重新构建审批卡，
   所以 SSE 重连不依赖丢失的瞬时事件。
6. command/file/legacy 的一次性批准映射 `accept`，会话批准映射 `acceptForSession`；拒绝映射
   `decline`，取消映射 `cancel`。permissions 批准只返回原请求中已验证的权限集合及
   `turn|session` scope，不能由 renderer 扩写。拒绝和取消绝不携带 scope 或 permissions。
7. 原生 operation 继续只作为 Codex 输出展示与审计；审批记录属于 Run metadata，不追加或修改
   Message，也不伪装成 Ait `ToolUse`/`ToolResult`。非审批 server request 仍由既有拒绝策略处理，
   不扩展本 ADR 范围。

## 后果

- `full_access` 是危险但显式的成员选择，部署方可用 daemon 上限彻底禁止；设置页面不再暗示
  `always` 已生效。
- 历史 Run 缺少快照时由 serde 采用最小权限 `read_only + on_request`，恢复不会静默继承旧的
  隐式写权限。
- app-server 进程退出后，协议级 session grant 自然失效；Ait 只保留非秘密审计记录，不自行
  重放或扩大授权。

## 验证

- application fake workspace agent 覆盖四种 sandbox 输入、设置变更后的快照不变、未知值、
  `always`、管理员上限冲突，以及审批等待期间查询可用。
- offline fake app-server 覆盖真实 wire 参数、原 request ID、permissions profile/scope、关联不匹配、
  重复回答、resolved、取消与事件继续分发。
- desktop 纯函数测试覆盖关联信息、权限集合转义、唯一允许的 action/scope 和审批事件重同步。
- workspace tests 继续证明 Message 历史不可变、Codex operation 不变成 ToolUse/ToolResult，且
  存储与导出中没有 provider credentials。
