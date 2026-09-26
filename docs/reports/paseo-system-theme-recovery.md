# Paseo 系统主题恢复同步

2026-09-26，基于 `5e9fc8a` 工作区修改。范围仅为 `apps/app` 的浏览器外观同步；
`apps/paseo` 使用该 renderer，没有 Rust 或领域边界变更。

## 排查结果

用户现象是自动日夜切换后消息正文、代码块或工具结果仍保留旧配色，切换 session
不能恢复，刷新或重启后恢复。

- Unistyles 的 CSS 变量由 `prefers-color-scheme` 媒体查询更新，不依赖 React 渲染。
- `withUnistyles` 的 React 主题快照通过主题通知更新。`AppearanceStyleBoundary` 的 key
  也来自这个快照。通知漏掉时，CSS 可以切换，保留的正文样式和刷新边界仍使用旧主题。
- 原 `AppearanceProvider` 只在设置变更时应用外观，缺少页面恢复可见、窗口重新聚焦、
  `pageshow` 时对当前系统主题的核对；保留 session 的导航不能保证重新初始化主题快照。
- 普通浅色→深色→浅色切换及冻结后恢复的隔离测试都通过。主动屏蔽媒体查询事件后，
  能复现“runtime 已变深色，正文仍是浅色主题”的状态。用户现场漏通知的具体触发条件
  （系统调度、后台或恢复时序）尚未确认，不能把模拟复现当作现场事件证据。

## 修改

`AppearanceProvider` 在自动主题模式下安装浏览器同步器，监听系统配色变化及
`visibilitychange`、`focus`、`pageshow`。实际主题名称发生变化时，重新发布当前主题对象，
使已有的主题消费者和外观边界恢复同步。

隐藏页面延后至恢复可见时核对；主题没有变化时不发布；固定主题不受系统事件影响。
正常通知已经更新的消费者收到相同主题对象，React 可跳过重复更新。卸载时移除所有监听。
原生平台继续使用 Unistyles 自身的订阅。

## 验证

- `npm exec --workspace=@getpaseo/app -- vitest run --project unit src/appearance/system-theme-sync.test.ts src/appearance/apply.test.ts src/components/appearance-style-boundary.test.ts`
  ：3 个文件、22 项测试通过，其中 7 项新增同步回归，覆盖双向切换、漏通知后恢复、重复事件、
  隐藏页面、固定主题、无主题和清理监听。
- `npm exec --workspace=@getpaseo/app -- tsgo --noEmit`：通过。
- `EXPO_NO_TELEMETRY=1 EXPO_OFFLINE=1 PASEO_WEB_PLATFORM=electron npm exec --workspace=@getpaseo/app -- expo export --platform web --output-dir ../../.tmp/appearance-repro/dist`
  ：通过，确认 web 平台实现进入桌面 renderer 构建。运行中的安装包未替换。
- 修改文件的 `oxfmt --check`、`oxlint` 与 app ESLint：通过；`git diff --check` 通过。
- `cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`：通过。
- 真实生产构建的 Markdown、代码高亮组件在 Chromium 中实测，临时诊断脚本为
  `.tmp/appearance-repro/recovery.mjs`：屏蔽媒体查询事件后，runtime 为 `dark`，正文仍为
  `rgb(26, 26, 30)`，挂载编号仍为 1；安装同步器并触发聚焦后，正文变为
  `rgb(250, 250, 250)`，挂载编号变为 2；恢复浅色后颜色恢复且挂载编号变为 3。
  该测试验证真实内容组件与新同步器，不是完整 session 端到端验收。
- 新打包产物再次通过 `.tmp/appearance-repro/production-recovery.mjs` 验证同一故障和恢复
  序列：这次使用 `AppearanceProvider` 自动安装的监听，不再单独注入同步器。
- `npm exec --workspace=@getpaseo/app -- vitest run --project unit`：601 个测试文件通过，
  5 个失败；5271 项通过、12 项失败，另有 32 个未处理错误。失败涉及缺失根目录
  `CHANGELOG.md`、当前中文 locale 与英文断言不符、Node 26 的 `localStorage` 不可用、
  本机监听端口被沙箱禁止，以及连接测试超时。此次未修改这些模块，也没有将全量结果
  标为通过。日志：`.tmp/appearance-repro/app-unit.log`。
- `cargo test --workspace`：沙箱内运行在 `ait-agent-adapters` 的本机 HTTP 夹具测试处
  因 `PermissionDenied` 停止。放宽整个 workspace 的运行请求被自动审批拒绝，理由是
  可能调用真实 provider/API、使用凭据并产生费用。核实 `llm_client.rs` 使用
  `127.0.0.1` 和固定假凭据、唯一付费测试明确 `#[ignore]` 后，改为仅重跑已审查的
  `cargo test -p ait-agent-adapters --test llm_client`：15 项通过，1 项付费测试跳过。
  日志：`.tmp/appearance-repro/llm-fixture-tests.log`。完整 workspace 仍未通过验收。

## Test coverage

未测量覆盖率百分比。此次没有 Rust 行为修改，未运行 `cargo llvm-cov`；TypeScript 测试
也未启用覆盖率采集。22 项通过是执行结果，不是覆盖率指标，没有可比较的本次覆盖率基线
或共享覆盖率 artifact。后续若需要数值覆盖率，应在允许本机测试服务的环境中执行
`cargo llvm-cov --workspace --html`，并另行采集前端覆盖率。

尚未覆盖真实操作系统按时间自动切换、系统休眠/唤醒，以及用户原会话的现场事件时序。
本次交付是对已验证失同步状态的恢复修复，不宣称已确定操作系统漏通知的原因。
