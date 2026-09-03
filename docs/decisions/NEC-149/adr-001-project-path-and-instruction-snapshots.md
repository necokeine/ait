## ADR-001：Project 路径边界与指令快照

- 状态：Accepted
- 日期：2026-09-03
- 依赖：NEC-150 ADR-001 v4
- 来源：NEC-149

## 1. 决策

1. Project 注册先将现有目录解析为规范化绝对路径。若该目录当前的 Git top-level 不等于自身（包括位于另一个仓库内部），在该目录执行 `git init`，复查成功后才允许持久化。
2. Project 内文件访问只接受相对路径，拒绝绝对路径、`..`、root/prefix 组件。读取时解析完整 canonical path；创建时解析最近的现有祖先。两种情况都必须仍位于 canonical Project root 内，因此指向外部的符号链接会被拒绝。
3. 越界访问没有隐式 fallback。调用方必须改用显式 external API，同时传入授权根和目标绝对路径；目标 canonical path 必须位于授权根下。
4. 指令源由应用显式配置唯一的数值 priority。按 priority 从低到高稳定渲染；高优先级内容靠后，冲突时覆盖低优先级。相同 priority 属于配置错误，缺失的可选文件被跳过。
5. 每份来源摘要记录名称、定位符、priority、字节数及 SHA-256。最终渲染文本也记录 SHA-256；digest 未变化则复用当前 revision，变化时 append 新 revision。
6. 新建 Message tree 与 Session 是同一存储事务：若需要先追加指令 revision，再保存包含完整 rendered prompt、revision、digest 和来源摘要的不可变根 System Message，最后保存指向它的 Session。Project 后续更新不得回写旧根消息。

## 2. 边界

- `domain` 只保存 Project、revision、来源摘要、System Message 与 Session 的纯数据语义。
- `ports` 定义文件/Git 能力，以及注册与新树创建的原子存储边界。
- `application` 决定 Git 初始化流程、指令排序/渲染和创建 Session 的编排。
- `project-local` 实现本机 canonical path、符号链接边界与 Git subprocess；它不持有 Project/Session 状态。
- SQLite 适配器后续实现 `ProjectStore` 时，必须在数据库唯一约束和事务中重复保证本 ADR 的唯一性、append-only 与原子性，不能依赖进程内预检查。

## 3. 已知限制

canonicalize 后再普通路径打开仍存在恶意本地进程并发替换符号链接的 TOCTOU 窗口。面向不可信工作目录的写工具必须在各平台使用目录句柄/no-follow 等原语完成最终打开；本切片提供的 guard 是路径策略与非对抗性本地访问基线。
