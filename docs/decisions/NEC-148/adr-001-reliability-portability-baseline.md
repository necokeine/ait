## ADR-001：可靠性、可观测性与可移植归档基线

- 状态：Accepted
- 日期：2026-09-04
- 依赖：NEC-150 ADR-001 v4、NEC-152 本地 API/CLI 纵向切片

## 决策

1. 本地 API 的结构化日志和单调计数指标使用同一个 `Correlation`：
   `project_id`、`session_id`、`run_id`、`call_id` 均为可选，但调用边界始终生成
   `call_id`。日志输出前递归脱敏，指标通过仅回环可访问的 `/v1/metrics` 暴露。
2. Project 归档使用显式 `format_version`。归档包含 Project、引用的 Agent 非敏感
   配置、全部 Message 分支和 Session ref；保留 Project/Agent/Session revision 与 Message
   identity/parent edge。Run/Cron、活动 Run 绑定、附件字节和凭证不进入归档。
3. 导入必须在单次 control snapshot CAS 中完成，并在写入前验证版本、ID 唯一性、完整
   无环消息树、Project 所有权、Session 指针和 Agent binding。目标 workdir 由导入方显式
   提供并重新执行 Git root 安全检查。
4. SQLite 默认使用 WAL、`synchronous=FULL`，通过 Online Backup API 生成一致备份；恢复
   后必须通过 `quick_check`。
5. 性能基线不设易受机器影响的绝对时限，Criterion 保存三条可比较曲线：10k 深度路径读取、
   2k Message 长上下文校验、1k Cron 扫描规划。相同环境相对基线回退超过 20% 必须评审。

## 安全边界

归档“无凭证”不等于“可公开”：Message 文本、工具结果、Project 路径和附件引用仍可能
敏感。日志只允许事件名与结构化字段，禁止将原始命令体、prompt、工具参数或响应正文写入
日志。脱敏规则覆盖 authorization、cookie、password、secret、token、api_key、credential
及 Bearer 值；新增凭证字段必须同时新增测试。

## 恢复语义

SQLite snapshot 与 durable event outbox 保持同事务。Run 与 Cron 的恢复仍沿用原有固定
Run ID、dedupe key、工具 reconciliation 与终止屏障，不因可移植归档引入跨机器恢复活动
Run 的隐式行为。

## 验收证据

测试和基准的稳定入口记录在 `docs/operations/reliability-security-observability.md`。
