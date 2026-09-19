# Desktop 旧数据库启动恢复

基于 `85404b23611afd5cf6f342c3c8fb0cfae5a14add` 的工作区，实现
[ADR-019](../decisions/adr-019-desktop-startup-database-reset.md)。

当本 Desktop 启动的 daemon 因旧存储格式退出时，页面显示删除范围、实际数据库路径、
“Delete old database…”和“Retry startup”。内部错误收进详情，不再统一要求用户重新编译。
恢复期间停用工作区导航与快捷键，避免离开恢复页进入未就绪的业务页面。

删除由窄 IPC `startup.reset-database` 发起，主进程固定当前 profile 的 catalog 路径并显示
默认取消的原生确认框；不接收 renderer 提供的目标路径。只删除该 catalog 及 SQLite 附属
文件，不备份、不迁移。成功后重载并由 daemon 创建空库；失败显示原因并允许重试。
项目库仍在原处，旧格式项目没有因此获得兼容性，原 catalog 被删除后依赖它的旧升级路径
也会丢失。所有破坏性验证均在临时目录，没有删除或升级用户实际数据库。

## Test coverage

Rust workspace 行覆盖率 **not measured**：本次只修改 Desktop TypeScript、样式和文档，
没有 Rust 行为变化，未重新运行 `cargo llvm-cov`；没有可用于本次 Desktop 改动的同范围基线。
完整 Desktop 行覆盖率也未测量，后续应将主进程、preload 和 renderer 纳入统一覆盖率工具。

Node V8 经 tsx 执行新增 `startup-recovery.ts` 的专项测量为 **100.00%（28/28 行）**、
**93.75%（15/16 分支）**。这只代表删除文件模块，不代表整个 Desktop 或 Rust workspace。
未覆盖分支是 unlink 阶段发生非 ENOENT 的 I/O 错误；目录/符号链接预检错误、缺失文件、重复
删除和其他数据库保留已覆盖。汇总见
[覆盖率 JSON](desktop-startup-database-reset-coverage.json)。

测量命令（在 `apps/desktop` 执行）：

```sh
node --experimental-test-coverage --import tsx --test \
  '--test-coverage-include=**/src/startup-recovery.ts' \
  --test-reporter=lcov --test-reporter-destination=/tmp/ait-startup-recovery.lcov \
  test/startup-recovery.test.ts
```

macOS arm64、Node 26.8.2 的执行结果与覆盖率分别记录：

- `npm run typecheck`、`npm run build` 通过；`npm test`：146 通过，0 失败。
- 新增浏览器恢复流程 3 项通过：取消、路径转义、默认无自动删除、导航冻结、等待期间禁用、
  删除错误后重试、成功重载和普通错误只提供启动重试。
- `node --test test/integration/startup-recovery.test.mjs`：1 项通过。通过真实 daemon 和临时
  SQLite 旧库，验证默认取消、并发删除/启动阻断、确认过程中出现 daemon 时拒绝删除、非自有
  daemon 不开放恢复、文件预检失败可重试、参数无法改写删除路径、其他 profile/项目库保留，
  以及删除后空项目列表和内置 Agent 初始化。
- `npm run test:browser`：83 通过，2 失败。失败分别是 Codex 导入菜单的焦点断言，以及
  Composer 的 `Saved Agent` 标签选择器匹配两个控件。用 HEAD 版本的 renderer 经 TypeScript
  转译后替换隔离 fixture 的构建文件，两项均复现相同错误；随后恢复本次构建文件。
  最后界面调整后，恢复专项 3 项和真实 daemon 集成 1 项再次通过。
- 已目视检查隔离 fixture 的恢复页截图 `/tmp/ait-startup-recovery.png`。
- `cargo fmt --all --check`、`cargo clippy --workspace --all-targets -- -D warnings`、
  `git diff --check` 通过。
- `cargo test --workspace --no-fail-fast -- --test-threads=1`（默认 features）：
  **481 通过、0 失败、5 忽略**，包含 1 项文档测试；没有额外排除源码或测试。
  5 项忽略沿用已有真实模型/外部 worker 环境要求。本次编译和 rustdoc 启动较慢，最终完整
  命令退出码为 0，没有把等待中的检查记为通过。

原生确认框通过 Electron 主进程替身验证选项和控制流；未进行真实原生对话框点击或用户数据
删除。发行包、Linux/Windows 未实测。端口健康检查不能证明其他端口的旧进程已停止，使用者
需按确认框提示关闭其他 Ait 后台；多文件删除中途 I/O 失败不是原子回滚。
