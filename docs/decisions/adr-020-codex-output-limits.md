# ADR-020：Codex 输出预算与超限诊断

- 状态：Accepted，2026-09-20
- 上位约束：ADR-001 v4、ADR-017、ADR-018
- 修订：NEC-169 worker 的输出预算字段与 ADR-017 原生执行限制；不改变领域聚合边界。

## 问题

Codex 原生执行复用了 API 工具输出的 64 KiB 上限，既限制整轮累计文本，也限制单个
原生 item 的完整 JSON。正常的长篇分析、命令结果和文件变更可能因此中止。计量器只返回
布尔值，桌面统一显示 `native execution resource limit exceeded`，丢失触发指标与数值。

## 决定

worker `Limits` 增加可选 `max_codex_output_bytes`，当前 daemon 默认发送 8 MiB。
该值独立限制一轮累计 UTF-8 文本字节数，以及每个原生 item 序列化后的 JSON 字节数。
API `max_output_bytes` 仍为 64 KiB。8 MiB 为有界的初始工程取值，容纳比原上限大 128 倍
的分析输出，并保持低于 64 MiB 的完整历史传输上限，不保证任意大小任务都能执行。
启动校验拒绝零值和超过编译上限的值；目前没有增加用户设置入口。

这是私有协议 3.0 的可选字段扩展。新 worker 收到未携带此字段的旧 daemon bootstrap 时，
继续使用原 `max_output_bytes`，不隐式放宽旧调用方的限制；旧 worker 按 optional-field
规则忽略新字段，仍执行较小的旧限制。daemon 与 worker 应来自同一构建。

所有预算均在严格超过阈值时触发，等于上限可以继续：

- `native_items`：一轮不同原生 item ID 的数量，默认 128；开始与完成只计一次。
- `item_bytes`：单个 item 的 JSON 字节数，包括编码转义；默认 8 MiB。
- `text_bytes`：一轮跨 item 累计文本 delta 字节数；完成事件不重复累计文本；默认 8 MiB。
- `tokens`：原生 `tokenUsage.last` 的 `max(totalTokens, inputTokens + outputTokens)`，
  默认 1,000,000；不重复加入缓存输入与推理输出，不将多次模型调用加总为此项。

计量器记录首个超限原因。后续缓冲事件不扩大计数，也不能把超限替换为成功或其他原因。
原生 writer 继续 interrupt、读取权威历史和回收进程；返回不可重试的 `RUN_LIMIT_EXCEEDED`，
`message` 包含指标、单位、实际值和上限，`details` 保存 `metric`、`actual`、`limit`。
诊断不包含提示词、命令输出、文件路径或凭证。通用 adapter stream 使用相同可读诊断。
application 沿用对外 `ApiError` 的 code/message/retryable 字段，具体数值随 message
持久化；结构化 details 用于原生 adapter/worker 边界。后续原生历史同步不能覆盖本地错误
及超限终态，也不能触发自动 Git 提交。

原生 item 大小采用只计数的 JSON writer，避免为长度检查再分配整份序列化缓冲。

## 传输与展示

更大的执行预算不改变 IPC frame 上限。worker 将大段文本 delta 按 UTF-8 边界拆成最多
4 KiB 的进度片段；`MessageStarted` / `MessageCompleted` 的实时文本预览最多保留
64 KiB，并显示截断标记。完整权威历史继续通过 16 KiB 分块传输，原生 Codex Thread 保留
原文；Ait 最终 Message 仍遵循 ADR-016 已有的有界 ProviderItem 投影（字符串最多
20,000 字符、payload 最多 256 KiB）。本次不修改投影版本或已经持久化的 Message。
预览大小不是执行预算，不因预览截断而取消 Run。

既有 wall-clock、step、token、历史总量与 API 工具限制继续生效；本次不修改这些阈值。

## 验证

测试覆盖各指标的阈值与具体数值、UTF-8/JSON 转义、重复事件、首错保留、旧 bootstrap、
真实 worker 的长输出与超限取消、跨 IPC 的结构化错误、历史同步保留和桌面终态展示。
测量结果见[实现与验证报告](../reports/codex-output-limits.md)。
