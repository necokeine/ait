## ADR-001：Cron occurrence 独立 Session 与 Desktop 管理页

- 状态：Accepted
- 日期：2026-09-17
- 来源：NEC-304
- 修订：NEC-150 ADR-001 v4 的 Cron Sessionless 约束、ADR-013 的 Cron 主工作区例外

Cron 仍固定引用 `project_id + base_message_id + agent_id`。Desktop 新增 Crons 页面，可从
Session 选择目标，也可直接输入 Message ID；选择 Session 时只在保存时读取其当前
`current_message_id`，之后 Session 移动不会改变 Cron target。Agent 必须是启用的具名
preset。页面支持列出、创建、启停和立即触发；schedule 与 IANA timezone 继续由 Rust
核心校验和保存，renderer 不复制解析规则。

每个新 occurrence 必须在同一 control transaction 中创建一个 Run 和一个新 Session。
Session 从 Cron 的固定 Message 打开、绑定 Cron 的 Agent，并立即以该 Run 占用
`active_run_id`；Run 的 `session_id` 指向它，生成的 Message 按既有 CAS 规则推进 Session。
终态结算条件释放占用。Session 使用 `<Project>/.ait/<session-id>` linked worktree，并从目标
Message 的 Git provenance 建立，因此 Cron 不再以 Project 主检出作为执行目录。

Session ID 由 `cron_id + scheduled_at` 在固定 UUID v5 namespace 下确定性派生。相同
occurrence 重放仍由既有 dedupe 返回原 Run，不创建第二个 Session、worktree、Message 或
event；不同 occurrence 得到不同 Run 与 Session，并从同一个固定 Message 形成独立分支。
调度端口通过 `create_session_id` 把该身份交给统一 Run starter。旧版本已持久化的无 Session
Cron Run 继续可读取和恢复，不做隐式回填。

当前 daemon 仍只有 Cron 配置与显式 occurrence 触发入口；本 ADR 不宣称已接入持续时钟
循环。Desktop 的“Run now”使用当前毫秒作为 occurrence，完成后打开新 Session。
