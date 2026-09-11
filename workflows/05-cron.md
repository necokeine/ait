# WF-05：保存和触发定时任务

用户目标：让一个固定上下文配合指定 Agent 重复生成独立结果，且不影响正在使用的 Session。
前置条件：完成 WF-01，`ROOT_ID` 属于 p1。此流程验证显式 occurrence 触发，不等待真实时钟。

## 操作

```bash
ait cron create --id cron-daily --name 每日总结 --project-id p1 \
  --base-message-id "$ROOT_ID" --agent-id agent-demo --schedule '0 9 * * *' --timezone Asia/Shanghai
ait cron disable --cron-id cron-daily
# 预期拒绝：当前未启用
ait cron trigger --cron-id cron-daily --scheduled-at 1788480000000
ait cron enable --cron-id cron-daily
ait cron trigger --cron-id cron-daily --scheduled-at 1788480000000
# 同一 occurrence 重试
ait cron trigger --cron-id cron-daily --scheduled-at 1788480000000
# 另一个 occurrence
ait cron trigger --cron-id cron-daily --scheduled-at 1788566400000
ait cron list
ait message list --project-id p1
ait run list --project-id p1
```

`scheduled_at` 是 Unix epoch 毫秒，示例固定数值仅标识演练 occurrence。
修改这个值意味着触发另一次运行，不能在重试时随意替换。

## 验收与失败恢复

- 禁用后触发新 occurrence 返回 `INVALID_CRON`，无新增 Run/Message。
- 启用后第一次触发创建一个 Run，`trigger=cron`、`cron_id=cron-daily`、`session_id=null`，
  `base_message_id` 固定为配置中的节点，不自动插入 user 输入。
- 重复 `cron_id + scheduled_at` 返回原 Run，不增加 Message、Run 或 durable event。
- 不同 occurrence 产生不同 Run，但都从同一固定基点形成分支；已有 Session 指针和版本不变。
- 当前去重查询优先于 enabled 检查，已存在 occurrence 的重放仍返回原记录。
  若新 occurrence 被禁用状态拒绝，先核对配置，再决定是否启用后重试。

当前 daemon 只暴露 Cron 配置与显式触发接口；保存 schedule 不保证之后会自动到点运行。
时钟循环、并发策略和 misfire 需要后续独立接入和测试。

自动化：[`wf05_cron_occurrence_is_idempotent_and_independent`](../bins/cli/tests/workflows.rs)，
覆盖启停、新 occurrence 拒绝、重复触发无事件增量，以及 Session 不受影响。
