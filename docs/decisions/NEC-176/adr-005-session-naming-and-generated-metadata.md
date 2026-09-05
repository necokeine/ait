# ADR-005：Session 命名与生成式检索元数据

- 状态：Accepted，已实现
- 日期：2026-09-05
- 修订：ADR-001 v4 的 Session 展示元数据
- 来源：NEC-176

## 决策

1. `Session.name` 是成员显式设置的可选名称；新 Session 默认空字符串。`Session.title` 与 `Session.description` 是 AI 生成的展示标题和纯文本检索摘要。
2. 展示优先级固定为非空 `name`、非空 `title`、`Session <id 前 8 位>`。清空手工名称会恢复 AI 标题或默认显示。
3. Session 第一次成功的交互完成后，桌面端从用户 prompt 去除 Markdown 与指令标记、合并空白并截到约 60 字，立即写入临时 `title`。
4. 临时标题显示后，桌面端异步触发一次只读标题请求。该请求最多读取清洗后的 2,000 字符，固定使用 `gpt-5.6-luna`、`low` 推理强度和 `title + description` JSON schema。
5. 生成标题最多 36 字，尽量少于 5 个词、通常以祈使动词开头、使用用户语言并保留 `ABC-123` 一类工单号；引号、Markdown 与结尾标点无效。成功结果原子替换临时标题并写入搜索摘要；失败时保留临时标题。
6. `title_generation_started` 持久化一次性启动状态，避免重复付费请求。名称、标题和描述更新不推进 Session 指针版本，因而不会与后台生成期间的新消息 CAS 冲突。

## 边界

application 只依赖 `SessionTitleGenerator` port；Codex adapter 负责 app-server 的只读调用、推理强度、结构化输出 schema 与结果校验。标题生成不创建 Message、Run 或 Git 提交，也不能修改 Project 工作目录。

搜索同时匹配最终展示标题和 `description`。手工名称始终优先，因此生成请求即使与重命名并发完成，也不会覆盖成员看到的名称。

## 验证

- control-plane 测试覆盖默认空名称、临时标题、一次性生成、2,000 字符上限、手工名称优先及指针版本不变。
- Codex protocol/adapter 测试覆盖 `gpt-5.6-luna`、`low`、只读沙箱、结构化 schema 和中文工单标题。
- 桌面测试覆盖 Markdown/指令标记清理、Unicode 截断与侧边栏入口。
