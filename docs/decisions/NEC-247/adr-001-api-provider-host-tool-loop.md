# ADR-001：公共 API Provider 宿主工具循环

- 状态：Proposed，待 NEC-247 验收
- 日期：2026-09-12
- 基线：ADR-001 v4、NEC-190 ADR-011、NEC-234、NEC-192

## 决策

公共 API Run 装配既有 `ait-runtime::RunCoordinator`。application 的 `ControlRunStore`
只实现存储端口及公共投影，没有第二个工具状态机。`RunView.execution` 保存协调器的规范 Run、
attempt 和 ToolExecution 子记录，与现有控制面实体在同一 SQLite CAS 事务中提交。
`MessageView.data.native_message` 是不可变领域 Message；`agent_revision` 记录固定修订。
Run/Session 的 head、run_seq 和唯一 ToolResult 与 Message 原子提交。

`AgentProviderGateway::complete_turn` 每次只返回一个领域 assistant 提案和 usage。
Rig adapter 保留原供应商 call/item id 和 reasoning 元数据，把 ToolResult 与完整历史送回原 Provider。
工具成功续轮重用同一 attempt；失败、恢复与预算继续由 RunCoordinator 处理。
只有没有待执行工具、结果、重试或队列时才通过原有终止屏障。
公共入口当前仍拒绝活动 Session 的新输入（SessionBusy），不创建平行 Run；尚未接通公开排队操作。

模型目录先按精确 provider+model 解析，再与宿主执行器能力相交。
`read/write/edit/grep/bash` 仅广告实际支持的参数；Codex 不接入 API ToolSet。
执行器通过 `RunToolFactory` 注入，不把文件、HTTP、Tokio 或 SDK 依赖引入 domain。
未安装工厂的嵌入程序使用文本能力和空工具表，不伪报文件能力。

## 权限、恢复与边界

- API Run 复用 Project 写租约；不可变 RunPermissionProfile 和管理员上限沿用 NEC-234。
- 文件操作以 cap-std Project 句柄为根，逐级打开无符号链接目录，禁止绝对路径、父路径、隐藏组件。
  写入为同目录临时文件原子替换，防止截断硬链接目标。单个参数/文件/结果上限 64 KiB。
- 读取、正则搜索和受控命令最多四个并发；有写入、审批或更大批次时串行。结果稳定排序。
- 首版 shell 仅固定的 echo/printf/sleep，无 shell 解释、继承环境、网络或后台任务；最长 30 秒，
  超时/取消/超限会终止子进程。Windows 不广告 shell。
- 显式 sandbox 升级由原生 `RunApproval` 端口拒绝并持久化 denied；没有借用 Codex 审批或虚构批准。
  批准也不能改变执行器的 Run 权限上限。后续交互审批可以替换该端口。
- intent 与 Running 记录必须在执行前 ACK。重启时 Running 工具结果未知则失败并保留记录，绝不盲重放。
  已保存的终态结果仅补齐唯一 ToolResult；API 已保存 Message 路径可以继续原 Run。
- Provider/工具错误为有界安全诊断，run 事件移除 execution 载荷；可移植归档省略原生工具参数、
  output 和 provider metadata，保留文本占位。完整执行历史只在本机 Message/Run 查询中可见。

## 首版限制与验证

API 工具直接在持有租约的 Project 内修改文件，保留未提交的 Git diff，不自动提交，也没有 Codex
隔离 worktree 的补偿发布流程。成员必须审阅/提交或清理改动后继续要求干净基线的交互。
FullAccess 是管理员允许的能力上限，首版执行器仍只开放 Project 文件和有限命令。
不包含任意 shell、网络工具、交互审批 UI、worker IPC 或远程执行器。

[WF-13](../../../workflows/13-api-provider-tool-loop.md) 默认离线执行真实两种 HTTP adapter 与公共
Session/Run/SQLite 路径，独立检查文件、Python、Git、Message、Run、usage 和重启查询。
WF-11 改为模型用 write/read 创建文件；付费调用继续显式 opt-in。
