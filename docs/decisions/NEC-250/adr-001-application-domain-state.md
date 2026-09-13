## ADR-001：application 领域状态与边界投影

- 状态：Accepted
- 日期：2026-09-13
- 基线：NEC-150 v4、NEC-252、NEC-253、NEC-248

## 决策

命令专属 context 保存 application 的领域状态，contracts 仅承担命令输入、公开结果、事件及旧记录格式的 codec。Project 默认 Agent/revision 使用 `domain::ProjectDefaults`，Session pointer、Agent binding、Run ownership/version 使用 `domain::SessionReference`；application aggregate 只补充名称、工作目录等编排元数据。Message 记录使用领域 role/kind，并在构建历史路径时转为经过校验的不可变 `domain::Message`；native Message 与兼容字段不一致时拒绝读取。Message 历史仍由 ControlStore 保证 append-only，Session 指针、Run 和事件仍共享 revision CAS。

API Run 的 `domain::Run` 是唯一执行状态权威。公开 status/phase、最后 Message 和错误从该状态投影；取消意图与 worker lease/receipt 是独立控制元数据。旧记录的冗余投影只在 decode 边界处理，commit 统一生成投影，不能让两个可独立修改的状态同时存在于 context。Workspace Run 保留独立的 typed phase、checkpoint、integration 和 Git settlement，不伪装成 API 工具循环。

两类执行统一通过 application lifecycle 入口完成准入后执行、监督、取消及恢复，继续复用既有 RunCoordinator、worker dispatcher 和 finalization gate。此变更不增加 supervisor，也不改变 worker 协议。

旧 MessageService 的分步 append/CAS 与 ProjectService 的独立存储事务不能表达生产 ControlStore 的跨记录提交，因此删除重复入口；可复用校验归入领域实体/策略，保留生产 facade 的跨记录/CAS 回归和 adapter 的文件边界测试。删除仅服务旧入口的内存 store、edit/regenerate/指令装配测试，不再把未接入 facade 的旧用例列为生产能力。旧存储接口仅在仍有独立 adapter 消费者时保留，不保留第二套 application use case。

## 兼容与验证

公开 HTTP/CLI/Desktop DTO 与版本化事件保持兼容。持久化 codec 显式读取旧字段、保留有界加载与未加载记录隔离，投影不得输出 execution、凭证引用或私有 receipt。迁移不得回写不可变 Message。验证包括领域转换、记录往返、旧 Run 恢复与错误注入、API tool loop、Session CAS、Workspace checkpoint/integration、权限快照和 lease fencing；交付前执行 workspace format、clippy 和 tests。


## 实现与迁移约束

- `control/model` 是 application aggregate 与 contracts 的转换层。`RunLifecycle::Api` 只保存领域执行和取消意图；Workspace 分支保存 typed 生命周期及其专属 journal。固定的 Run identity/config 元数据在 codec 往返时与领域快照校验，worker successor 的身份、单调性与 completion gate 校验位于 domain。
- facade 通过 `ExecuteRun` → `supervise_run` → `drive_run` 进入同一个既有监督路径。`RunControl`、终结提交、startup recovery 和 worker dispatcher 继续共享所有权、lease 与 finalization gate；API 调用原 RunCoordinator，Workspace 调用原 checkpoint/integration 路径。
- durable JSON 形状保持兼容：写入时从权威状态生成旧外层字段。decode 检测历史漂移，record transaction 在正常 revision CAS 中写回 canonical 投影。已经 terminal 的领域执行优先；旧 terminal 投影加未结束的领域执行进入失败/取消结算，旧外层 completed 不得授予 Run 完成资格。迁移继续结算未完成子记录，不重放未知工具效果，也不能夺取已移动的 Session。
- `GetRun`、list、同步命令结果以及恢复结果明确省略 `execution`（contracts 本来已将其标为可选内部字段）；内部审计/worker receipt 保留在 durable record。API 故障和进程测试从真实存储读取审计数据，并单独断言公开结果没有 execution。事件继续使用原版本化脱敏规则。
- 删除未被 adapter/生产调用的 `MessageStore`、`SessionStore`、`ProjectStore` 及仅服务旧 service 的输入结构；保留仍有消费者的 `ProjectEnvironment` 与 NEC-253 `ProjectWorkspace`。NEC-147 中旧 MessageService 的分步 append/CAS 方案由当前 ControlStore 原子 transaction 取代，历史 ADR 不改写。

## 验证位置

`domain::SessionReference` 覆盖 stale pointer/version、忙碌 binding 与迟到 release；`control/model/run/tests.rs` 覆盖 canonical projection、取消意图、旧记录修复、终态优先和敏感错误；`control/state/tests` 保留不可变 Message、跨记录事件回滚、有界读取与 CAS 冲突；`api_tool_loop`/`api_tool_faults` 保留工具结果顺序、crash/panic 恢复与禁止重放；worker process 与 Workspace 测试保留进程隔离、checkpoint、权限和 fencing 回归。10k Message benchmark 直接调用生产路径共享的领域 traversal。
