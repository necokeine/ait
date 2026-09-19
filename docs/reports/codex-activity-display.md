# 接近 Codex 原生样式的过程折叠

原来的 `Events` 展开后，为每段过程重复显示角色、时间和 `Process`/`Tool call` 标签，并直接
铺开命令与输出。2026-09-20 更新后，使用三层按需展开的展示：

- 外层 `Activity` 默认收起；展开后使用分隔线和正常正文排版，最终回答始终留在外层之外。
- 连续命令合并为 `Ran N commands` 摘要，展开后显示紧凑命令列表。
  每条 Command 默认独立收起，只显示图标、单行命令、必要状态和箭头；长命令使用省略号，
  无命令摘要时回退到操作标题。点击或使用 Enter/Space 后显示完整命令、路径和输出。
  其他工具与有内容的推理各用一行图标和名称，展开后查看详情。
- 空推理只显示灯泡图标和不可展开的 `Reason` 标签，没有箭头、空卡片或点击反馈。
  同时处理 Codex operation 和 provider reasoning 的空白内容；流式推理收到正文后恢复折叠详情，
  运行中或失败状态仍保留。含摘要、路径或正文的推理继续可查看。
- 命令列表移除整组卡片的边框、背景和额外内外边距；相邻折叠行连续排列，包含不同消息或
  不同活动种类的折叠段落。仅展开的详情保留卡片，过程正文仍保留段落间距。
  展开命令组不再缩进列表，组标题与每条命令的图标保持同一左侧位置。
- 已完成状态不重复占据摘要；运行中、失败、拒绝、取消等状态继续在摘要中可见。
  混合成功/失败的命令组给出失败数量。2026-09-20 按用户要求，状态仅在操作摘要中显示，
  外层 `Activity` 不再汇总失败或运行中数量。
- 过程段的作者和时间保留给辅助技术与消息检查器，视觉上去掉重复标题。
  Message 身份、顺序、选中状态及历史数据不变。
- 每条命令使用消息/Run 范围内的原生 item ID 保存展开状态。同一会话重新渲染时保留三层
  展开状态和摘要的键盘焦点；命令组增长、输出和状态更新不会收起已展开的命令。
  切换上下文时重置折叠。

现有 Desktop 历史视图尚未接入 Codex 已提供的 Turn 耗时、开始与结束时间，未用 Message
时间差猜测“用时”。外层先使用 `Activity`；2026-09-20 的协议核对和只读实测见
[Turn 耗时核对](codex-turn-timing.md)。非空/异常 Codex 文本元数据仍沿用原来的可查看展示。

## Test coverage

本轮 **not measured** 行覆盖率：仅修改 Desktop TypeScript、CSS、测试及文档，未修改 Rust
行为，也未采集 TypeScript 行覆盖率。Rust 的历史覆盖率不能作为本轮 UI 覆盖率。
后续应单独配置 Desktop 的行覆盖率测量；本轮验证以数据保真、真实 DOM 交互和视觉检查为准。

基于 `91931165b0e514dcdd59da8ae16596f40d486fa5` 的工作区，2026-09-20 在 macOS arm64 执行：

- `npm run typecheck`、`npm run build` 通过；`npm test`：144 通过、0 失败、0 跳过。
- 在构建后执行 `AIT_TEST_SCREENSHOTS=/tmp/ait-reason-command-screens node --test test/browser/message-activity.test.mjs`
  及 `node --test test/browser/*.test.mjs`：完整浏览器套件 82 通过、0 失败、0 跳过。
  其中 4 项活动展示测试覆盖浅色/深色、单行省略与无额外行间隙、
  默认收起、单条独立展开、HTML 转义、完整命令/输出、Enter/Space、重新渲染后的展开与焦点、
  关闭父组后再次打开、上下文重置，以及流式组增长后状态更新。补充空推理的静态显示、
  两种推理数据从空白到正文的流式切换，以及命令组和展开前后命令行的图标位置断言。
  主题测试同时检查单条命令
  折叠摘要与展开详情的运行状态可读性。
- 已检查 `commands-folded-dark.png` 与 `command-expanded-light.png`，确认空推理无展开箭头、
  命令图标对齐、折叠列表紧凑、长命令单行省略、展开内容可读；截图位于上述临时目录，未写入仓库。
- `cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings` 和
  `git diff --check` 通过。
- `cargo test --workspace --no-fail-fast`（默认 features）：464 通过、1 失败、5 忽略。
  `daemon_is_ready_and_rejects_unsent_native_recovery_without_replay` 首次运行超过既有 15 秒
  启动窗口；单独重跑 `cargo test -p ait-daemon --test codex_http` 后该组 5 项全部通过。
  未修改 Rust 或放宽断言；本次全量运行仍记录为有一次耗时断言失败。
  4 项真实模型测试与 1 项外部 worker 测试沿用既有忽略配置。

浏览器使用隔离 preload fixture，未连接真实 Codex 账号；Linux/Windows 未实测。
