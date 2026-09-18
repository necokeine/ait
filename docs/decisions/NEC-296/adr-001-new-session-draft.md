# ADR-001：新 Session 从首条用户输入开始

- 状态：Accepted
- 日期：2026-09-16
- 来源：NEC-296
- 依赖：NEC-150、NEC-162、NEC-226

桌面端点击 Project 的新建 Session 或命令面板 Create Session，只进入本地派生草稿。
草稿基点是 Project 的 `root_message_id` 指定的初始 system Message；不从当前 Session
head 或任意其他根节点推断。草稿不写 Session、Message、Run 或 linked worktree。

草稿默认选择 Project 的默认 Agent，允许切换已保存的 Agent preset。首次提交非空用户输入时，
Electron 通过现有 `/v1/session/submit-fork` 把基点、Agent 和文本交给 Rust application，
由已有原子事务创建 Session、用户 Message 和 Run。新建草稿没有 source Session，不能复用
其他 Session。已有 Session 的 Derive from here 继续使用 `/v1/session/submit-derive`。

接纳后立即打开新 Session 展示 Run 进度；不等待生成完成。接纳失败保留草稿输入供重试。
提交期间阻止同一草稿重复发送，之后的导航优先于迟到的提交响应。取消或离开未提交草稿不会
留下空 Session。其他 Session 正在执行或派生时仍可打开新的草稿。

接纳回执与 Project 读取分开：IPC 返回 Session / Run 回执后，草稿即被消费，不依赖视图 mutation
generation。重命名或失败的导航不能解锁已消费的输入。读取失败时提供重新打开 Session 的入口，
只重试读取；后续成功导航保留优先级，后台接纳与标题更新不取代导航意图。

第一次提交前，草稿固定候选 Session UUID、文本与 Agent。传输结果不明时冻结这份输入，重试先
按 Project 读取 Run 并用候选 Session ID 恢复回执；未找到时仍用同一 ID 提交。Rust 的 Session ID
唯一性与原子 fork 事务保证并发重放也不能重复创建 Message / Run。仅明确的首次校验拒绝允许编辑
后重试；结果不明期间的错误不释放原提交。此恢复状态属于当前内存草稿，不提供应用重启后的草稿恢复。

这是桌面交互的调整，CLI/HTTP 的显式 `CreateSession` 命令保留。Provider、model 和 reasoning
仍由已保存的 Agent preset 决定；Session 创建并空闲后可编辑其私有配置。

验证覆盖无 Session 的 Project、空输入、取消、重复新建、默认 Agent、活动 Run 并存、
接纳失败重试、并发重命名、失败/延迟导航、接纳后读取失败、回执丢失与稳定 ID 恢复，
以及 Rust 原子派生的父链、失败无领域写入与重复 ID 不产生新领域记录。
