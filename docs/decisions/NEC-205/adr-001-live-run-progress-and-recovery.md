# ADR-001：Run 实时进度、游标恢复与异步提交

- 状态：Proposed（NEC-205 实现，待评审）
- 日期：2026-09-07
- 依赖：ADR-001 v4、NEC-152、NEC-204 ADR-001

## 决策

1. 保留现有 `/v1/session/send-message` 和 `/v1/session/fork` 的同步完成契约；桌面端改用新增的
   `submit-message` / `submit-fork`。异步接口只在 user Message 与 queued Run 原子持久化后返回，
   后续执行由 daemon 持有的任务继续，HTTP 请求或事件订阅结束不传播取消。
2. `WorkspaceAgent` 可接收 application 提供的 `WorkspaceProgressReporter`。Codex Adapter 在该边界
   归一化 message started/delta/completed、operation started/completed、可见 warning/retry 和 turn
   status；application 为每个 Run 分配从 1 递增的 `seq`，并在事件中同时携带 Run、Project、
   Session 和 item 身份。
3. 进度写入使用容量 256 的有界通道，以最多 64 项或 40ms 为一批写入独立 durable event outbox，
   同时 upsert 一份有界 Run 展示 checkpoint（最多 512 个 item、每条实时消息 512 KiB、8 条 warning）。
   它不逐 token 改写 workspace snapshot，也不修改未完成的 Message；最终 assistant Message 与
   NEC-204 的 `output_items` 仍一次性原子落库。
4. `/v1/event/stream` 先按 cursor 分页回放，再持续查询同一 durable outbox。回放和监听没有两个数据源，
   因而交接期间到达的事件由后续 cursor 查询读取；客户端按全局 cursor 和 Run-local `seq` 去重。
   所有普通状态事件和 progress 事件通过同一事务内裁剪路径，使 outbox 在任意写入后都只保留最近
   50,000 项；非零 cursor 落在保留窗口之外或指向未来时，服务发送
   `stream.reset_required`，客户端直接采纳服务端 cursor 并重新读取 snapshot 和 active Run checkpoint。
   cursor bounds 与 replay page 在 SQLite 同一锁快照内读取，裁剪不能在校验与取页之间制造静默缺口。
5. Electron main 只建立一条受控事件流，并通过固定 IPC channel 广播给 renderer。每次文档加载由 preload
   生成唯一 generation，renderer 安装监听器后显式发送 ready；main 只向当前 ready generation 投递，frame
   与 ACK 也以 generation 隔离。ready 时主动同步当前连接状态并要求 snapshot/checkpoint resync；ACK 超时则
   舍弃未确认的临时投影并再次收敛到 resync。每个窗口至多有一帧未确认 IPC，main 与 snapshot 期间的
   renderer backlog 均限制为 512 项/1 MiB。renderer 每帧合并应用事件且最多重绘一次，只把 `activeRunId`
   匹配当前 Session 的 checkpoint/事件投影为临时消息；Session 切换不创建重复订阅。处于底部时跟随增量，
   向上阅读时保留 `scrollTop`。连接断开是独立的重连状态，不改变 Run 终态。
6. provider turn 完成后尽力把 Run 展示为 `settling`；该中间状态写入失败不能阻断最终 Message 和 Run
   终态的可靠持久化，终态落库后才清除 checkpoint。Run admission 之后的普通错误、panic 和 terminal
   store 暂时故障均由 supervisor 收敛为可靠终态；在此之前继续持有 Session、Project workspace 和
   finalization guards。所有终态统一发送 `run.updated`；renderer 也兼容保留窗口内旧版
   `run.cancelled`，收到两者后都重新读取 snapshot，以不可变 Message 或终态卡片替换临时投影。
7. daemon 启动时把遗留的 `queued | running | settling` Run 明确标记为
   `RUN_RECOVERY_FAILED` 并释放 Session。自动续跑与 Codex thread resume 不在本票范围内。

## 结果

- 桌面发送不再被完整 Codex turn 阻塞，订阅断开不会终止执行。
- 慢订阅客户端不向执行链路施加背压；应用内部的持久化通道有界，且最终状态保存不依赖订阅者。
- 刷新和 cursor 重连可从 checkpoint 与有序事件恢复；游标过期不会静默漏掉进度。
- CLI 脚本和既有调用者仍获得同步终态，不发生静默契约变化。

## 验证

- application 测试以 300 个慢速模拟增量验证 500ms 内异步接受、超过一页回放、Run-local 顺序、
  工具运行态 checkpoint、最终清理与不可变结果收尾。
- HTTP 测试覆盖回放切到持续监听时恰好提交的新事件。
- desktop 测试覆盖 checkpoint 恢复、重复序号过滤、工具状态替换、部分 final answer 和断线状态。
- desktop 压力与竞态测试覆盖 50,000 事件慢消费者、有界 frame/backlog、future cursor 收敛、
  commentary-only phase、即时完成与 terminal-before-submit 标题顺序，以及刷新 generation 交接、
  旧 generation 迟到 ACK、断线状态重同步、ACK 丢失后的超时恢复，以及 active snapshot 收到取消
  事件后清除 active Run 并展示 cancelled 终态。
- application fault tests 在 queued→running 和 integrated→settling 分别注入 CAS conflict 与 store
  error，验证异步 Run 终态、Session 释放、同 Project 租约和整合结果一致性。
- SQLite 并发裁剪测试保证边界读取只会得到连续 terminal event 或显式 cursor reset；普通状态与
  terminal commits 在 progress 填满上限后仍保持全局 50,000 项和正确 cursor bounds。
- 真实 Codex 桌面验收覆盖异步接受、53 个顺序进度事件、运行中操作卡片、自动标题，以及最终 Message/文件落盘。
