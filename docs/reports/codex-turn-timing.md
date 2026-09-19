# Codex Turn 耗时核对与 Activity 摘要调整

2026-09-20 按用户要求，`Activity` 外层只显示标题与展开箭头，不再汇总失败/运行中数量。
具体命令或工具行中的状态及分组失败数量继续保留。

## 原生时间字段

本机 `codex-cli 0.153.4` 重新执行
`codex app-server generate-ts --out /tmp/ait-codex-timing-schema-20260920`，未启用
`--experimental`，生成的 `v2/Turn.ts` 包含：

| 字段 | 单位与含义 | 缺失情况 |
| --- | --- | --- |
| `durationMs` | Turn 从开始到结束的毫秒耗时 | `null` 表示未知 |
| `startedAt` | Turn 开始的 Unix 秒时间戳 | 可为 `null` |
| `completedAt` | Turn 结束的 Unix 秒时间戳 | 可为 `null` |

生成的 `ThreadTurnsListResponse` 返回 `Turn[]`；`TurnCompletedNotification` 的 `turn`
也是同一个 `Turn`。因此历史列表和完成事件的协议契约均支持这些时间字段。
[OpenAI Docs](https://learn.chatgpt.com/docs/app-server#items) 还说明 `commandExecution`
和 `dynamicToolCall` 的 item 级 `durationMs`；item 耗时与整轮耗时必须区分，不能将命令耗时
简单相加作为整轮用时。Turn 耗时反映开始到结束的经过时间，不应描述为纯模型推理耗时。

对当前 Ait 工作目录只读抽样 3 个原生 Thread、每个最多 3 个 Turn：`thread/list` 使用
`useStateDbOnly: true`，`thread/turns/list` 使用 `itemsView: "notLoaded"`。
返回的 9 个 Turn 中，8 个 completed 均带正数 `durationMs` 和开始/结束时间，
例如 `18967`、`62094` 毫秒；1 个 interrupted 的 `durationMs`、`completedAt` 为 null。
所有返回 Turn 的 items 均为空，未读取消息正文、导入会话或启动模型回合。
此次抽样证明本机实际历史可返回耗时，不代表所有旧记录都完整。

## Ait 尚未接入的部分

`crates/ports/src/provider.rs` 的 `CodexTurnSnapshot` 接收了 `started_at`、`completed_at`，
但没有 `duration_ms`，原生 `durationMs` 在反序列化时被忽略。application 目前只用完成/开始
时间选择 Message 的 `created_at`，没有将整组时间信息传给 Desktop。Message 时间戳也包含
投影分段偏移，不能拿来推算原生耗时。

接入时应优先保留并使用 `durationMs`；缺失时只在开始/结束时间都有效且结束不早于开始的
情况下，用差值换算毫秒。`0` 是有效耗时，可选字段同时兼容省略和显式 null。
数据不完整就省略用时。一个 Turn 被投影成多个 Message 或过程段时
应按 Turn 身份关联，避免重复计时。本轮完成协议核对与摘要修改，尚未实现耗时数据到 UI 的传递。

## Test coverage

本轮行覆盖率 **not measured**：仅修改 Desktop 展示、现有浏览器断言及文档，未修改 Rust
行为，未采集 TypeScript 行覆盖率。后续耗时字段接入若修改 Rust，应执行
`cargo llvm-cov --workspace --html` 并补充序列化、投影与显示链路覆盖。

基于 `a4d005997c9e6fa0a455f23475614d1b585ee954` 的工作区，macOS arm64 验证：

- `npm run typecheck`、`npm run build` 通过；`npm test`：144 通过、0 失败、0 跳过。
- `npm run test:browser`：81 通过、0 失败、0 跳过；已验证外层无状态统计而内层失败状态保留。
- `cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings` 和
  `git diff --check` 通过。
- `cargo test --workspace --no-fail-fast`（默认 features）：465 通过、0 失败、5 忽略。
  4 项真实模型测试和 1 项外部 worker 测试沿用已有忽略配置；此前超时的 daemon 启动测试
  本轮通过。未调整测试时限或改动后端代码。
- 原生时间抽样使用真实 app-server，只读取元数据；不计入自动化测试数。

浏览器回归使用隔离 preload fixture；Linux/Windows 未实测。真实协议抽样过程及结果位于
`/tmp/ait-codex-timing-probe.py`、`/tmp/ait-codex-timing-probe.json`，只输出状态和时间字段，
不输出 Thread ID、标题或消息内容。
