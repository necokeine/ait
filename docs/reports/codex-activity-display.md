# 接近 Codex 原生样式的过程折叠

原来的 `Events` 展开后，为每段过程重复显示角色、时间和 `Process`/`Tool call` 标签，并直接
铺开命令与输出。本次将它改为两层按需展开的展示：

- 外层 `Activity` 默认收起；展开后使用分隔线和正常正文排版，最终回答始终留在外层之外。
- 连续命令合并为 `Ran N commands` 摘要，其他工具与推理各用一行图标和名称。
  展开摘要后再查看命令、路径、输出或推理详情。
- 已完成状态不重复占据摘要；运行中、失败、拒绝、取消等状态继续在摘要中可见。
  混合成功/失败的命令组给出失败数量，外层收起时也保留状态提示。
- 过程段的作者和时间保留给辅助技术与消息检查器，视觉上去掉重复标题。
  Message 身份、顺序、选中状态及历史数据不变。
- 同一会话重新渲染时保留两层展开状态和摘要的键盘焦点；切换上下文时重置折叠。

现有 Desktop 历史视图没有可靠的 Turn 开始/结束时间，未用 Message 时间差猜测“用时”。
外层先使用 `Activity`。非空/异常 Codex 文本元数据仍沿用原来的可查看展示。

## Test coverage

本轮 **not measured** 行覆盖率：仅修改 Desktop TypeScript、CSS、测试及文档，未修改 Rust
行为，也未采集 TypeScript 行覆盖率。Rust 的历史覆盖率不能作为本轮 UI 覆盖率。
后续应单独配置 Desktop 的行覆盖率测量；本轮验证以数据保真、真实 DOM 交互和视觉检查为准。

基于 `84b47827474a12d9c4284d3cab06ef6667172cc6` 的工作区（含前一轮空元数据显示修复），
macOS arm64 执行：

- `npm run typecheck`、`npm run build` 通过；`npm test`：143 通过、0 失败、0 跳过。
- `npm run test:browser`：72 通过、0 失败、0 跳过。新增 3 项覆盖浅色/深色样式、命令分组、
  混合状态提示、HTML 转义、最终回答可见、Enter/Space 操作、重新渲染后的展开与焦点、
  上下文重置，以及流式命令组增长后状态更新。
- `AIT_TEST_SCREENSHOTS=/tmp/ait-codex-activity-screens node --test test/browser/message-activity.test.mjs`
  专项 3 项通过。已检查浅色/深色截图 `activity-light.png`、`activity-dark.png`；图像位于上述
  临时目录。更新截图 fixture 的时间与焦点后专项再次通过，未将重复运行累加到用例数。
- `cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings` 和
  `git diff --check` 通过。
- `cargo test --workspace --no-fail-fast`（默认 features）：459 通过、1 失败、5 忽略。
  失败仍为 `daemon_is_ready_and_rejects_unsent_native_recovery_without_replay` 的 15 秒启动
  时限断言；4 项真实模型测试与 1 项外部 worker 测试沿用既有忽略配置。
- 随后运行 `cargo test --workspace --test codex_http`：5 通过、0 失败、0 忽略，执行耗时
  15.77 秒。全量启动耗时问题在独立测试组中未复现；本轮未修改后端或调整测试时限。
  该问题已在前一轮出现，仍需独立跟踪全量负载下的 daemon 启动耗时，不记为全量一次通过。

浏览器使用隔离 preload fixture，未连接真实 Codex 账号；Linux/Windows 未实测。
