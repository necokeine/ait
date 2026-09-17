# ADR-001：固定 Run 权限与 API HostTools 单次升级授权

- 状态：Proposed，待 NEC-290 / CodeGate 验收
- 日期：2026-09-15
- 实现基线：`68e91487b8edeaf07d44ec793ebd786987a92299`
- 依赖：ADR-001 v4、NEC-234、NEC-247、NEC-248、NEC-263、NEC-252
- 修订：替代 NEC-234/247/263 中 API 升级自动拒绝、Readonly 不广告可申请写工具、忽略 API approval 设置的条款；Codex 原生审批保持原语义。

## 决策

RunPermissionProfile 是运行基线，创建后不变。高于基线的已实现 HostTools 操作通过独立
`ToolApprovalRecord` 获得明确的 `ToolGrant`，只适用于绑定的一个调用。普通操作仍按基线执行，
没有全局或 Session 授权，也没有原生 thread/turn/request scope。

domain 定义授权目标、grant 和状态值对象，无异步、IPC 或 UI 依赖。application 拥有准入、
校验、请求/决定/消费事务与 waiter；daemon 是唯一 durable writer。runtime 保留 ToolUse →
ToolExecution → 唯一 ToolResult → Provider 的既有循环。worker 的独立 IPC 驱动在等待决定时
继续心跳；daemon 控制面、查询和事件不等待该工具。私有协议 minor 2 要求 `tool-grants-v1`，
旧 worker 缺少能力时拒绝握手，避免默默退回自动拒绝或伪报升级成功。

### 授权与执行

1. runtime 先保存 Pending ToolExecution。write/edit 在 Readonly 需要至少 Workspace Write；
   显式 sandbox_permissions 可以请求更高档位。未知工具、非法参数和不能证明的目标拒绝。
2. application 从持久化 Run/ToolExecution 读取完整调用，通过执行器 factory 的 review port
   形成目标，不信任 renderer 的目标/权限，也不把任意 worker 文本作为可批准对象。
3. 记录关联 request ID、Run、ToolExecution、call ID、完整参数 SHA-256、lease epoch、期限。
   目标包含工具、命令或文件路径、cwd、升级理由、当前/请求档位与路径身份摘要。
   内容/补丁不进入审批目标；敏感参数沿现有持久化拒绝策略处理，敏感或不完整的授权目标不提供批准。
4. 成员只能 Approve once、Deny、Cancel Run。决定事务检查 pending、活跃 waiter、Run、lease、
   期限、管理员上限、完整参数及真实路径；提交之后才通知 waiter。重复/冲突决定返回错误。
5. runtime 保存 Running 后，执行前通过 application 再次校验并 CAS 消费 Approved → Consumed。
   消费是副作用前的持久化屏障；失去 ACK 不自动再次消费。worker 再校验所有调用标识、摘要、
   期限、目标和路径，在其本地调用集合中拒绝重复 grant。文件发布与 Shell 启动前还会复核期限。
6. HostTools 只克隆该次调用的执行上下文，采用 grant 的有效 sandbox；原执行器/Run/Settings
   不变。Shell 使用与 review 探测相同的 Seatbelt/bwrap 后端；后续独立命令恢复原档位。
   worker 的 SandboxToolFactory 还独立执行 bootstrap 管理员上限。

文件路径拒绝绝对路径、父路径、隐藏组件、符号链接；记录祖先与目标的设备/inode 身份，文件
还记录大小与时间属性。决定、消费及实际执行检查路径，写入发布前再检，仍使用 capability
目录句柄与原子替换，不追随符号链接或截断外部硬链接。路径变化导致拒绝。

Shell 的授权对象是**完整命令 + cwd + 该次进程树的有效 sandbox**，不把命令解析成猜测的文件
白名单。Workspace Write 继续限制外部读取、写入和网络；Full Access 明确表示该命令及其子进程
拥有宿主文件/网络能力。卡片明确显示这一区别；不把完整命令授权伪装成单个网络或文件授权。
cwd/根目录身份在执行前复核；限制继承、输出预算和进程树回收沿用 NEC-263。

### 期限、取消与恢复

默认人工决定窗口最多 120 秒，并截短至 Run 和实际 worker 总墙钟期限之前 2 秒。等待计入
预算，不重置 Run deadline；预留时间只用于结算，不保证 Provider 可在剩余时间完成。
期限到达后记录 Expired，保存 denied ToolResult 并尝试继续 Provider；总预算仍可终止 Run。
Desktop 展示绝对期限，断线时也移除过期的批准按钮。

取消先持久化 Cancelling 和请求 Cancelled，再停止执行；runtime 区分拒绝与整个 Run 取消。
worker 每次 claim lease 都建立独立、不可恢复的连接失效信号，由该 Run 的 store 持有；
断线先标记该信号，再通知 waiter 并 drain RPC。目标检查开始前已取得信号，因此注册 waiter
之前断线也不会丢失。请求、等待、决定和消费共用该信号，在异步目标检查和提交边界后复核。
已进入存储的提交必须完成，再立即过期；即使该提交短暂留下 Pending，后端也拒绝失效决定，
不会向 worker 发放授权。连接状态不依赖全局历史 registry，随 store/lease 释放或替换。

重新 claim lease 时过期所有未消费授权，并将旧进程尚未派发的 Pending/Approved 工具意图
结算为 Denied（包括目标检查尚未生成审批记录的情况）；runtime 随后持久化唯一 ToolResult。
daemon 重启不会复活旧 waiter，也不会默认批准。已经 Running 的
工具结果未知时沿 NEC-247 fencing/reconcile 失败并要求人工检查，不重放副作用。
Consumed 只表示授权已消费，不保证操作成功；最终结果以唯一 ToolResult 为准。

### UI、策略与存储兼容

Session 和 Runs → View Run 均从持久化 Run 构建卡片。本 ADR 原有的 Cron `session_id=None`
约束已由 NEC-304 supersede：当前 occurrence 会携带独立 Session；旧版无 Session Cron Run
仍可操作。
事件触发重读，页面恢复和断线重连不依赖瞬时通知。Runs 详情保留最终 Run 状态/最后回复。
新 HTTP 路由 `/v1/run/tool-approval/resolve` 调用 application 专属授权事务入口；原 native
approval 命令及 scope 保持独立。Electron preload/main 使用单独白名单方法，拒绝 scope 和
额外参数，并在写入前检查 Project 归属。

API Provider 只支持 on_request：基线内直接执行、可交互升级请求人工决定。新 Run 选择
untrusted_only/always 会在 Message/Run 准入前明确失败；不会再静默忽略设置，也不改变
Codex 原生策略。Settings 默认值继续是 workspace_write + on_request。

旧 Run 缺少 tool_approvals 时读为空集合，不重写历史 Message、旧 denied 结果、设置或 Run
权限。授权记录嵌入既有 Run JSON，由原事务/CAS 持久化，无独立数据库 migration。路径身份
校验目前支持 Unix；Windows 单次升级拒绝，原档位下的结构化文件操作仍可用。

## 验证与演示

见 [API 工具审批手册](../../operations/api-tool-approvals.md)。验证包含真实进程、临时 SQLite、
两种离线 Provider、真实 Shell、重复/变更/过期/取消/worker 丢失，以及真实 Electron GUI。
GUI 夹具使用 Playwright 官方 [Electron API](https://playwright.dev/docs/api/class-electron)，
用支持的 profile 路径配置载入原 main/preload/renderer，模型端仅监听临时 loopback 地址。

断线回归通过目标工厂与真实 SQLite 的提交暂停点控制时序，并等待 supervisor 确认连接已
失效后才恢复执行；覆盖两个 Provider 的首次目标检查、Pending 提交前后、决定目标检查和
决定提交前后。测试不靠固定 sleep 猜测 EOF 已处理，并断言及时恢复、旧决定拒绝、唯一拒绝
ToolResult、零副作用与无新 lease 审批复活。
