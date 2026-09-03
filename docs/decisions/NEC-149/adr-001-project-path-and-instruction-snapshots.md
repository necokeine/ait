## ADR-001：Project 路径边界与指令快照

- 状态：Accepted
- 日期：2026-09-03
- 依赖：NEC-150 ADR-001 v4
- 来源：NEC-149

## 1. 决策

1. Project 注册先将现有目录解析为规范化绝对路径。若该目录当前的 Git top-level 不等于自身（包括位于另一个仓库内部），在该目录执行 `git init`，复查成功后才允许持久化。
2. Project 内文件访问只接受相对路径，拒绝绝对路径、`..`、root/prefix 组件。读取时解析完整 canonical path；创建时解析最近的现有祖先。两种情况都必须仍位于 canonical Project root 内，因此指向外部的符号链接会被拒绝。
3. 越界访问没有隐式 fallback。调用方必须改用显式 external API，同时传入授权根和目标绝对路径；目标 canonical path 必须位于授权根下。
4. 指令源由应用显式配置唯一的数值 priority，并按 priority 从低到高保存为结构化内容；高优先级来源靠后。相同 priority 属于配置错误，缺失的可选文件被跳过。
5. 每份来源快照记录名称、定位符、priority、字节数、SHA-256 与原始 UTF-8 内容。整个结构化组件按无歧义的长度前缀编码计算 digest；digest 未变化则复用当前 revision，变化时 append 新 revision。
6. 新建 Message tree 与 Session 是同一存储事务：若需要先追加指令 revision，再把完整 Project 指令快照作为一个结构化 System Message 组件保存，最后保存指向它的 Session。这里不预先拼接最终 prompt；Project 后续更新不得回写旧根消息。
7. 最终 provider prompt 在 Session/Run 真正调用 LLM API 时即时组装。组装器读取根到 head 的 System Message 组件，并结合固定的 Agent revision、运行时能力/工具策略及本次调用上下文生成请求；最终 prompt 不是 Project 或 Session 的可变缓存。

## 2. 边界

- `domain` 只保存 Project、revision、来源摘要、System Message 与 Session 的纯数据语义。
- `ports` 定义文件/Git 能力，以及注册与新树创建的原子存储边界。
- `application` 决定 Git 初始化流程、结构化指令发现和创建 Session 的编排；最终 prompt 渲染属于后续 Run/provider 调用边界。
- `project-local` 实现本机 canonical path、符号链接边界与 Git subprocess；它不持有 Project/Session 状态。
- SQLite 适配器后续实现 `ProjectStore` 时，必须在数据库唯一约束和事务中重复保证本 ADR 的唯一性、append-only 与原子性，不能依赖进程内预检查。

## 3. 已知限制

canonicalize 后再普通路径打开仍存在恶意本地进程并发替换符号链接的 TOCTOU 窗口。面向不可信工作目录的写工具必须在各平台使用目录句柄/no-follow 等原语完成最终打开；本切片提供的 guard 是路径策略与非对抗性本地访问基线。
