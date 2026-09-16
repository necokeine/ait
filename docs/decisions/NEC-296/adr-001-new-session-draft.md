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

这是桌面交互的调整，CLI/HTTP 的显式 `CreateSession` 命令保留。Provider、model 和 reasoning
仍由已保存的 Agent preset 决定；Session 创建并空闲后可编辑其私有配置。

验证覆盖无 Session 的 Project、空输入、取消、重复新建、默认 Agent、活动 Run 并存、
接纳失败重试、提交期间导航，以及 Rust 原子派生的父链和失败无领域写入。
