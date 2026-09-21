# ADR-021：Codex 原生任务取消固定运行时限

- 状态：Accepted，2026-09-20
- 上位约束：ADR-001 v4、ADR-017、ADR-018、ADR-020
- 修订：ADR-017 中 Codex 原生 writer 的 wall-clock 策略；不改变领域聚合或 wire schema。

## 问题

Codex 原生任务与 API、辅助查询共用 worker 的 300 秒截止时间。正常分析、构建和等待工具
可能超过该值。supervisor 到期发送普通 Cancel，原生最终历史记录 interrupted，导致桌面
显示笼统的取消错误。桌面还把所有 interrupted Run 标为工作区恢复问题；告警显示时，
四个网格子元素进入仅定义三行的布局，告警与会话标题重叠。

## 决定

按用户明确要求，`Executor::Codex` 的 `Operation::Open` 不再创建固定截止时间。该操作
覆盖原生 writer 的准入、等待 Start、模型与工具执行、审批等待及最终历史读取。
`Limits.wall_clock_ms` 继续限制 API Run 和 Codex 辅助操作（历史、模型目录、标题）。
不新增巨大毫秒值代替“无限”，不改变现有字段格式或协议版本；策略由 daemon supervisor
根据受信任的 executor 类型决定。

“无固定时限”仅指任务总耗时：手动取消、daemon shutdown、心跳失联、进程退出、协议
错误与原有 step/token/output/cost 限制继续生效。握手、单次 I/O、原生准入和最终历史
对账仍有各自的短超时。正常结束和取消后的进程树回收保留。关闭应用或系统进程结束后，
任务不保证继续运行；未知输入仍只对账，不重放。

桌面告警、会话标题、消息滚动区和输入框分别占显式网格行；没有告警时该行折叠，多个
告警占用至多 `min(240px, 30vh)` 并可滚动。`RUN_RECOVERY_FAILED` 保留工作区恢复标题，
其余 interrupted Run 显示 `Run interrupted`，原始错误内容与定位入口保留。
不重写已有 Run/Message，不因升级自动续跑之前中断的任务。

## 验证

使用暂停时钟与真实 frame 编解码/连接泵，验证原生 writer 经一天模拟时间仍能正常结束；
API 与辅助查询仍在原截止时间取消，手动取消、shutdown、心跳失联和 drain 超时仍有效。
浏览器回归检查单条、多条、长文本与隐藏告警的几何边界、滚动和 Session 定位。
执行结果与覆盖率见[验证报告](../reports/codex-unlimited-runtime.md)。
